//! Subagentes.
//!
//! El agente principal puede repartir trabajo: mandar a otro que se lea un
//! módulo entero, que rastree de dónde sale un fallo, que revise un diff. Cada
//! subagente tiene su propia ventana de contexto.
//!
//! ## Con qué va cada uno
//!
//! Con lo que haya configurado el usuario, y no solo con la suscripción. Un
//! subagente es un `claude -p` aparte cuando se elige un modelo de `claude`, y
//! una conversación HTTP con [`remoto`] cuando se elige cualquier otro de los
//! declarados en `proveedores.toml`. Estuvo atado a los tres alias de `claude`
//! mucho tiempo y era el mismo fallo de siempre: veinte modelos configurados y
//! el menú ofreciendo tres: [`Motor`] es la pieza que resuelve el nombre una
//! sola vez y deja al reparto sin saber por dónde va cada encargo.
//!
//! ## Por qué esto es más que «hacer varias preguntas»
//!
//! El motivo no es la velocidad, es el **contexto**. Leerse veinte ficheros
//! para responder una pregunta llena la ventana del agente principal de texto
//! que ya no le sirve, y a partir de ahí razona peor sobre todo lo demás.
//! Delegar esa lectura devuelve quince líneas útiles en lugar de veinte
//! ficheros, y el hilo principal se queda limpio.
//!
//! ## Qué herramientas tienen, y por qué justo esas
//!
//! `Read`, `Grep`, `Glob`. Ni `Bash`, ni `Edit`, ni `Write`.
//!
//! Es un salto de capacidad grande y un riesgo contenido: con esas tres un
//! subagente puede leerse un proyecto entero y responder de verdad, y no puede
//! cambiar ni un byte ni ejecutar nada. El invariante de vsioncode sigue en
//! pie: lo único que toca el sistema es un comando en tu terminal, y ese lo
//! lanzas tú o lo lanza vsioncode a la vista.
//!
//! Un subagente por HTTP no tiene herramientas que declarar —la API de chat no
//! sabe de eso—, así que se las damos nosotros en [`lector`]: las mismas tres,
//! escritas en Rust y encerradas en el directorio de trabajo. Ahí la promesa no
//! depende de que otro proceso la cumpla, sino de este código.
//!
//! Si algo hay que escribir, el subagente lo dice y el agente principal lo
//! propone como comando. La cadena de mando no cambia.

use std::process::Stdio;

use color_eyre::eyre::{Result, eyre};
use serde_json::Value;
use tokio::process::Command;

use std::collections::HashMap;

use crate::ai::http::Proveedor;
use crate::proveedores::{Config, ModeloToml, ProveedorToml};
use crate::slash;

pub mod lector;
pub mod remoto;

/// Herramientas de un subagente. Solo leen.
///
/// Cambiar esta lista es cambiar el modelo de seguridad del proyecto entero,
/// así que hay un test que la fija: si alguien añade `Bash` o `Write`, falla.
pub const HERRAMIENTAS: &[&str] = &["Read", "Grep", "Glob"];

/// Nunca menos de estos, aunque la máquina vaya ahogada.
///
/// Con uno solo esto dejaría de ser repartir trabajo, y el caso de dos en
/// paralelo está medido y funciona en el equipo más modesto que se contempla.
pub const MIN_A_LA_VEZ: usize = 2;

/// Nunca más de estos, por mucha memoria que sobre.
///
/// El límite deja de ser la máquina y pasa a ser lo que vuelve: cada subagente
/// devuelve un informe a la ventana del principal, y cuatro informes largos ya
/// desplazan lo que se estaba haciendo.
pub const TOPE_A_LA_VEZ: usize = 4;

/// El modelo de un subagente cuando nadie ha dicho cuál.
///
/// No es el del jefe. Un subagente solo lee —`Read`, `Grep`, `Glob`— y lo que
/// devuelve es un resumen de lo que ha encontrado: ese trabajo lo hace bien un
/// modelo de gama media, y hacerlo con el más caro multiplica el gasto por
/// cuatro sin mejorar el informe. Heredar al jefe era la conducta anterior, y
/// era herencia por omisión, no una decisión de nadie.
pub const POR_DEFECTO: &str = "sonnet";

/// Reconoce un modelo por su alias, venga suelto o dentro de un id completo.
///
/// Hace falta lo segundo porque `cfg.model` puede traer tanto `sonnet` —lo que
/// escribe `/modelo`— como `claude-haiku-4-5-20251001`, que es lo que se usa al
/// probar. Devolver el alias y no el id deja una sola forma de comparar gamas.
pub fn reconocer(nombre: &str) -> Option<&'static str> {
    let n = nombre.trim().to_ascii_lowercase();
    slash::MODELOS
        .iter()
        .map(|(a, _)| *a)
        .find(|a| n.contains(a))
}

/// En qué escalón de gama está: 0 el más barato.
///
/// El orden sale de [`slash::MODELOS`] y no de una lista propia porque es el
/// mismo orden que ve el usuario en `/modelo`. Dos listas ordenadas a mano
/// acabarían discrepando, y la discrepancia se pagaría en cuota.
fn escalon(alias: &str) -> usize {
    slash::MODELOS
        .iter()
        .position(|(a, _)| *a == alias)
        .unwrap_or(0)
}

/// El modelo de los subagentes cuando el usuario no ha fijado ninguno.
///
/// Regla: **nunca por encima de quien lo manda, y por defecto no pasa de
/// [`POR_DEFECTO`]**. Con opus arriba, los subagentes van con sonnet; con haiku
/// arriba, con haiku — que un ayudante cueste más que su jefe no tiene sentido
/// en ninguna dirección.
///
/// Si el jefe no se reconoce —un modelo de Groq o de NVIDIA, o ninguno— se cae
/// en [`POR_DEFECTO`] en vez de arrastrar lo que hubiera en `cfg.model`. Ese
/// campo conserva el último alias de claude aunque se haya cambiado a otro
/// proveedor, así que fiarse de él ahí sería fiarse de un dato viejo.
pub fn modelo_por_defecto(jefe: Option<&str>) -> String {
    match jefe.and_then(reconocer) {
        Some(t) if escalon(t) < escalon(POR_DEFECTO) => t.to_string(),
        _ => POR_DEFECTO.to_string(),
    }
}

/// Baja de gama lo que haya pedido el modelo si se pasa del techo.
///
/// El techo lo pone quien manda de verdad: el usuario si ha fijado un modelo, y
/// si no, [`modelo_por_defecto`]. Bajar es libre —el modelo sabe mejor que
/// nadie si un encargo es un `grep` con patas— y subir no, porque entonces el
/// límite de gasto lo decidiría justo quien no lo paga.
///
/// Esto solo se aplica a los encargos que vienen en un bloque `agente:`. Los
/// que lanza el usuario con `/agente` no pasan por aquí: su palabra es el techo.
pub fn bajo_techo(encargos: &mut [Encargo], techo: &str) {
    match reconocer(techo) {
        // El techo está en la escalera de claude: se compara escalón a
        // escalón, y lo que no es de esa escalera no se toca. Un modelo de
        // Groq o de NVIDIA no está por encima ni por debajo de sonnet — es
        // otra cuenta, con otro precio, y ordenarlos entre sí sería inventar
        // una equivalencia que no existe.
        Some(t) => {
            let tope = escalon(t);
            for e in encargos {
                if e.modelo
                    .as_deref()
                    .and_then(reconocer)
                    .is_some_and(|a| escalon(a) > tope)
                {
                    e.modelo = Some(techo.to_string());
                }
            }
        }
        // El usuario ha fijado un modelo de otro proveedor. Pedir uno de
        // claude es entonces salirse de lo que él decidió y gastar de su
        // suscripción, así que eso sí se corrige.
        None => {
            for e in encargos {
                if e.modelo.as_deref().and_then(reconocer).is_some() {
                    e.modelo = Some(techo.to_string());
                }
            }
        }
    }
}

/// Lo que ocupa un subagente, medido.
///
/// Entre 260 y 270 MB de RSS por proceso `claude -p` en el equipo de
/// referencia. Se redondea hacia arriba: pasarse de optimista aquí se paga en
/// swap, con la TUI abierta.
const COSTE_MB: u64 = 280;

/// Lo que se le deja al resto del sistema antes de repartir.
///
/// La TUI, el daemon del agente principal y el shell con lo que esté corriendo
/// dentro. Sin esta reserva, el cálculo se gastaría en subagentes la memoria
/// que hace falta para atender lo que devuelvan.
const RESERVA_MB: u64 = 400;

/// Cuántos pueden trabajar a la vez, según la memoria que quede.
///
/// Antes era un `2` fijo justificado por los núcleos del equipo, y el número
/// que manda no es ese: cada subagente es un proceso con su modelo detrás y lo
/// que se acaba es la RAM, no la CPU —la mayor parte del tiempo se va
/// esperando a la red—. Se recalcula en cada reparto porque la memoria libre
/// de dentro de un minuto no es la de ahora, y con subagentes vivos ya cuenta
/// lo que están gastando: el propio cálculo se frena solo.
pub fn max_a_la_vez() -> usize {
    cupo_para(memoria_disponible_mb())
}

/// La aritmética sola, para poder probarla sin depender de la máquina.
fn cupo_para(disponible_mb: Option<u64>) -> usize {
    let Some(libre) = disponible_mb else {
        // Sin dato no se especula: se hace lo que se hacía antes de existir
        // este cálculo.
        return MIN_A_LA_VEZ;
    };
    let util = libre.saturating_sub(RESERVA_MB);
    ((util / COSTE_MB) as usize).clamp(MIN_A_LA_VEZ, TOPE_A_LA_VEZ)
}

/// Memoria realmente disponible, en megas.
///
/// `MemAvailable` y no `MemFree`: el segundo no cuenta la caché reclamable y
/// en un equipo con horas de uso da un número mucho peor que la realidad, que
/// aquí se traduciría en no repartir nunca.
#[cfg(not(windows))]
fn memoria_disponible_mb() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let linea = info.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = linea.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

/// Lo mismo en Windows, donde lo cuenta el sistema y no un fichero.
///
/// `ullAvailPhys` es el equivalente de `MemAvailable`: lo que se puede repartir
/// sin empezar a paginar. Sin esto la respuesta era `None` y el cupo caía al
/// mínimo de dos subagentes en un equipo de 64 GB, que funciona pero reparte
/// mucho menos de lo que se puede.
#[cfg(windows)]
fn memoria_disponible_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut m = MEMORYSTATUSEX {
        // La estructura lleva su propio tamaño dentro: es como el sistema sabe
        // con qué versión de ella se le está hablando. Sin rellenarlo, la
        // llamada falla.
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then_some(m.ullAvailPhys / (1024 * 1024))
}

/// Lo que le pide el agente principal a uno de los suyos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encargo {
    /// Para qué es, en dos palabras. Se enseña mientras trabaja.
    pub titulo: String,
    /// Lo que tiene que hacer.
    pub tarea: String,
    /// Con qué modelo, si alguien lo ha dicho.
    ///
    /// `None` no significa «el del jefe»: significa que nadie ha elegido, y
    /// entonces decide [`modelo_por_defecto`]. La diferencia importa porque la
    /// herencia silenciosa es justo lo que se vino a quitar de aquí.
    pub modelo: Option<String>,
}

/// Cómo acabó.
#[derive(Debug, Clone)]
pub struct Resultado {
    pub titulo: String,
    pub texto: String,
    pub coste_usd: f64,
    pub ok: bool,
}

/// Instrucciones que se le añaden a cada subagente.
///
/// Cortas y exigentes en una cosa: que responda con lo que ha averiguado, no
/// con un relato de lo que ha hecho. Lo que vuelve va directo a la ventana de
/// contexto del agente principal, y ahí cada línea de paja desplaza una línea
/// útil.
pub(crate) const SISTEMA: &str = "\
Eres un subagente de vsioncode. Te han encargado una tarea concreta y tu respuesta \
vuelve a otro agente, no a una persona.

- Responde con HALLAZGOS, no con un diario de lo que has hecho. Nada de «primero \
he mirado…»: directamente lo que has encontrado.
- Sé concreto: rutas de fichero y números de línea cuando los tengas.
- Si algo no lo has podido comprobar, dilo en vez de suponerlo. Una suposición \
tuya se convierte en un hecho para quien te lea.
- Sé breve. Lo que devuelvas ocupa sitio en la ventana de otro.
- Solo puedes leer. Si hace falta cambiar o ejecutar algo, dilo y ya lo hará quien \
corresponda.
- Responde en español.";

/// Con qué se lanza un subagente.
///
/// Un nombre de modelo ya no basta para saber a quién se le pregunta: `haiku`
/// es un proceso `claude` con la suscripción del usuario y `qwen3.6-27b` es una
/// petición HTTP a Groq con su clave. Son dos caminos distintos —proceso con
/// herramientas contra protocolo de texto—, y quien reparte no tiene por qué
/// saber cuál toca: resuelve el nombre una vez, aquí, y después solo lanza.
#[derive(Debug, Clone)]
pub enum Motor {
    /// El proceso `claude -p`, con las herramientas de serie.
    Daemon(String),
    /// Un proveedor de los declarados en el fichero. Ver [`remoto`].
    ///
    /// En `Box` porque un [`Proveedor`] son cinco cadenas y esta variante
    /// haría el enum cuatro veces más grande que la otra, que es la que se
    /// mueve en cada reparto.
    Http(Box<Proveedor>),
}

impl Motor {
    /// Cómo se llama en la conversación. Es lo que se enseña al lanzarlo, y lo
    /// único que hace visible con qué está trabajando cada subagente.
    pub fn nombre(&self) -> String {
        match self {
            Self::Daemon(m) => m.clone(),
            Self::Http(p) => format!("{} · {}", p.id, p.modelo),
        }
    }
}

/// Un subagente que se puede lanzar **de verdad** en esta máquina.
///
/// Las cuatro cosas van juntas a propósito: el nombre que se enseña, de quién
/// es, para qué va bien y con qué se lanza. Mientras el menú se armaba por un
/// lado y el lanzamiento por otro, se podía ofrecer un modelo que al elegirlo
/// no arrancaba — y eso es peor que no ofrecerlo, porque el usuario ya ha
/// gastado la decisión.
#[derive(Debug, Clone)]
pub struct Posible {
    /// Cómo se escribe en `/agente` y cómo sale en el menú.
    pub nombre: String,
    /// De quién es. Para agrupar la lista y para decirlo en la nota.
    pub proveedor: String,
    /// Lo que se pinta a la derecha.
    pub detalle: String,
    pub motor: Motor,
}

/// Cómo se llama un modelo en la lista de subagentes.
///
/// Los de `claude` por su **alias** —es lo que entiende la escalera de gamas y
/// lo que sabe leer `/agente <alias> <tarea>`—; los demás por su nombre corto,
/// sin el prefijo del fabricante. En los dos casos una sola palabra: el nombre
/// va delante de la tarea, y un id largo o con espacios acabaría leyéndose como
/// la primera palabra del encargo.
fn nombre_de(p: &ProveedorToml, m: &ModeloToml) -> Option<String> {
    if p.es_daemon() {
        // Lo que el proceso no sabe reconocer se queda fuera: sin gama no hay
        // techo que aplicarle, y `--model` con un id inventado falla entero.
        return reconocer(&m.id).map(str::to_string);
    }
    Some(m.corto().to_string())
}

/// Todo lo que puede llevar un subagente en esta máquina.
///
/// Sale del fichero del usuario entero, no solo del bloque de `claude`.
/// Durante mucho tiempo fue justo al revés —los tres alias de la suscripción y
/// nada más—, y con cinco proveedores y veinte modelos configurados eso
/// significaba que diecisiete de ellos no existían para `/agente`. Es el mismo
/// fallo de §9 que ya se pagó dos veces: una opción que la interfaz no enseña
/// no existe para quien la usa.
///
/// Y al revés que un catálogo escrito en el binario: si no hay nada declarado
/// y tampoco está el proceso `claude`, esto queda **vacío** y el menú no ofrece
/// nada. No hay lista de consolación — ofrecer modelos que no se pueden lanzar
/// es prometer lo que no se tiene.
pub fn posibles(
    catalogo: &Config,
    entorno: &HashMap<String, String>,
    binario: &str,
) -> Vec<Posible> {
    let hay_claude = hay_daemon(binario);

    // Un mismo modelo puede servirlo más de un proveedor —`gpt-oss-120b` lo
    // tienen Groq y NVIDIA—, y ahí el nombre corto ya no dice a quién se le
    // pregunta. Solo esos se escriben con su servidor delante: hacerlo con
    // todos alargaría veinte nombres para desambiguar dos.
    let mut todos: Vec<String> = Vec::new();
    for p in &catalogo.proveedor {
        for m in p.a_elegir() {
            if let Some(n) = nombre_de(p, &m) {
                todos.push(n);
            }
        }
    }

    let mut v: Vec<Posible> = Vec::new();
    for p in &catalogo.proveedor {
        if p.es_daemon() && !hay_claude {
            continue;
        }
        // Que falte la clave se dice aquí, en la propia fila. Enterarse al
        // pulsar de que ese proveedor no puede trabajar es enterarse tarde.
        let sin_clave =
            !p.es_daemon() && p.clave(entorno).is_empty() && !p.base_url.contains("localhost");

        for m in p.a_elegir() {
            let Some(corto) = nombre_de(p, &m) else {
                continue;
            };
            let nombre = if todos.iter().filter(|n| **n == corto).count() > 1 {
                format!("{}/{corto}", p.id)
            } else {
                corto
            };
            if v.iter().any(|x| x.nombre == nombre) {
                continue;
            }
            let nota = m.nota_util();
            let detalle = match (sin_clave, nota.is_empty()) {
                (true, _) => format!("{} — le falta la clave", p.id),
                (false, true) => p.id.clone(),
                (false, false) => format!("{} · {nota}", p.id),
            };
            let motor = if p.es_daemon() {
                Motor::Daemon(nombre.clone())
            } else {
                Motor::Http(Box::new(p.con_modelo(entorno, &m)))
            };
            v.push(Posible {
                nombre,
                proveedor: p.id.clone(),
                detalle,
                motor,
            });
        }
    }

    // Sin fichero que leer —la primera vez, antes de configurar nada— los
    // alias de serie son lo único que hay, y solo si el proceso está.
    if v.is_empty() && hay_claude {
        return slash::MODELOS
            .iter()
            .map(|(n, d)| Posible {
                nombre: (*n).to_string(),
                proveedor: "claude".to_string(),
                detalle: (*d).to_string(),
                motor: Motor::Daemon((*n).to_string()),
            })
            .collect();
    }
    v
}

/// De un nombre escrito a mano al de la lista y al motor que lo lanza.
///
/// Acepta lo que ofrece el menú y también el identificador entero, que es lo
/// que se copia del fichero de configuración cuando uno lo tiene delante.
/// Devuelve el nombre **de la lista** y no el escrito: si tecleas
/// `claude-haiku-4-5-20251001`, en la conversación lees `haiku`, que es como se
/// llama aquí.
///
/// Lo que no reconoce devuelve `None` y quien llame decide: `/agente` avisa, y
/// un bloque `agente(…)` del cerebro cae en el defecto.
pub fn resolver(
    catalogo: &Config,
    entorno: &HashMap<String, String>,
    binario: &str,
    nombre: &str,
) -> Option<(String, Motor)> {
    let n = nombre.trim();
    if n.is_empty() {
        return None;
    }
    let posibles = posibles(catalogo, entorno, binario);
    if let Some(p) = posibles.iter().find(|p| p.nombre.eq_ignore_ascii_case(n)) {
        return Some((p.nombre.clone(), p.motor.clone()));
    }
    // Por el id completo: `qwen/qwen3.6-27b` o `claude-haiku-4-5-20251001`.
    for p in &catalogo.proveedor {
        for m in p.a_elegir() {
            if !m.id.eq_ignore_ascii_case(n) {
                continue;
            }
            let Some(corto) = nombre_de(p, &m) else {
                continue;
            };
            let largo = format!("{}/{corto}", p.id);
            if let Some(x) = posibles
                .iter()
                .find(|x| x.nombre == corto || x.nombre == largo)
            {
                return Some((x.nombre.clone(), x.motor.clone()));
            }
        }
    }
    None
}

/// ¿Está el proceso `claude` en esta máquina?
///
/// Se pregunta para no ofrecer subagentes de suscripción a quien no la tiene.
/// Un menú que enseña `sonnet` en un equipo sin `claude` promete algo que al
/// elegirlo falla, y eso es peor que no ofrecer nada — la regla de §9 vale en
/// los dos sentidos.
///
/// A mano y sin dependencias: `which` es recorrer el `PATH` y mirar si existe.
pub fn hay_daemon(binario: &str) -> bool {
    crate::rutas::en_el_path(binario)
}

/// Lanza un subagente y espera su respuesta.
///
/// El motor ya no es opcional: a estas alturas alguien ha decidido con qué va.
/// Dejarlo en `Option` volvía a abrir la puerta al «pues el del jefe», que es
/// exactamente lo que se quitó.
pub async fn lanzar(
    motor: &Motor,
    binario: &str,
    cwd: &str,
    encargo: &Encargo,
) -> Result<Resultado> {
    match motor {
        Motor::Daemon(modelo) => lanzar_daemon(binario, modelo, cwd, encargo).await,
        Motor::Http(p) => remoto::lanzar(p, cwd, encargo).await,
    }
}

/// El subagente de siempre: un `claude -p` con `Read`, `Grep` y `Glob`.
pub async fn lanzar_daemon(
    binario: &str,
    modelo: &str,
    cwd: &str,
    encargo: &Encargo,
) -> Result<Resultado> {
    let mut cmd = Command::new(binario);
    cmd.arg("-p")
        .arg(&encargo.tarea)
        .arg("--output-format")
        .arg("json")
        .arg("--tools")
        .arg(HERRAMIENTAS.join(","))
        // Sin esto se quedaría pidiendo un permiso que nadie puede conceder:
        // aquí no hay ninguna persona mirando.
        .arg("--allowedTools")
        .arg(HERRAMIENTAS.join(" "))
        .arg("--setting-sources")
        .arg("")
        .arg("--strict-mcp-config")
        .arg("--disable-slash-commands")
        .arg("--append-system-prompt")
        .arg(SISTEMA)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    cmd.arg("--model").arg(modelo);

    let salida = cmd
        .output()
        .await
        .map_err(|e| eyre!("no pude lanzar el subagente: {e}"))?;

    let texto = String::from_utf8_lossy(&salida.stdout);
    let v: Value = serde_json::from_str(texto.trim())
        .map_err(|_| eyre!("el subagente no devolvió nada legible"))?;

    let ok = !v["is_error"].as_bool().unwrap_or(false);
    Ok(Resultado {
        titulo: encargo.titulo.clone(),
        texto: v["result"]
            .as_str()
            .unwrap_or("(sin respuesta)")
            .to_string(),
        coste_usd: v["total_cost_usd"].as_f64().unwrap_or(0.0),
        ok,
    })
}

/// Saca los encargos de una respuesta del agente principal.
///
/// La sintaxis es un bloque marcado, igual que los comandos:
///
/// ````text
/// ```agente: leer el módulo de audio
/// Dime qué hace src/voz.rs y qué invariantes asume.
/// ```
/// ````
///
/// Y el modelo se elige entre paréntesis, pegado a la etiqueta:
///
/// ````text
/// ```agente(haiku): contar los ficheros de tests
/// ¿Cuántos hay en tests/ y qué prueba cada uno?
/// ```
/// ````
///
/// Va en la propia valla y no en una línea de dentro porque lo de dentro es la
/// tarea, y la tarea se la lee otro modelo: un `modelo: haiku` en la primera
/// línea sería a la vez configuración y texto del encargo, y quien lo recibe no
/// puede distinguirlos.
///
/// El nombre viaja tal y como lo escribió el modelo y lo resuelve quien
/// reparte, que es el único que sabe qué hay configurado en esta máquina. Un
/// nombre que no existe **no rompe nada y no avisa**: se cae en el defecto. Un
/// `--model` inventado haría fallar el proceso entero por una errata, y con qué
/// va cada subagente se dice al lanzarlo, así que la corrección se ve sin
/// necesidad de un aviso más.
///
/// Se eligió un bloque y no una frase por lo mismo que con todo lo demás:
/// detectar intenciones por parecido de texto falla en los dos sentidos, y
/// aquí un falso positivo cuesta dinero y una llamada al modelo.
pub fn extraer(md: &str) -> Vec<Encargo> {
    let mut encargos = Vec::new();
    let mut dentro: Option<(String, Option<String>)> = None;
    let mut cuerpo = String::new();

    for linea in md.lines() {
        let t = linea.trim_start();
        if t.starts_with("```") {
            if let Some((titulo, modelo)) = dentro.take() {
                let tarea = cuerpo.trim().to_string();
                if !tarea.is_empty() {
                    encargos.push(Encargo {
                        titulo,
                        tarea,
                        modelo,
                    });
                }
                cuerpo.clear();
                continue;
            }
            let etiqueta = t.trim_start_matches('`').trim();
            if let Some((titulo, modelo)) = cabecera(etiqueta) {
                dentro = Some((titulo, modelo));
                cuerpo.clear();
            }
            continue;
        }
        if dentro.is_some() {
            cuerpo.push_str(linea);
            cuerpo.push('\n');
        }
    }
    encargos
}

/// Lee la valla de apertura: `agente`, `agente: título`, `agente(modelo): …`.
///
/// Devuelve `None` si no es un encargo. Lo que va detrás de `agente` tiene que
/// empezar por `:`, por `(` o por un espacio; sin esa condición, `agentes:` —o
/// cualquier palabra que empiece igual— abriría un encargo con un título
/// absurdo, que es lo que hacía antes.
fn cabecera(etiqueta: &str) -> Option<(String, Option<String>)> {
    let resto = etiqueta.strip_prefix("agente")?;

    // El nombre se guarda tal cual, sin comprobar que exista. Aquí no hay
    // forma de saberlo: los modelos salen del fichero del usuario y este
    // módulo no lo lee. Quien reparte sí lo sabe, y es quien decide qué hacer
    // con un nombre que no reconoce.
    let (modelo, resto) = match resto.strip_prefix('(').and_then(|r| r.split_once(')')) {
        Some((m, tras)) => {
            let m = m.trim();
            ((!m.is_empty()).then(|| m.to_string()), tras)
        }
        None => (None, resto),
    };

    let titulo = match resto.strip_prefix(':') {
        Some(r) => r.trim(),
        None if resto.is_empty() || resto.starts_with(char::is_whitespace) => resto.trim(),
        // Ni `:` ni espacio: era otra palabra que empieza igual.
        None => return None,
    };

    Some((
        if titulo.is_empty() {
            "encargo".to_string()
        } else {
            titulo.to_string()
        },
        modelo,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::proveedores::{Config, ModeloToml, ProveedorToml};

    /// Un binario que existe seguro, para las pruebas que necesitan «hay
    /// claude». Lo que se comprueba es la decisión, no el proceso.
    /// Un binario que existe seguro, para las pruebas que necesitan «hay
    /// daemon». `/bin/sh` no vale: en Windows no existe, y allí estas pruebas
    /// no comprobaban lo suyo sino que faltaba un fichero de Unix.
    ///
    /// El ejecutable que corre la prueba existe siempre y en los dos sistemas,
    /// que es exactamente lo que hace falta.
    fn hay() -> String {
        std::env::current_exe()
            .expect("el ejecutable de la prueba tiene que existir")
            .display()
            .to_string()
    }
    const NO_HAY: &str = "/no/existe/claude";

    fn modelo(id: &str, nota: &str) -> ModeloToml {
        ModeloToml {
            id: id.into(),
            nota: nota.into(),
            esfuerzo: None,
            pensar: None,
            imagenes: false,
        }
    }

    fn servidor(id: &str, tipo: &str, modelos: Vec<ModeloToml>) -> ProveedorToml {
        ProveedorToml {
            id: id.into(),
            tipo: tipo.into(),
            base_url: "https://ejemplo/v1".into(),
            api_key: "una-clave".into(),
            modelo: String::new(),
            modelos,
        }
    }

    fn fichero() -> Config {
        Config {
            por_defecto: None,
            env_file: None,
            proveedor: vec![
                servidor("claude", "daemon", Vec::new()),
                servidor(
                    "groq",
                    "openai",
                    vec![
                        modelo("openai/gpt-oss-120b", "equilibrado"),
                        modelo("qwen/qwen3.6-27b", "el más rápido"),
                    ],
                ),
            ],
        }
    }

    /// El fallo que traía esto: `/agente` ofrecía los tres alias de la
    /// suscripción y nada más. Quien tenía cinco proveedores declarados podía
    /// hablar con todos sus modelos y delegar solo en tres.
    #[test]
    fn se_delega_en_lo_que_hay_configurado() {
        let v = posibles(&fichero(), &HashMap::new(), &hay());
        let nombres: Vec<&str> = v.iter().map(|p| p.nombre.as_str()).collect();
        assert!(nombres.contains(&"gpt-oss-120b"), "{nombres:?}");
        assert!(nombres.contains(&"qwen3.6-27b"), "{nombres:?}");
        // Y los de claude siguen estando, por su alias.
        assert!(nombres.contains(&"sonnet"), "{nombres:?}");

        // Cada uno con su motor: el de claude es un proceso y el de Groq no.
        let groq = v.iter().find(|p| p.nombre == "qwen3.6-27b").unwrap();
        assert!(matches!(groq.motor, Motor::Http(_)));
        assert_eq!(groq.proveedor, "groq");
        assert!(groq.detalle.contains("groq"), "{}", groq.detalle);
        let claude = v.iter().find(|p| p.nombre == "sonnet").unwrap();
        assert!(matches!(claude.motor, Motor::Daemon(_)));
    }

    /// Sin el proceso `claude` no se ofrece la suscripción: elegirla fallaría.
    #[test]
    fn sin_el_proceso_no_se_ofrecen_sus_alias() {
        let v = posibles(&fichero(), &HashMap::new(), NO_HAY);
        let nombres: Vec<&str> = v.iter().map(|p| p.nombre.as_str()).collect();
        assert!(!nombres.contains(&"sonnet"), "{nombres:?}");
        assert!(nombres.contains(&"gpt-oss-120b"), "{nombres:?}");
    }

    /// Y sin nada de nada, la lista se queda vacía. No hay lista de
    /// consolación: ofrecer lo que no se puede lanzar es peor que no ofrecer.
    #[test]
    fn sin_configuracion_y_sin_proceso_no_hay_nada() {
        assert!(posibles(&Config::default(), &HashMap::new(), NO_HAY).is_empty());
        // Con el proceso, en cambio, están los alias de serie: es lo único que
        // hay la primera vez, antes de que exista el fichero.
        assert_eq!(
            posibles(&Config::default(), &HashMap::new(), &hay()).len(),
            slash::MODELOS.len()
        );
    }

    /// Dos servidores con el mismo modelo: el nombre corto ya no dice a quién
    /// se le pregunta, así que esos —y solo esos— llevan el servidor delante.
    #[test]
    fn un_modelo_repetido_lleva_su_servidor_delante() {
        let mut cfg = fichero();
        cfg.proveedor.push(servidor(
            "nvidia",
            "openai",
            vec![
                modelo("openai/gpt-oss-120b", ""),
                modelo("meta/llama-3.3-70b-instruct", ""),
            ],
        ));
        let v = posibles(&cfg, &HashMap::new(), &hay());
        let nombres: Vec<&str> = v.iter().map(|p| p.nombre.as_str()).collect();
        assert!(nombres.contains(&"groq/gpt-oss-120b"), "{nombres:?}");
        assert!(nombres.contains(&"nvidia/gpt-oss-120b"), "{nombres:?}");
        // El que no se repite se queda corto.
        assert!(nombres.contains(&"llama-3.3-70b-instruct"), "{nombres:?}");
    }

    /// Que falte la clave se dice antes de elegir, no al pulsar.
    #[test]
    fn un_proveedor_sin_clave_lo_dice_en_su_fila() {
        let mut cfg = fichero();
        cfg.proveedor[1].api_key = "env:NO_EXISTE_ESTA_CLAVE".into();
        let v = posibles(&cfg, &HashMap::new(), &hay());
        let groq = v.iter().find(|p| p.nombre == "qwen3.6-27b").unwrap();
        assert!(groq.detalle.contains("falta la clave"), "{}", groq.detalle);
    }

    /// Se puede escribir el id entero —es lo que uno tiene delante cuando mira
    /// el fichero—, y lo que vuelve es el nombre con el que se llama aquí.
    #[test]
    fn se_resuelve_por_el_nombre_y_por_el_id() {
        let (cfg, e) = (fichero(), HashMap::new());
        let (n, _) = resolver(&cfg, &e, &hay(), "qwen/qwen3.6-27b").unwrap();
        assert_eq!(n, "qwen3.6-27b");
        let (n, _) = resolver(&cfg, &e, &hay(), "QWEN3.6-27B").unwrap();
        assert_eq!(n, "qwen3.6-27b");
        assert!(resolver(&cfg, &e, &hay(), "no-existe").is_none());
        assert!(resolver(&cfg, &e, &hay(), "  ").is_none());
        // Lo que no se ofrece tampoco se resuelve: sin proceso, no hay sonnet.
        assert!(resolver(&cfg, &e, NO_HAY, "sonnet").is_none());
    }

    /// El cupo se mueve con la memoria, pero nunca fuera de sus topes.
    ///
    /// Los dos extremos importan por motivos distintos: por abajo, que una
    /// máquina cargada siga pudiendo repartir trabajo; por arriba, que sobrar
    /// memoria no convierta la respuesta en cuatro informes largos que no
    /// caben en la ventana de quien los pidió.
    #[test]
    fn el_cupo_respeta_sus_topes() {
        assert_eq!(cupo_para(Some(0)), MIN_A_LA_VEZ, "sin memoria, el mínimo");
        assert_eq!(cupo_para(Some(600)), MIN_A_LA_VEZ, "apurado, el mínimo");
        assert_eq!(
            cupo_para(Some(64 * 1024)),
            TOPE_A_LA_VEZ,
            "de sobra, el tope"
        );
        for mb in [0, 300, 900, 1500, 4000, 64 * 1024] {
            let c = cupo_para(Some(mb));
            assert!(
                (MIN_A_LA_VEZ..=TOPE_A_LA_VEZ).contains(&c),
                "{mb} MB dio {c}"
            );
        }
    }

    /// Sin `/proc/meminfo` no se especula: se hace lo de siempre.
    ///
    /// Es el caso de un contenedor raro o un sistema que no es Linux. Fallar
    /// hacia el número que funcionaba antes de existir este cálculo no rompe
    /// nada; adivinar al alza tumbaría la máquina.
    #[test]
    fn sin_dato_de_memoria_se_queda_como_estaba() {
        assert_eq!(cupo_para(None), MIN_A_LA_VEZ);
    }

    /// En esta máquina el dato existe y el resultado es utilizable.
    #[test]
    fn aqui_hay_dato_de_memoria() {
        assert!(memoria_disponible_mb().is_some_and(|mb| mb > 0));
        assert!((MIN_A_LA_VEZ..=TOPE_A_LA_VEZ).contains(&max_a_la_vez()));
    }

    /// Este test **es** el modelo de seguridad, escrito.
    ///
    /// Un subagente que pudiera ejecutar o escribir rompería lo único que
    /// vsioncode promete: que lo que toca el sistema pasa por la terminal, a
    /// la vista. Si alguien añade una herramienta aquí, que sea a sabiendas.
    #[test]
    fn los_subagentes_no_pueden_tocar_nada() {
        for prohibida in ["Bash", "Write", "Edit", "NotebookEdit", "WebFetch", "Agent"] {
            assert!(
                !HERRAMIENTAS.contains(&prohibida),
                "«{prohibida}» le daría a un subagente poder que no debe tener"
            );
        }
        assert!(HERRAMIENTAS.contains(&"Read"), "sin leer no sirve de nada");
    }

    /// La regla de gama, escrita: nunca por encima del jefe, y por defecto
    /// nunca por encima de sonnet.
    ///
    /// Este test **es** el ahorro. Si alguien lo relaja, un opus arriba vuelve
    /// a lanzar cuatro opus abajo y la cuota se va en un reparto.
    #[test]
    fn un_subagente_nunca_cuesta_mas_que_su_jefe() {
        assert_eq!(
            modelo_por_defecto(Some("opus")),
            "sonnet",
            "opus no reparte opus"
        );
        assert_eq!(modelo_por_defecto(Some("sonnet")), "sonnet");
        assert_eq!(
            modelo_por_defecto(Some("haiku")),
            "haiku",
            "ni sube al de arriba"
        );
        // El id completo es lo que hay en `cfg.model` al probar con haiku.
        assert_eq!(
            modelo_por_defecto(Some("claude-haiku-4-5-20251001")),
            "haiku"
        );
        // Sin jefe reconocible —otro proveedor, o ninguno— no se especula.
        assert_eq!(modelo_por_defecto(None), POR_DEFECTO);
        assert_eq!(modelo_por_defecto(Some("openai/gpt-oss-120b")), POR_DEFECTO);
    }

    /// El modelo puede bajar de gama por su cuenta, pero no subir.
    #[test]
    fn el_techo_solo_corta_hacia_arriba() {
        let mut e = vec![
            Encargo {
                titulo: "caro".into(),
                tarea: "t".into(),
                modelo: Some("opus".into()),
            },
            Encargo {
                titulo: "barato".into(),
                tarea: "t".into(),
                modelo: Some("haiku".into()),
            },
            Encargo {
                titulo: "sin decir".into(),
                tarea: "t".into(),
                modelo: None,
            },
        ];
        bajo_techo(&mut e, "sonnet");
        assert_eq!(
            e[0].modelo.as_deref(),
            Some("sonnet"),
            "opus se baja al techo"
        );
        assert_eq!(e[1].modelo.as_deref(), Some("haiku"), "bajar es cosa suya");
        assert_eq!(e[2].modelo, None, "quien no pide nada sigue sin pedir nada");
    }

    /// Un modelo de otro proveedor no se ordena contra la escalera de claude.
    ///
    /// No es que dé igual: es que no hay forma de compararlos. Lo que sí se
    /// corrige es lo contrario — con un techo de otro proveedor, pedir uno de
    /// claude es gastar de la suscripción que el usuario había dejado fuera.
    #[test]
    fn el_techo_y_los_otros_proveedores() {
        let mut e = vec![Encargo {
            titulo: "otro".into(),
            tarea: "t".into(),
            modelo: Some("qwen3.6-27b".into()),
        }];
        bajo_techo(&mut e, "haiku");
        assert_eq!(
            e[0].modelo.as_deref(),
            Some("qwen3.6-27b"),
            "no se toca lo que no está en la escalera"
        );

        let mut e = vec![Encargo {
            titulo: "caro".into(),
            tarea: "t".into(),
            modelo: Some("opus".into()),
        }];
        bajo_techo(&mut e, "qwen3.6-27b");
        assert_eq!(e[0].modelo.as_deref(), Some("qwen3.6-27b"));
    }

    #[test]
    fn el_bloque_puede_traer_modelo() {
        let e = extraer("```agente(haiku): mirar los tests\nqué prueban\n```");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].titulo, "mirar los tests");
        assert_eq!(e[0].modelo.as_deref(), Some("haiku"));
    }

    /// El nombre llega tal cual: aquí no se sabe qué hay configurado.
    ///
    /// Antes se filtraba contra los tres alias de claude, y eso dejaba fuera
    /// cualquier modelo del fichero del usuario: pedir `agente(qwen3.6-27b)`
    /// era lo mismo que no pedir nada. Quien reparte lo resuelve contra el
    /// catálogo y, si no existe, cae en el defecto.
    #[test]
    fn el_nombre_del_modelo_llega_sin_filtrar() {
        let e = extraer("```agente(sonet): revisar\nalgo\n```");
        assert_eq!(e.len(), 1, "el encargo se lanza igual");
        assert_eq!(e[0].titulo, "revisar");
        assert_eq!(e[0].modelo.as_deref(), Some("sonet"));
        let e = extraer("```agente(qwen3.6-27b): revisar\nalgo\n```");
        assert_eq!(e[0].modelo.as_deref(), Some("qwen3.6-27b"));
    }

    /// Sin paréntesis, nadie ha elegido — y eso NO es «el del jefe».
    #[test]
    fn sin_parentesis_no_hay_modelo_elegido() {
        assert_eq!(extraer("```agente: x\ny\n```")[0].modelo, None);
    }

    /// Una palabra que empieza por «agente» no abre un encargo.
    #[test]
    fn no_confunde_una_palabra_parecida() {
        assert!(extraer("```agentes\nlista\n```").is_empty());
        assert!(extraer("```agente-de-viajes: x\ny\n```").is_empty());
    }

    #[test]
    fn saca_un_encargo() {
        let md = "Voy a repartir esto.\n\n```agente: leer el módulo de audio\n\
                  Dime qué hace src/voz.rs.\n```\n\nY sigo.";
        let e = extraer(md);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].titulo, "leer el módulo de audio");
        assert_eq!(e[0].tarea, "Dime qué hace src/voz.rs.");
    }

    #[test]
    fn saca_varios() {
        let md = "```agente: uno\ntarea una\n```\ntexto\n```agente: dos\ntarea dos\n```";
        let e = extraer(md);
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].titulo, "dos");
    }

    #[test]
    fn conserva_las_lineas_del_encargo() {
        let md = "```agente: revisar\nprimera línea\nsegunda línea\n```";
        assert_eq!(extraer(md)[0].tarea, "primera línea\nsegunda línea");
    }

    /// Un bloque de comando no puede confundirse con un encargo: uno se
    /// ejecuta en la terminal y el otro cuesta una llamada al modelo.
    #[test]
    fn no_confunde_un_bloque_de_comando() {
        assert!(extraer("```bash\nls -la\n```").is_empty());
        assert!(extraer("```rust\nfn main() {}\n```").is_empty());
        assert!(extraer("```\nsuelto\n```").is_empty());
    }

    #[test]
    fn un_encargo_vacio_no_cuenta() {
        assert!(extraer("```agente: nada\n\n```").is_empty());
    }

    #[test]
    fn admite_el_bloque_sin_titulo() {
        let e = extraer("```agente\nhaz esto\n```");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].titulo, "encargo");
    }
}
