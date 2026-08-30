//! Comandos de barra para el panel del agente.
//!
//! Escribir `/` en la caja del agente abre un menú con lo que se puede hacer.
//! No es azúcar: es el sitio donde vive todo lo que no cabe en un atajo de
//! teclado. Los atajos son para lo que haces cada minuto; esto es para lo que
//! haces cada rato y no quieres memorizar.
//!
//! Dos reglas de diseño:
//!
//! - **Se descubren solos.** Al teclear `/` aparece la lista filtrada. Nadie
//!   tiene que leerse un manual para enterarse de que existe `/modelo`.
//! - **Nunca se ejecutan por accidente.** Si lo escrito no es un comando
//!   conocido, se manda al agente como pregunta normal. Escribir
//!   `/usr/bin/algo no funciona` en la caja no dispara nada raro.

/// Una acción pedida por el usuario desde la caja del agente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slash {
    /// Lista los comandos.
    Ayuda,
    /// Cambia de modelo, o lista los disponibles si no se da ninguno.
    Modelo(Option<String>),
    /// Cambia de modo, o lista los modos si no se da ninguno.
    Modo(Option<String>),
    /// Vacía la conversación (no el registro de comandos).
    Limpiar,
    /// Enseña exactamente lo que se le mandaría al modelo ahora mismo.
    Contexto,
    /// Enseña los últimos comandos capturados.
    Registro,
    /// Enciende o apaga la captura.
    Captura,
    /// Gasto de la sesión.
    Coste,
    /// Ejecuta un comando en el shell, directamente.
    Ejecutar(String),
    /// Adjunta lo que haya en esa ruta: una imagen, un fichero o una carpeta.
    Adjuntar(String),
    /// Enciende o apaga la consulta de internet.
    Web,
    /// Manda al agente lo que se ve en la pantalla del shell.
    Pantalla,
    /// Mide el micrófono y dice si sirve.
    Microfono,
    /// Análisis a fondo: enciende el modo libre y se pone a ello.
    Analizar(Option<String>),
    /// Traza el plan de un encargo y para: no ejecuta nada hasta que se aprueba.
    ///
    /// Sin argumento no es un comando vacío: es «planifica lo que venimos
    /// hablando», que es como se pide de verdad —primero se cuenta el
    /// problema y luego se pide el plan— y ahorra repetir el encargo entero.
    Plan(Option<String>),
    /// Abre la puesta en marcha: servicios, claves y modelo por defecto.
    Configurar,
    /// Enseña qué son las skills, cuáles hay instaladas y de dónde salen.
    Skills,
    /// Trae una skill de fuera a la carpeta de vsioncode.
    ///
    /// El argumento es de dónde: un repositorio de GitHub, una URL a un
    /// `SKILL.md` o una carpeta del disco.
    SkillInstalar(String),
    /// Delega una tarea en un subagente, o decide con qué modelo se delega.
    Agente(Agente),
    /// Enciende o apaga el modo libre.
    Libre,
    /// Enciende o apaga la captura del ratón.
    Raton,
    /// Enciende o apaga el modo de voz, o cambia de modelo de reconocimiento.
    Voz(Option<String>),
    /// Cierra vsioncode.
    Salir,
}

/// Las tres cosas que se pueden pedir con `/agente`.
///
/// Un enum y no tres comandos porque son la misma idea —repartir trabajo— y
/// tres verbos en el menú enterrarían dos comandos útiles. El precedente es
/// `/skills instalar`: lo más usado no se paga con un subcomando, y lo que se
/// usa una vez sí lo lleva.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Agente {
    /// `/agente` a secas: qué modelo se está usando y cómo se pide otro.
    Como,
    /// `/agente modelo <nombre>`: fija con qué van a partir de ahora.
    Fijar(String),
    /// `/agente [modelo] <tarea>`: lánzalo ya.
    Lanzar {
        /// Solo si el usuario lo ha nombrado. Su palabra no lleva techo.
        modelo: Option<String>,
        tarea: String,
    },
}

/// Lee lo que va detrás de `/agente`.
///
/// El modelo se reconoce **solo como primera palabra y solo si detrás hay
/// tarea**: `/agente sonnet` a secas es una tarea que se llama «sonnet», no un
/// encargo vacío con modelo. Y un encargo que empiece por «opus…» de verdad se
/// escribe con `/agente opus opus…`, que es el precio de no tener que teclear
/// un guion en el caso normal.
///
/// Qué cuenta como modelo sale de `cat` y no de [`MODELOS`]. Con la lista del
/// binario, `/agente qwen3.6-27b mira los tests` mandaba el encargo «qwen3.6-27b
/// mira los tests» al modelo de siempre: un modelo configurado por el usuario no
/// se reconocía ni escribiéndolo entero.
fn agente(arg: Option<String>, cat: &Catalogo) -> Agente {
    let Some(a) = arg else {
        return Agente::Como;
    };
    let Some((primera, resto)) = a.split_once(char::is_whitespace) else {
        return sin_tarea(&a);
    };
    let (p, resto) = (primera.to_ascii_lowercase(), resto.trim());
    if resto.is_empty() {
        return sin_tarea(primera);
    }
    if matches!(p.as_str(), "modelo" | "model") {
        return Agente::Fijar(resto.to_string());
    }
    if cat
        .subagentes
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case(&p))
    {
        return Agente::Lanzar {
            modelo: Some(p),
            tarea: resto.to_string(),
        };
    }
    Agente::Lanzar {
        modelo: None,
        tarea: a,
    }
}

/// Una sola palabra detrás de `/agente`: casi siempre es un encargo corto.
///
/// La excepción es el verbo que fija el defecto. `/agente modelo` a secas no
/// fija nada —falta decir cuál—, así que es la pregunta «¿con qué van?» y no
/// una tarea que se llame «modelo». Antes se lanzaba como encargo: elegir esa
/// fila del menú y pulsar Enter gastaba una llamada al modelo en preguntarle a
/// un subagente por la palabra «modelo».
fn sin_tarea(palabra: &str) -> Agente {
    if matches!(palabra.to_ascii_lowercase().as_str(), "modelo" | "model") {
        return Agente::Como;
    }
    Agente::Lanzar {
        modelo: None,
        tarea: palabra.to_string(),
    }
}

/// Lo que se ofrece en el menú al escribir `/`.
///
/// Solo lo que se usa a diario **y no tiene ya una tecla**. El menú enseña un
/// puñado de filas: cada entrada que sobra entierra una que importa, y llegó a
/// haber diecisiete comandos para siete filas visibles. Lo que no está aquí no
/// desaparece — sigue en [`OCULTOS`], se reconoce igual y se lista en `/ayuda`.
pub const COMANDOS: &[(&str, &str, &str)] = &[
    ("/modelo", "[nombre]", "cambiar de modelo, o ver cuáles hay"),
    (
        "/voz",
        "[tiny|base]",
        "elegir con qué te oye; sin nada, enciende la voz (o Alt+V)",
    ),
    (
        "/configurar",
        "",
        "añadir un servicio o cambiar una clave (Groq, NVIDIA, Gemini…)",
    ),
    (
        "/modo",
        "[nombre]",
        "quién ejecuta: observar · sugerir · conversar · ejecutar",
    ),
    (
        "/plan",
        "[qué]",
        "que lo piense y te enseñe los pasos antes de tocar nada",
    ),
    (
        "/analizar",
        "[qué]",
        "análisis a fondo, sin techo de pasos y con subagentes",
    ),
    (
        "/agente",
        "[tarea]",
        "delegar en un subagente — o ← con la caja vacía",
    ),
    ("/ejecutar", "<comando>", "lanzarlo en el shell ahora mismo"),
    (
        "/skills",
        "[instalar <qué o de dónde>]",
        "manuales que el agente puede pedir; instalar trae uno de fuera",
    ),
    ("/registro", "", "ver los últimos comandos capturados"),
    ("/contexto", "", "ver qué se le está mandando al modelo"),
    ("/web", "", "permitir o no que el agente consulte internet"),
    ("/coste", "", "gasto y cuota de esta sesión"),
    ("/limpiar", "", "vaciar la conversación"),
    (
        "/adjuntar",
        "<ruta>",
        "que el agente vea una imagen o un fichero del disco",
    ),
];

/// Funcionan escritos, pero no ocupan sitio en el menú.
///
/// Los cinco primeros **duplican una tecla que ya existe** — `/libre` lo
/// admitía hasta en su propia descripción, «(o Alt+L)». Los dos últimos son de
/// usar una vez y no volver: medir el micrófono y soltar el ratón.
///
/// Dos se han ido de aquí, y por el mismo motivo las dos veces. `/configurar`
/// no tenía tecla, y quien busca añadir un servicio lo busca justo en el menú.
/// `/voz` sí la tenía, pero solo para **la mitad** de lo que hace: Alt+V
/// enciende y apaga, y elegir el modelo de reconocimiento —lo rápido o lo
/// preciso que te oye— no lo hace ninguna tecla. Un comando que solo funciona
/// si ya sabes que existe no existe.
pub const OCULTOS: &[(&str, &str, &str)] = &[
    ("/ayuda", "", "esta lista — o Alt+H"),
    ("/libre", "", "quitarle la correa al agente — o Alt+L"),
    ("/captura", "", "encender o apagar la captura — o Alt+W"),
    (
        "/pantalla",
        "",
        "que el agente lea lo que hay en pantalla ahora — o Alt+S",
    ),
    ("/salir", "", "cerrar vsioncode — o Alt+Q"),
    (
        "/microfono",
        "",
        "medir el micrófono: habla 5 segundos y te digo qué tal",
    ),
    ("/raton", "", "soltar el ratón para poder seleccionar texto"),
];

/// Los alias de `claude`, como respaldo de lo que no diga el fichero.
///
/// Ya no son «los modelos»: los modelos salen del TOML del usuario, y esta
/// lista solo entra cuando no hay nada que leer —la primera vez, antes de
/// configurar nada— o cuando el bloque de `claude` no declara ninguno. Los
/// entiende el propio proceso, así que valen siempre y no hay que mantener
/// identificadores completos.
///
/// Sigue siendo además el **orden de gama**: `agente::escalon` cuenta la
/// posición en esta lista para no dejar que un subagente cueste más que su
/// jefe. Reordenarla es cambiar el techo de gasto.
pub const MODELOS: &[(&str, &str)] = &[
    (
        "haiku",
        "el más rápido y barato; de sobra para explicar errores",
    ),
    ("sonnet", "equilibrado; el que suele convenir aquí"),
    ("opus", "el más capaz, y el más lento y caro"),
];

/// Modelos de reconocimiento de voz que se ofrecen siempre.
///
/// Los dos que están medidos aquí. `faster-whisper` no instala nada: se baja
/// el que le pidas la primera vez que lo usas, así que lo que de verdad hay en
/// la máquina se mira en la caché —ver [`crate::voz::modelos_descargados`]— y
/// se suma a estos dos. Estos son el suelo, no el techo.
pub const VOCES: &[(&str, &str)] = &[
    (
        "tiny",
        "el más rápido: ~2,5 s por frase. El que conviene para conversar",
    ),
    (
        "base",
        "algo más preciso pero el doble de lento y 200 MB más de RAM",
    ),
];

/// Modos, para ofrecerlos en `/modo`.
pub const MODOS: &[(&str, &str)] = &[
    (
        "observar",
        "solo mira y llena el registro; no llama al modelo",
    ),
    ("sugerir", "al fallar algo, te lo ofrece en la barra"),
    ("conversar", "él propone, tú ejecutas"),
    (
        "ejecutar",
        "él trabaja en la terminal mientras conversas con él",
    ),
];

/// Una opción del menú de autocompletado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidato {
    /// Lo que queda escrito en la caja al aceptarlo.
    pub completar: String,
    /// Lo que se pinta a la izquierda.
    pub etiqueta: String,
    /// La explicación, a la derecha.
    ///
    /// Es `String` y no `&'static str` porque los proveedores no se conocen
    /// al compilar: el detalle de «groq» es el modelo que el usuario haya
    /// puesto en su fichero, y sin esto el menú solo podía enseñar lo que
    /// venía escrito en el binario.
    pub detalle: String,
    /// Si al aceptarlo ya se puede enviar, o todavía faltan argumentos.
    ///
    /// De esto depende que Enter envíe o solo complete: `/limpiar` está listo
    /// en cuanto lo eliges, pero `/modelo` necesita que digas cuál.
    pub completo: bool,
    /// Abre un grupo: es un servidor, y lo que viene detrás son sus modelos.
    ///
    /// No es decoración. `/modelo` ofrece dos clases de cosa en la misma lista
    /// —servidores y modelos— y sin distinguirlas quedaban siete nombres
    /// seguidos donde cuatro eran servidores y tres modelos sueltos, sin nada
    /// que dijera de quién era cada cual ni por qué estaban juntos. Se elige
    /// igual que cualquier otra fila: `/modelo groq` cambia de servidor.
    pub cabecera: bool,
}

/// Una opción de segundo nivel, antes de convertirse en [`Candidato`].
///
/// Los tres datos que no se pueden dar por supuestos: el nombre, la
/// explicación y si al elegirlo el comando ya está entero. Lo tercero se dice
/// por opción y no por comando porque dentro de `/agente` conviven las dos
/// cosas — un modelo cierra la orden, el verbo `modelo` todavía no.
type Opcion = (String, String, bool);

/// Lo que el menú no puede saber al compilar.
///
/// Los comandos son fijos, están escritos en el binario. Lo que se elige *con*
/// ellos no: los modelos salen del fichero del usuario y los motores de voz de
/// lo que haya descargado. Mientras esas listas vivieron dentro de este
/// módulo, instalar o declarar algo no cambiaba nada de lo que se ve — el
/// mismo fallo que ya se pagó con los proveedores, repetido un nivel más
/// abajo.
///
/// Cada lista trae ya su respaldo aplicado: quien la rellena decide qué
/// enseñar cuando no hay nada configurado. Aquí solo se ordena y se filtra.
#[derive(Debug, Default, Clone)]
pub struct Catalogo {
    /// Servidores declarados, en el orden del fichero: `(id, con qué responde)`.
    pub proveedores: Vec<(String, String)>,
    /// Los modelos de cada servidor, por su id: `(id del servidor, nombre, nota)`.
    ///
    /// Aparte de [`Catalogo::proveedores`] y no dentro para que el menú pueda
    /// intercalarlos: primero el servidor, luego los suyos, luego el
    /// siguiente. Es el mismo orden y la misma agrupación que la ventana de
    /// `/modelo`, y que sean dos sitios no es motivo para que se lean distinto.
    pub modelos: Vec<(String, String, String)>,
    /// Con qué puede ir un subagente: `(alias, para qué sirve)`.
    ///
    /// Alias y no identificadores enteros: es lo que entiende el reparto —el
    /// techo de gama se cuenta en alias— y lo que sabe leer
    /// `/agente <alias> <tarea>`.
    pub subagentes: Vec<(String, String)>,
    /// Modelos de reconocimiento de voz: `(nombre, qué tal)`.
    pub voces: Vec<(String, String)>,
}

/// Qué ofrecer para lo que se lleva escrito.
///
/// Trabaja en tres niveles, que es lo que hace que el menú siga sirviendo
/// después del nombre del comando:
///
/// - `/mod` → los comandos que empiezan así.
/// - `/modelo ` → los servidores y sus modelos.
/// - `/agente modelo ` → con cuál van los subagentes.
///
/// Sin el segundo nivel habría que saberse de memoria que existe `haiku`. Sin
/// el tercero, escribir el verbo dejaba el menú ofreciendo otra vez la palabra
/// «modelo» y no había forma de llegar a la lista desde el propio menú, que es
/// justo para lo que está el menú.
///
/// Todo lo elegible sale de `cat`, que es lo que hay de verdad en esta
/// máquina. Los servidores van **antes** que sus modelos y en el orden del
/// fichero: es el que ha escrito el usuario, y reordenarlo por un criterio
/// nuestro le quitaría el único sitio donde decide qué ve primero.
pub fn candidatos(input: &str, cat: &Catalogo) -> Vec<Candidato> {
    let Some(resto) = input.strip_prefix('/') else {
        return Vec::new();
    };

    match resto.split_once(char::is_whitespace) {
        // Todavía se escribe el nombre del comando.
        None => {
            let pref = resto.to_ascii_lowercase();
            filtrar(
                COMANDOS.iter().map(|(n, a, d)| (*n, *a, *d)),
                &pref,
                |(n, a, d)| Candidato {
                    completar: if a.is_empty() {
                        n.to_string()
                    } else {
                        format!("{n} ")
                    },
                    etiqueta: if a.is_empty() {
                        n.to_string()
                    } else {
                        format!("{n} {a}")
                    },
                    detalle: d.to_string(),
                    // Un argumento entre corchetes es opcional, así que el
                    // comando ya está entero sin él: `/modelo` a secas abre la
                    // ventana de modelos, y quedarse completando el hueco de
                    // un argumento que nadie tiene por qué dar era pedir dos
                    // pulsaciones para una decisión. `<comando>` sí es
                    // obligatorio y sigue esperando.
                    completo: a.is_empty() || a.starts_with('['),
                    cabecera: false,
                },
            )
        }
        // Ya hay un comando; toca ofrecer sus argumentos.
        Some((verbo, arg)) => {
            let v = verbo.to_ascii_lowercase();

            // El tercer nivel se resuelve antes: `/agente modelo …` ya no
            // ofrece verbos, ofrece modelos.
            if matches!(v.as_str(), "agente" | "agent" | "subagente" | "delegar")
                && let Some((sub, cual)) = arg.trim_start().split_once(char::is_whitespace)
                && matches!(sub.to_ascii_lowercase().as_str(), "modelo" | "model")
            {
                let pref = cual.trim().to_ascii_lowercase();
                return filtrar(cat.subagentes.iter(), &pref, move |(n, d)| Candidato {
                    completar: format!("/{verbo} {sub} {n}"),
                    etiqueta: n.clone(),
                    detalle: d.clone(),
                    completo: true,
                    cabecera: false,
                });
            }

            let fijas = |o: &[(&str, &str)]| -> Vec<Opcion> {
                o.iter()
                    .map(|(n, d)| (n.to_string(), d.to_string(), true))
                    .collect()
            };
            let sueltas = |o: &[(String, String)]| -> Vec<Opcion> {
                o.iter()
                    .map(|(n, d)| (n.clone(), d.clone(), true))
                    .collect()
            };

            // Los servidores llevan su marca; solo `/modelo` los tiene.
            let mut cabeceras: Vec<String> = Vec::new();
            let opciones: Vec<Opcion> = match v.as_str() {
                "modelo" | "model" | "m" => {
                    cabeceras = cat.proveedores.iter().map(|(id, _)| id.clone()).collect();
                    agrupar(cat)
                }
                "modo" | "mode" => fijas(MODOS),
                // Los modelos y el verbo que fija el defecto. La tarea es texto
                // libre, así que en cuanto se escribe algo que no es ninguno de
                // estos el filtro se queda vacío y el menú desaparece solo.
                // Detrás de `/agente ` va la tarea, que es texto libre. La
                // lista de agentes ya no sale aquí: vive en la barra de
                // delegar, donde se elige y se escribe el encargo de una vez.
                // Enseñarla también aquí serían dos sitios para la misma
                // decisión, y con dieciocho modelos el menú no era el bueno.
                //
                // Sigue reconociéndose el nombre escrito a mano —`/agente
                // haiku mira los tests`—, porque quien se lo sabe no tiene por
                // qué pasar por la barra.
                "agente" | "agent" | "subagente" | "delegar" => {
                    if arg.trim_start().is_empty() {
                        vec![(
                            "modelo".to_string(),
                            "fijar con qué modelo van los subagentes".to_string(),
                            false,
                        )]
                    } else {
                        return Vec::new();
                    }
                }
                "voz" | "voice" | "hablar" => sueltas(&cat.voces),
                // El menú anuncia `[instalar <de dónde>]` y hasta ahora al
                // escribir el espacio no ofrecía nada: un argumento que la
                // interfaz nombra y no sabe completar.
                "skills" | "skill" => {
                    // Con el verbo ya escrito, lo que falta es de dónde: un
                    // repositorio, una URL o una carpeta. Texto libre.
                    if arg.trim_start().split_once(char::is_whitespace).is_some() {
                        return Vec::new();
                    }
                    vec![(
                        "instalar".to_string(),
                        "usuario/repo, una URL, una carpeta — o describe qué buscas".to_string(),
                        false,
                    )]
                }
                // `/ejecutar` y `/adjuntar` reciben texto libre y rutas del
                // disco: aquí no hay lista que enseñar.
                _ => return Vec::new(),
            };

            let pref = arg.trim().to_ascii_lowercase();
            filtrar(opciones.iter(), &pref, move |(n, d, c)| Candidato {
                completar: format!("/{verbo} {n}"),
                etiqueta: n.clone(),
                detalle: d.clone(),
                completo: *c,
                cabecera: cabeceras.contains(n),
            })
        }
    }
}

/// Los servidores con sus modelos detrás, en el orden del fichero.
///
/// Antes iban los servidores primero y los tres alias del binario al final,
/// sueltos: la lista quedaba en cuatro nombres de servidor seguidos de tres
/// modelos de uno de ellos, sin nada que dijera que `haiku` era de `claude` ni
/// por qué estaban juntos. Agrupado se lee como la ventana de `/modelo`, que
/// es donde se toma la misma decisión.
///
/// Un servidor sin modelos declarados sale igual: elegirlo cambia de servidor,
/// que es una decisión válida por sí sola.
fn agrupar(cat: &Catalogo) -> Vec<Opcion> {
    let mut v: Vec<Opcion> = Vec::new();
    for (id, detalle) in &cat.proveedores {
        v.push((id.clone(), detalle.clone(), true));
        for (de, nombre, nota) in &cat.modelos {
            if de == id {
                v.push((nombre.clone(), nota.clone(), true));
            }
        }
    }
    // Y lo que no sea de ningún servidor declarado, detrás: no puede perderse
    // por no encontrar su grupo.
    for (de, nombre, nota) in &cat.modelos {
        if !cat.proveedores.iter().any(|(id, _)| id == de) {
            v.push((nombre.clone(), nota.clone(), true));
        }
    }
    v
}

/// Filtra por prefijo y, si nada empieza así, por contenido.
///
/// Lo de buscar dentro es lo que hace que escribir «gasto» encuentre
/// `/coste`: quien no recuerda el nombre exacto suele acordarse de una
/// palabra que sí aparece en la descripción.
fn filtrar<T, I, F>(items: I, pref: &str, hacer: F) -> Vec<Candidato>
where
    I: Iterator<Item = T> + Clone,
    T: Clone,
    F: Fn(T) -> Candidato,
{
    let por_prefijo: Vec<Candidato> = items
        .clone()
        .map(&hacer)
        .filter(|c| {
            c.etiqueta
                .trim_start_matches('/')
                .to_ascii_lowercase()
                .starts_with(pref)
        })
        .collect();

    if !por_prefijo.is_empty() || pref.is_empty() {
        return por_prefijo;
    }

    items
        .map(&hacer)
        .filter(|c| {
            c.etiqueta.to_ascii_lowercase().contains(pref)
                || c.detalle.to_ascii_lowercase().contains(pref)
        })
        .collect()
}

/// Interpreta lo escrito en la caja del agente.
///
/// Necesita el [`Catalogo`] por lo mismo que el menú: lo elegible no se sabe
/// al compilar. Sin él, `/agente <un modelo del fichero> <tarea>` no distinguía
/// el modelo de la tarea.
///
/// Devuelve `None` si no es un comando conocido — y entonces el texto se
/// manda al agente tal cual, que es lo que espera cualquiera que escriba una
/// ruta absoluta al principio de una frase.
pub fn parse(input: &str, cat: &Catalogo) -> Option<Slash> {
    let t = input.trim();
    let resto = t.strip_prefix('/')?;
    if resto.is_empty() {
        return Some(Slash::Ayuda);
    }

    let (verbo, arg) = match resto.split_once(char::is_whitespace) {
        Some((v, a)) => (v, a.trim()),
        None => (resto, ""),
    };
    let arg = if arg.is_empty() {
        None
    } else {
        Some(arg.to_string())
    };
    let v = verbo.to_ascii_lowercase();

    // Se aceptan también los nombres en inglés: quien viene de otras
    // herramientas los tiene en los dedos y no cuesta nada.
    match v.as_str() {
        "ayuda" | "help" | "?" => Some(Slash::Ayuda),
        "modelo" | "model" | "m" => Some(Slash::Modelo(arg)),
        "modo" | "mode" => Some(Slash::Modo(arg)),
        "configurar" | "config" | "proveedores" | "providers" => Some(Slash::Configurar),
        "skills" | "skill" => match arg.as_deref().map(str::trim) {
            // `/skills instalar <de dónde>`. Sin el verbo, `/skills` sigue
            // siendo la lista de siempre: lo más usado no se paga con un
            // subcomando.
            Some(a) => match a.split_once(char::is_whitespace) {
                Some((v, resto))
                    if matches!(
                        v.to_ascii_lowercase().as_str(),
                        "instalar" | "instala" | "install" | "añadir" | "add"
                    ) =>
                {
                    let resto = resto.trim();
                    (!resto.is_empty()).then(|| Slash::SkillInstalar(resto.to_string()))
                }
                _ => Some(Slash::Skills),
            },
            None => Some(Slash::Skills),
        },
        "analizar" | "analiza" | "analyze" => Some(Slash::Analizar(arg)),
        "plan" | "planear" | "planifica" | "planificar" | "pasos" => Some(Slash::Plan(arg)),
        "agente" | "agent" | "subagente" | "delegar" => Some(Slash::Agente(agente(arg, cat))),
        "libre" | "free" => Some(Slash::Libre),
        "limpiar" | "clear" | "nuevo" => Some(Slash::Limpiar),
        "contexto" | "context" | "ctx" => Some(Slash::Contexto),
        "registro" | "ledger" | "log" => Some(Slash::Registro),
        "pantalla" | "screen" | "ver" => Some(Slash::Pantalla),
        "captura" | "capture" | "rec" => Some(Slash::Captura),
        "coste" | "cost" | "gasto" => Some(Slash::Coste),
        "web" | "internet" | "buscar" => Some(Slash::Web),
        "raton" | "ratón" | "mouse" => Some(Slash::Raton),
        "microfono" | "micrófono" | "mic" => Some(Slash::Microfono),
        "voz" | "voice" | "hablar" => Some(Slash::Voz(arg)),
        "salir" | "quit" | "exit" | "q" => Some(Slash::Salir),
        "ejecutar" | "run" | "sh" | "!" => arg.map(Slash::Ejecutar),
        // «ver» no está libre: es alias de `/pantalla` desde antes que esto.
        "adjuntar" | "adjunta" | "attach" | "imagen" | "img" => arg.map(Slash::Adjuntar),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lo mínimo que sirve la aplicación cuando no hay fichero: los alias de
    /// serie, y nada más.
    fn de_serie() -> Catalogo {
        Catalogo {
            proveedores: Vec::new(),
            modelos: MODELOS
                .iter()
                .map(|(n, d)| (String::new(), (*n).to_string(), (*d).to_string()))
                .collect(),
            subagentes: MODELOS
                .iter()
                .map(|(n, d)| ((*n).to_string(), (*d).to_string()))
                .collect(),
            voces: VOCES
                .iter()
                .map(|(n, d)| ((*n).to_string(), (*d).to_string()))
                .collect(),
        }
    }

    /// El menú de quien no ha configurado ningún proveedor.
    fn menu(input: &str) -> Vec<Candidato> {
        candidatos(input, &de_serie())
    }

    /// Lo escrito, leído con el catálogo de quien no ha configurado nada.
    ///
    /// Casi ningún comando depende del catálogo; `/agente` sí, y para ese hay
    /// pruebas aparte con un fichero de verdad detrás.
    fn parse(input: &str) -> Option<Slash> {
        super::parse(input, &de_serie())
    }

    /// Dos servidores con sus modelos, como los declararía un fichero real.
    fn con_proveedores() -> Catalogo {
        Catalogo {
            proveedores: vec![
                ("claude".to_string(), "el proceso claude".to_string()),
                ("groq".to_string(), "openai/gpt-oss-120b".to_string()),
            ],
            modelos: vec![
                (
                    "claude".to_string(),
                    "haiku".to_string(),
                    "el más rápido".to_string(),
                ),
                (
                    "groq".to_string(),
                    "gpt-oss-120b".to_string(),
                    "equilibrado — 642 ms".to_string(),
                ),
            ],
            ..de_serie()
        }
    }

    /// El fallo que tenía esto: se podían declarar Groq, NVIDIA y Gemini en el
    /// fichero y el menú seguía ofreciendo solo los alias de claude. Configurar
    /// un proveedor no cambiaba nada de lo que se ve, así que la única forma de
    /// enterarse de que existía era saberlo ya.
    #[test]
    fn los_proveedores_configurados_salen_en_el_menu() {
        let m = candidatos("/modelo ", &con_proveedores());
        let etiquetas: Vec<&str> = m.iter().map(|c| c.etiqueta.as_str()).collect();
        assert!(etiquetas.contains(&"groq"), "{etiquetas:?}");
        assert!(etiquetas.contains(&"claude"), "{etiquetas:?}");
        // Y sus modelos con ellos, que es lo que se venía a elegir.
        assert!(etiquetas.contains(&"gpt-oss-120b"), "{etiquetas:?}");
    }

    /// Un modelo del fichero vale para delegar, no solo los tres de claude.
    ///
    /// Era el fallo de §9 un nivel más abajo: `/agente` ofrecía y entendía
    /// únicamente los alias de la suscripción, así que quien tenía veinte
    /// modelos configurados podía hablar con todos y delegar en tres.
    #[test]
    fn se_delega_en_un_modelo_configurado() {
        let cat = Catalogo {
            subagentes: vec![
                ("haiku".to_string(), "claude · el más rápido".to_string()),
                ("gpt-oss-120b".to_string(), "groq · equilibrado".to_string()),
            ],
            ..con_proveedores()
        };

        // En el menú del tercer nivel, que es el que fija el defecto.
        let m = candidatos("/agente modelo ", &cat);
        let etiquetas: Vec<&str> = m.iter().map(|c| c.etiqueta.as_str()).collect();
        assert!(etiquetas.contains(&"gpt-oss-120b"), "{etiquetas:?}");

        // Y al escribirlo: modelo por un lado, tarea por otro.
        assert_eq!(
            super::parse("/agente gpt-oss-120b mira los tests", &cat),
            Some(Slash::Agente(Agente::Lanzar {
                modelo: Some("gpt-oss-120b".into()),
                tarea: "mira los tests".into()
            }))
        );

        // Lo que no está en la lista es tarea, no modelo: si no, un encargo
        // que empiece por una palabra rara se quedaría sin su primera palabra.
        assert_eq!(
            super::parse("/agente inventado mira los tests", &cat),
            Some(Slash::Agente(Agente::Lanzar {
                modelo: None,
                tarea: "inventado mira los tests".into()
            }))
        );
    }

    /// Cada servidor con los suyos detrás, y no los servidores por un lado y
    /// unos modelos sueltos por el otro: en la captura que lo destapó había
    /// cuatro nombres de servidor seguidos de tres modelos de uno de ellos,
    /// sin nada que dijera de quién era cada cual.
    #[test]
    fn cada_servidor_va_con_sus_modelos_detras() {
        let m = candidatos("/modelo ", &con_proveedores());
        let etiquetas: Vec<&str> = m.iter().map(|c| c.etiqueta.as_str()).collect();
        assert_eq!(
            etiquetas,
            vec!["claude", "haiku", "groq", "gpt-oss-120b"],
            "el orden del fichero, con los modelos bajo su servidor"
        );
        // Y el servidor se distingue del modelo, que si no la lista son
        // cuatro nombres seguidos de dos clases distintas de cosa.
        assert!(m[0].cabecera, "«claude» es un servidor");
        assert!(!m[1].cabecera, "«haiku» es uno de sus modelos");
    }

    #[test]
    fn el_detalle_de_un_proveedor_es_su_modelo() {
        let m = candidatos("/modelo gro", &con_proveedores());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].completar, "/modelo groq");
        assert_eq!(m[0].detalle, "openai/gpt-oss-120b");
    }

    /// Un modelo declarado se elige por su nombre, sin pasar por el servidor.
    #[test]
    fn un_modelo_declarado_se_puede_pedir_por_su_nombre() {
        let m = candidatos("/modelo gpt-oss", &con_proveedores());
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].completar, "/modelo gpt-oss-120b");
        assert!(m[0].completo);
    }

    /// `/adjuntar` pide ruta, y `/ver` sigue siendo lo que era.
    ///
    /// Lo segundo importa más que lo primero: «ver» ya era alias de
    /// `/pantalla` desde antes, y robárselo habría cambiado en silencio lo que
    /// hace una tecla que la gente ya tiene en los dedos.
    #[test]
    fn adjuntar_pide_ruta_y_no_pisa_a_pantalla() {
        assert_eq!(
            parse("/adjuntar ~/x.png"),
            Some(Slash::Adjuntar("~/x.png".into()))
        );
        assert_eq!(
            parse("/img /tmp/una captura.png"),
            Some(Slash::Adjuntar("/tmp/una captura.png".into())),
            "la ruta llega entera, con espacios y todo"
        );
        // Sin ruta no hay nada que adjuntar: se manda al agente como pregunta,
        // que es lo que hace esta herramienta con lo que no reconoce.
        assert_eq!(parse("/adjuntar"), None);
        assert_eq!(parse("/ver"), Some(Slash::Pantalla));
    }

    #[test]
    fn reconoce_los_comandos() {
        assert_eq!(parse("/ayuda"), Some(Slash::Ayuda));
        assert_eq!(parse("/limpiar"), Some(Slash::Limpiar));
        assert_eq!(parse("/modelo"), Some(Slash::Modelo(None)));
        assert_eq!(
            parse("/modelo haiku"),
            Some(Slash::Modelo(Some("haiku".into())))
        );
        assert_eq!(
            parse("/modo ejecutar"),
            Some(Slash::Modo(Some("ejecutar".into())))
        );
    }

    /// `/plan` a secas no es un comando a medias: es «planifica lo que
    /// venimos hablando», que es como se pide de verdad.
    #[test]
    fn plan_recoge_el_encargo_y_tambien_va_solo() {
        assert_eq!(
            parse("/plan arreglar el parpadeo de la barra"),
            Some(Slash::Plan(Some("arreglar el parpadeo de la barra".into())))
        );
        assert_eq!(parse("/plan"), Some(Slash::Plan(None)));
        assert_eq!(
            parse("/planificar algo"),
            Some(Slash::Plan(Some("algo".into())))
        );
        // Y desde el menú se envía tal cual: el argumento es opcional.
        let p = menu("/plan").into_iter().next().unwrap();
        assert!(p.completo, "sin argumento planifica lo hablado");
    }

    #[test]
    fn acepta_los_nombres_en_ingles() {
        assert_eq!(
            parse("/model sonnet"),
            Some(Slash::Modelo(Some("sonnet".into())))
        );
        assert_eq!(parse("/clear"), Some(Slash::Limpiar));
    }

    #[test]
    fn ejecutar_conserva_el_comando_entero() {
        assert_eq!(
            parse("/ejecutar git log --oneline | head -5"),
            Some(Slash::Ejecutar("git log --oneline | head -5".into()))
        );
    }

    /// Lo que no es un comando conocido tiene que llegarle al agente como
    /// pregunta. Si no, escribir una ruta al empezar una frase haría algo
    /// inesperado.
    #[test]
    fn lo_desconocido_va_al_agente() {
        assert_eq!(parse("/usr/bin/algo no arranca"), None);
        assert_eq!(parse("por qué falla /etc/hosts"), None);
        assert_eq!(parse("/comandoquenoexiste"), None);
        assert_eq!(parse("hola"), None);
    }

    /// El menú no puede volver a llenarse de cosas que ya son una tecla: eso
    /// es lo que lo dejó en diecisiete entradas para siete filas visibles.
    #[test]
    fn el_menu_no_repite_lo_que_ya_tiene_tecla() {
        for (nombre, _, _) in COMANDOS {
            assert!(
                !OCULTOS.iter().any(|(o, _, _)| o == nombre),
                "«{nombre}» está en las dos tablas"
            );
        }
        // `/voz` no está en esta lista a propósito: Alt+V cubre solo encender
        // y apagar, y `/voz base` no tiene tecla ninguna. Tener atajo para una
        // parte no es tenerlo para el comando.
        for con_tecla in ["/ayuda", "/salir", "/captura", "/libre", "/pantalla"] {
            assert!(
                !COMANDOS.iter().any(|(n, _, _)| *n == con_tecla),
                "«{con_tecla}» ya tiene atajo: no debe ocupar sitio en el menú"
            );
        }
    }

    #[test]
    fn el_menu_se_filtra_al_escribir() {
        assert_eq!(menu("/").len(), COMANDOS.len());
        let m = menu("/mod");
        assert_eq!(m.len(), 2, "deberían quedar /modelo y /modo");
        assert!(m.iter().all(|c| c.etiqueta.starts_with("/mod")));
    }

    /// El segundo nivel: lo que hace que elegir un modelo sea señalarlo en
    /// una lista y no tener que recordar cómo se llama.
    #[test]
    fn tras_el_comando_ofrece_sus_argumentos() {
        let m = menu("/modelo ");
        assert_eq!(m.len(), MODELOS.len());
        assert!(m.iter().any(|c| c.etiqueta == "haiku"));
        // Al aceptarlo queda la línea entera y ya se puede enviar.
        let h = m.iter().find(|c| c.etiqueta == "haiku").unwrap();
        assert_eq!(h.completar, "/modelo haiku");
        assert!(h.completo);
    }

    /// El caso que pide el usuario: delegar eligiendo modelo.
    #[test]
    fn agente_reconoce_el_modelo_delante_de_la_tarea() {
        assert_eq!(
            parse("/agente sonnet lee src/voz.rs"),
            Some(Slash::Agente(Agente::Lanzar {
                modelo: Some("sonnet".into()),
                tarea: "lee src/voz.rs".into(),
            }))
        );
    }

    /// Sin modelo delante, la primera palabra es parte de la tarea. Un encargo
    /// que empiece por una palabra cualquiera no puede perderla por el camino.
    #[test]
    fn agente_sin_modelo_no_se_come_la_primera_palabra() {
        assert_eq!(
            parse("/agente lee src/voz.rs"),
            Some(Slash::Agente(Agente::Lanzar {
                modelo: None,
                tarea: "lee src/voz.rs".into(),
            }))
        );
    }

    /// `/agente sonnet` a secas no es un encargo vacío: es una tarea rara, y
    /// lanzar un subagente sin tarea sería tirar una llamada al modelo.
    #[test]
    fn agente_con_solo_un_modelo_no_deja_el_encargo_vacio() {
        assert_eq!(
            parse("/agente sonnet"),
            Some(Slash::Agente(Agente::Lanzar {
                modelo: None,
                tarea: "sonnet".into(),
            }))
        );
    }

    #[test]
    fn agente_fija_el_modelo_y_a_secas_se_explica() {
        assert_eq!(
            parse("/agente modelo haiku"),
            Some(Slash::Agente(Agente::Fijar("haiku".into())))
        );
        assert_eq!(parse("/agente"), Some(Slash::Agente(Agente::Como)));
    }

    /// Detrás de `/agente ` va la tarea, y la tarea es texto libre.
    ///
    /// La lista de agentes estuvo aquí y se fue a la barra de delegar, que es
    /// donde se elige y se escribe el encargo de una vez. Dejarla también aquí
    /// serían dos sitios para la misma decisión — y con dieciocho modelos
    /// configurados, el menú no era el bueno.
    #[test]
    fn agente_no_repite_la_lista_de_la_barra() {
        let m = menu("/agente ");
        assert!(
            !m.iter().any(|c| c.etiqueta == "haiku"),
            "los agentes se eligen en la barra"
        );
        assert!(
            m.iter().any(|c| c.etiqueta == "modelo"),
            "el verbo que fija el defecto sí, que sigue siendo un comando"
        );
        // Con algo escrito ya es la tarea: nada que ofrecer.
        assert!(menu("/agente mira los tests").is_empty());
    }

    /// El fallo de la captura: con `/agente modelo ` escrito, el menú volvía a
    /// ofrecer la palabra «modelo» y no había forma de llegar a la lista.
    /// Elegir esa fila dejaba lo mismo escrito, en bucle.
    #[test]
    fn agente_modelo_ofrece_con_cual_van() {
        let m = menu("/agente modelo ");
        let etiquetas: Vec<&str> = m.iter().map(|c| c.etiqueta.as_str()).collect();
        assert!(etiquetas.contains(&"haiku"), "{etiquetas:?}");
        assert!(etiquetas.contains(&"sonnet"), "{etiquetas:?}");
        assert!(
            !etiquetas.contains(&"modelo"),
            "el verbo ya está escrito: {etiquetas:?}"
        );
        let h = m.iter().find(|c| c.etiqueta == "haiku").unwrap();
        assert_eq!(h.completar, "/agente modelo haiku");
        assert!(h.completo);
        // Y se sigue filtrando al escribir.
        assert_eq!(menu("/agente modelo hai").len(), 1);
    }

    /// El verbo no cierra la orden: falta decir cuál.
    ///
    /// Importa porque el menú lo ofrece. Con `completo`, elegirlo y pulsar
    /// Enter lanzaba un subagente con la tarea «modelo» — una llamada al
    /// modelo pagada por señalar una fila.
    #[test]
    fn el_verbo_modelo_no_se_envia_al_elegirlo() {
        let v = menu("/agente ")
            .into_iter()
            .find(|c| c.etiqueta == "modelo")
            .expect("el verbo está en el menú");
        assert!(!v.completo);
        assert_eq!(v.completar, "/agente modelo");
        // Y si aun así se envía, no encarga nada: contesta con cuál van.
        assert_eq!(parse("/agente modelo"), Some(Slash::Agente(Agente::Como)));
    }

    /// El menú anunciaba `[instalar <de dónde>]` y no sabía completarlo.
    #[test]
    fn skills_ofrece_instalar() {
        let m = menu("/skills ");
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].completar, "/skills instalar");
        assert!(!m[0].completo, "falta decir de dónde");
    }

    #[test]
    fn los_argumentos_tambien_se_filtran() {
        let m = menu("/modelo so");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].completar, "/modelo sonnet");

        let m = menu("/modo eje");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].completar, "/modo ejecutar");
    }

    /// Un comando que aún necesita argumentos no debe enviarse al elegirlo:
    /// Lo que distingue a un comando entero de uno a medias es si le falta
    /// algo **obligatorio**, no si admite argumentos.
    #[test]
    fn distingue_lo_que_ya_esta_completo() {
        let limpiar = menu("/limpiar").into_iter().next().unwrap();
        assert!(limpiar.completo);
        assert_eq!(limpiar.completar, "/limpiar");

        // Con argumento opcional: se envía, y aun así deja el espacio puesto
        // por si quieres seguir escribiendo en vez de enviar.
        let modelo = menu("/modelo").into_iter().next().unwrap();
        assert!(modelo.completo);
        assert_eq!(modelo.completar, "/modelo ");

        let ejecutar = menu("/ejecutar").into_iter().next().unwrap();
        assert!(!ejecutar.completo, "sin comando no hay nada que ejecutar");
    }

    /// Quien no recuerda el nombre exacto suele acordarse de una palabra de
    /// la descripción. Buscar dentro cuando nada empieza así lo cubre.
    #[test]
    fn si_nada_empieza_asi_busca_dentro() {
        let m = menu("/gasto");
        assert!(
            m.iter().any(|c| c.etiqueta.starts_with("/coste")),
            "«gasto» debería encontrar /coste"
        );
    }

    #[test]
    fn sin_barra_no_hay_menu() {
        assert!(menu("hola").is_empty());
        assert!(menu("").is_empty());
    }

    #[test]
    fn los_comandos_de_texto_libre_no_ofrecen_nada() {
        // `/ejecutar` recibe cualquier cosa: no hay lista que enseñar.
        assert!(menu("/ejecutar git st").is_empty());
    }

    /// Un comando en el menú que `parse` no reconozca sería una promesa rota:
    /// aparece en la lista y al pulsarlo no hace nada. Y uno oculto que no se
    /// reconozca es peor: no hay ni forma de descubrir que se rompió.
    #[test]
    fn todo_lo_que_se_anuncia_funciona() {
        for (nombre, args, _) in COMANDOS.iter().chain(OCULTOS) {
            let linea = if args.is_empty() {
                nombre.to_string()
            } else {
                format!("{nombre} algo")
            };
            assert!(
                parse(&linea).is_some(),
                "«{linea}» sale en el menú y no se reconoce"
            );
        }
    }
}
