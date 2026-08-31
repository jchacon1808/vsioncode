//! Editar ficheros, no solo proponer comandos.
//!
//! Hasta aquí, para cambiar una línea el agente proponía un `sed` retorcido o
//! un heredoc que reescribía el fichero entero. Funciona y es incómodo de
//! revisar: no se ve qué cambia, se ve un comando.
//!
//! Un bloque de edición dice qué texto se va y qué texto entra:
//!
//! ````text
//! ```edit: src/app.rs
//! <<<<<<< BUSCA
//! let x = 1;
//! =======
//! let x = 2;
//! >>>>>>> CAMBIA
//! ```
//! ````
//!
//! ## Por qué este formato y no un diff unificado
//!
//! Aider midió que el diff unificado acierta más **con modelos grandes** — del
//! 26 % al 59 % con GPT-4 Turbo. Pero exige una sintaxis estricta
//! (`@@ -l,s +l,s @@`, con números de línea y recuentos correctos) que los
//! modelos pequeños rompen constantemente.
//!
//! Y aquí el objetivo declarado son los modelos gratuitos. Así que el formato
//! que produce el modelo es lo más simple posible —dos trozos de texto, sin
//! numerar nada— y **la tolerancia vive en quien aplica**, no en el prompt.
//!
//! ## La cascada
//!
//! Se intenta encajar el texto buscado de menos a más permisivo, y en cuanto
//! uno funciona se para:
//!
//! 1. **Exacto.** Lo normal cuando el modelo ha leído el fichero.
//! 2. **Ignorando el sangrado.** El fallo más común: el modelo recuerda el
//!    código pero no cuántos espacios llevaba.
//! 3. **Por anclas.** Casan la primera y la última línea; lo de en medio se
//!    da por bueno. Para bloques largos donde algo del centro se transcribió
//!    mal.
//!
//! Cada escalón es más permisivo, así que **cada uno exige que la coincidencia
//! sea única**. Si aparece dos veces, se rechaza en vez de elegir: cambiar el
//! sitio equivocado de un fichero es peor que no cambiar nada.

use std::path::{Path, PathBuf};

/// Un cambio propuesto sobre un fichero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edicion {
    pub ruta: PathBuf,
    /// Lo que hay que encontrar. Vacío significa crear el fichero.
    pub busca: String,
    /// Lo que hay que poner en su lugar.
    pub cambia: String,
}

/// Cómo se consiguió encajar el cambio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Como {
    Exacto,
    SinSangrado,
    PorAnclas,
    /// El fichero no existía y se crea.
    Nuevo,
}

impl Como {
    pub fn label(self) -> &'static str {
        match self {
            Como::Exacto => "exacto",
            Como::SinSangrado => "ajustando el sangrado",
            Como::PorAnclas => "por anclas",
            Como::Nuevo => "fichero nuevo",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fallo {
    NoEncontrado,
    /// Aparece más de una vez y no se puede saber cuál.
    Ambiguo(usize),
    NoSePudoLeer(String),
}

impl std::fmt::Display for Fallo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fallo::NoEncontrado => write!(f, "no encontré ese texto en el fichero"),
            Fallo::Ambiguo(n) => write!(
                f,
                "ese texto aparece {n} veces; hace falta más contexto para saber cuál"
            ),
            Fallo::NoSePudoLeer(e) => write!(f, "no pude leerlo: {e}"),
        }
    }
}

/// El resultado de preparar un cambio, sin haberlo escrito todavía.
#[derive(Debug, Clone)]
pub struct Preparado {
    pub ruta: PathBuf,
    pub como: Como,
    /// El contenido completo que quedaría.
    pub nuevo: String,
    /// El de ahora, para poder enseñar la diferencia.
    pub viejo: String,
}

/// A qué fichero se refiere de verdad una ruta.
///
/// Resolver el enlace es lo correcto —renombrar encima lo convertiría en un
/// fichero normal y el cambio no llegaría al destino—, pero **no resolverlo
/// tiene que ser una decisión, no un descuido**. Tratar todos los errores por
/// igual con un `unwrap_or_else` hacía que un enlace que no se puede resolver
/// por permisos acabara sustituido por un fichero normal, con el destino real
/// intacto y diciéndole al usuario que el cambio se escribió.
///
/// Así que se distingue:
///
/// - No existe nada ahí: es un fichero nuevo. Se trabaja donde se dijo.
/// - Es un enlace y su destino no existe: se sigue igualmente y se crea el
///   destino. Es lo que espera quien tiene `config.toml -> ../compartido/…`
///   con el otro extremo todavía sin traer; borrarle el enlace, no.
/// - Cualquier otro fallo —permisos, un tramo que no deja pasar—: se para. Si
///   no se puede saber a qué fichero apunta, no se escribe a ciegas.
fn resolver(ruta: &Path) -> Result<PathBuf, Fallo> {
    match std::fs::canonicalize(ruta) {
        Ok(r) => Ok(r),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match std::fs::read_link(ruta) {
            // Enlace roto: su destino es donde hay que escribir.
            Ok(apunta) if apunta.is_absolute() => Ok(apunta),
            Ok(apunta) => Ok(ruta.parent().unwrap_or(Path::new(".")).join(apunta)),
            // No es un enlace: sencillamente no existe todavía.
            Err(_) => Ok(ruta.to_path_buf()),
        },
        Err(e) => Err(Fallo::NoSePudoLeer(format!(
            "no pude saber a qué fichero apunta {}: {e}",
            ruta.display()
        ))),
    }
}

/// Prepara un cambio: lo calcula y lo deja listo, **sin escribir nada**.
///
/// Separar el cálculo de la escritura no es limpieza: es lo que permite
/// enseñar exactamente qué va a pasar y esperar a que el humano diga que sí,
/// igual que con los comandos.
pub fn preparar(e: &Edicion) -> Result<Preparado, Fallo> {
    // El enlace simbólico se resuelve **aquí**, no al escribir.
    //
    // Escribir a través del enlace es lo correcto —renombrar encima lo
    // convertiría en un fichero normal y el cambio no llegaría al destino—,
    // pero si eso se decide en `escribir`, la ruta que se enseña y la ruta
    // donde aterrizan los bytes dejan de ser la misma: se lee «Cambio en
    // notas.txt», se acepta, y se acaba escribiendo en el otro extremo del
    // enlace. Resolverlo antes de enseñar nada hace que lo que se aprueba sea
    // literalmente lo que se hace. Si la ruta no se puede resolver —no existe
    // todavía, o el enlace está roto— se trabaja donde se dijo.
    let ruta = resolver(&e.ruta)?;
    let existe = ruta.exists();

    if e.busca.trim().is_empty() {
        // Sin texto que buscar, es un fichero nuevo.
        return Ok(Preparado {
            ruta: ruta.clone(),
            como: Como::Nuevo,
            viejo: if existe {
                std::fs::read_to_string(&ruta).unwrap_or_default()
            } else {
                String::new()
            },
            nuevo: e.cambia.clone(),
        });
    }

    let viejo =
        std::fs::read_to_string(&ruta).map_err(|err| Fallo::NoSePudoLeer(err.to_string()))?;

    let (nuevo, como) = aplicar(&viejo, &e.busca, &e.cambia)?;
    Ok(Preparado {
        ruta,
        como,
        nuevo,
        viejo,
    })
}

/// Un temporal que se recoge solo si la maniobra no llega a término.
///
/// Sin esto, un `rename` que falla —el directorio no deja escribir, el destino
/// está en otro sistema de ficheros— deja un `.vsion<pid>` tirado al lado del
/// fichero del usuario, y nadie lo barre nunca.
struct Temporal {
    ruta: PathBuf,
    logrado: bool,
}

impl Temporal {
    /// Lo pone en su sitio definitivo. A partir de aquí ya no hay nada que
    /// recoger: el `rename` se llevó el fichero.
    fn renombrar_sobre(mut self, destino: &Path) -> std::io::Result<()> {
        std::fs::rename(&self.ruta, destino)?;
        self.logrado = true;
        Ok(())
    }
}

impl Drop for Temporal {
    fn drop(&mut self) {
        if !self.logrado {
            let _ = std::fs::remove_file(&self.ruta);
        }
    }
}

/// Escribe el cambio ya preparado.
///
/// Se escribe a un temporal y se renombra encima: si algo falla a mitad, el
/// fichero original sigue entero en vez de quedarse truncado.
pub fn escribir(p: &Preparado) -> std::io::Result<()> {
    // La ruta ya viene resuelta de `preparar`: aquí se escribe exactamente
    // donde dijo lo que se enseñó, sin volver a interpretar nada.
    let destino = &p.ruta;

    if let Some(d) = destino.parent() {
        std::fs::create_dir_all(d)?;
    }

    let modo = modo_de(destino);
    let temporal = crear_con_modo(&ruta_temporal(destino), modo, &p.nuevo)?;
    temporal.renombrar_sobre(destino)
}

/// Dónde se escribe mientras el cambio no está confirmado.
///
/// El nombre lleva un número distinto en cada intento, y no solo el PID, por
/// una razón concreta: como el temporal se crea con `create_new`, un nombre
/// estable durante toda la sesión convierte cualquier resto —un disco que se
/// llenó, un `abort`, un PID reciclado dentro de un contenedor— en un fichero
/// que ya no se puede volver a editar hasta reiniciar, y el error que se ve es
/// un «File exists» que no dice qué estorba. Con un nombre por intento se
/// conserva lo bueno de `create_new` —no pisar nada ajeno— sin el atasco.
fn ruta_temporal(destino: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static INTENTO: AtomicU64 = AtomicU64::new(0);

    destino.with_extension(format!(
        "{}.vsion{}-{}",
        destino
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("tmp"),
        std::process::id(),
        INTENTO.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Los permisos que ya tiene el fichero, si es que existe.
fn modo_de(ruta: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Solo los bits de permiso: `mode()` devuelve el `st_mode` entero, con
        // el tipo de fichero dentro (0o100755 y no 0o755). Hoy Linux lo
        // enmascara al aplicarlo, pero un valor que miente sobre lo que es
        // acaba pintándose o comparándose en algún sitio.
        std::fs::metadata(ruta)
            .ok()
            .map(|m| m.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = ruta;
        None
    }
}

/// Crea el temporal con sus permisos definitivos y le vuelca el contenido.
///
/// Devuelve la guarda **ya armada**, y por eso devuelve algo en vez de no
/// devolver nada: en cuanto `open` tiene éxito el fichero existe, así que un
/// `write_all` que falle a mitad —un disco lleno es el caso realista— dejaría
/// un temporal tirado si la guarda se armara al volver de aquí. Se arma en el
/// único punto donde no hay hueco: entre crear y escribir.
///
/// `create_new` a propósito: si en esa ruta hay algo, se falla en vez de
/// escribir encima. Lo que haya ahí no es nuestro y no se pisa a ciegas — y
/// como el nombre lleva un número por intento, chocar es casi imposible.
///
/// Los permisos se ponen **dos veces**, y las dos hacen falta:
///
/// - Al crear, con `mode()`, porque `open` los aplica antes de que el fichero
///   exista y así el contenido nunca está en disco bajo permisos más laxos de
///   los que le tocan. Esa es la ventana que hay que cerrar.
/// - Y otra vez al terminar, con `set_permissions`, porque `open` no da el
///   modo pedido sino `modo & ~umask`: con la umask 022 de casi cualquier
///   distribución un fichero 664 salía 644, y con una umask endurecida el
///   `deploy.sh` volvía a perder el bit de ejecución — el fallo original,
///   entrando por la puerta de atrás. La máscara solo puede quitar permisos,
///   así que ampliarlos después es seguro: en ningún instante el fichero está
///   más abierto de lo que le toca.
fn crear_con_modo(tmp: &Path, modo: Option<u32>, contenido: &str) -> std::io::Result<Temporal> {
    use std::io::Write as _;

    let mut opciones = std::fs::OpenOptions::new();
    opciones.write(true).create_new(true);
    #[cfg(unix)]
    if let Some(m) = modo {
        use std::os::unix::fs::OpenOptionsExt;
        opciones.mode(m);
    }
    #[cfg(not(unix))]
    let _ = modo;

    let mut fichero = opciones.open(tmp)?;
    let temporal = Temporal {
        ruta: tmp.to_path_buf(),
        logrado: false,
    };

    fichero.write_all(contenido.as_bytes())?;

    #[cfg(unix)]
    if let Some(m) = modo {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(m))?;
    }
    Ok(temporal)
}

/// La cascada de estrategias.
fn aplicar(texto: &str, busca: &str, cambia: &str) -> Result<(String, Como), Fallo> {
    if let Some(r) = exacto(texto, busca, cambia)? {
        return Ok((r, Como::Exacto));
    }
    if let Some(r) = sin_sangrado(texto, busca, cambia)? {
        return Ok((r, Como::SinSangrado));
    }
    if let Some(r) = por_anclas(texto, busca, cambia)? {
        return Ok((r, Como::PorAnclas));
    }
    Err(Fallo::NoEncontrado)
}

/// Coincidencia literal. Lo normal cuando el modelo ha leído el fichero.
fn exacto(texto: &str, busca: &str, cambia: &str) -> Result<Option<String>, Fallo> {
    let veces = texto.matches(busca).count();
    match veces {
        0 => Ok(None),
        1 => Ok(Some(texto.replacen(busca, cambia, 1))),
        n => Err(Fallo::Ambiguo(n)),
    }
}

/// Ignora los espacios del principio de cada línea.
///
/// Es el fallo más común de los modelos pequeños: recuerdan el código pero no
/// con cuánto sangrado estaba. Al escribir se conserva el del fichero, no el
/// que propuso el modelo, para no descolocar el bloque.
fn sin_sangrado(texto: &str, busca: &str, cambia: &str) -> Result<Option<String>, Fallo> {
    let lineas: Vec<&str> = texto.lines().collect();
    let buscadas: Vec<&str> = busca.lines().collect();
    if buscadas.is_empty() || buscadas.len() > lineas.len() {
        return Ok(None);
    }

    let mut encontradas = Vec::new();
    for i in 0..=lineas.len() - buscadas.len() {
        let casa = buscadas
            .iter()
            .zip(&lineas[i..i + buscadas.len()])
            .all(|(a, b)| a.trim() == b.trim());
        if casa {
            encontradas.push(i);
        }
    }

    match encontradas.len() {
        0 => Ok(None),
        1 => {
            let i = encontradas[0];
            // El sangrado que manda es el del fichero.
            let sangrado = sangrado_de(lineas[i]);
            let sustituto = resangrar(cambia, &sangrado);
            let mut fuera: Vec<String> = lineas[..i].iter().map(|s| s.to_string()).collect();
            fuera.extend(sustituto.lines().map(|s| s.to_string()));
            fuera.extend(lineas[i + buscadas.len()..].iter().map(|s| s.to_string()));
            Ok(Some(unir(fuera, texto)))
        }
        n => Err(Fallo::Ambiguo(n)),
    }
}

/// Casa la primera y la última línea, y da por bueno lo de en medio.
///
/// Para bloques largos donde el modelo transcribió mal algo del centro. Es la
/// estrategia más permisiva y por eso la última: solo se acepta si el par de
/// anclas aparece **una sola vez**.
fn por_anclas(texto: &str, busca: &str, cambia: &str) -> Result<Option<String>, Fallo> {
    let buscadas: Vec<&str> = busca.lines().filter(|l| !l.trim().is_empty()).collect();
    // Con menos de tres líneas no hay «centro» que perdonar: si no encajó
    // antes, aquí tampoco debería pasar nada por la puerta de atrás.
    if buscadas.len() < 3 {
        return Ok(None);
    }
    let (primera, ultima) = (buscadas[0].trim(), buscadas[buscadas.len() - 1].trim());
    let lineas: Vec<&str> = texto.lines().collect();

    let mut encontradas = Vec::new();
    for (i, l) in lineas.iter().enumerate() {
        if l.trim() != primera {
            continue;
        }
        // Se busca el cierre en una ventana razonable alrededor del tamaño
        // esperado; si no, un bloque cualquiera podría emparejarse con un
        // cierre lejano que no tiene nada que ver.
        let tope = (i + buscadas.len() * 2 + 4).min(lineas.len());
        // El índice absoluto hace falta para recortar después, así que se
        // itera con `enumerate` sobre la ventana en vez de indexar a mano.
        if let Some((desp, _)) = lineas[i + 1..tope]
            .iter()
            .enumerate()
            .find(|(_, l)| l.trim() == ultima)
        {
            encontradas.push((i, i + 1 + desp));
        }
    }

    match encontradas.len() {
        0 => Ok(None),
        1 => {
            let (i, j) = encontradas[0];
            let sangrado = sangrado_de(lineas[i]);
            let sustituto = resangrar(cambia, &sangrado);
            let mut fuera: Vec<String> = lineas[..i].iter().map(|s| s.to_string()).collect();
            fuera.extend(sustituto.lines().map(|s| s.to_string()));
            fuera.extend(lineas[j + 1..].iter().map(|s| s.to_string()));
            Ok(Some(unir(fuera, texto)))
        }
        n => Err(Fallo::Ambiguo(n)),
    }
}

fn sangrado_de(linea: &str) -> String {
    linea
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Pone el sangrado del fichero al texto nuevo, conservando el relativo.
fn resangrar(texto: &str, sangrado: &str) -> String {
    let base = texto
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| sangrado_de(l).len())
        .min()
        .unwrap_or(0);
    texto
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("{sangrado}{}", &l[base.min(l.len())..])
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Une las líneas conservando si el original acababa en salto.
fn unir(lineas: Vec<String>, original: &str) -> String {
    let mut s = lineas.join("\n");
    if original.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Saca los bloques de edición de una respuesta.
///
/// Los marcadores son los de un conflicto de git a propósito: cualquier modelo
/// los ha visto miles de veces en su entrenamiento, así que los reproduce sin
/// que haya que explicárselos.
pub fn extraer(md: &str, raiz: &Path) -> Vec<Edicion> {
    const BUSCA: &str = "<<<<<<< BUSCA";
    const SEPARA: &str = "=======";
    const CAMBIA: &str = ">>>>>>> CAMBIA";

    let mut fuera = Vec::new();
    let mut ruta: Option<String> = None;
    let mut cuerpo = String::new();

    for linea in md.lines() {
        let t = linea.trim_start();
        if t.starts_with("```") {
            if let Some(r) = ruta.take() {
                if let Some(e) = trocear(&cuerpo, &r, raiz, BUSCA, SEPARA, CAMBIA) {
                    fuera.push(e);
                }
                cuerpo.clear();
                continue;
            }
            // Tolerante con cómo escribe el modelo la etiqueta: `edit:`,
            // `edit `, `editar:`, con o sin espacio. Exigir una forma exacta
            // hacía que el bloque se pintara como si fuera un comando y el
            // cambio no se aplicara — mientras el modelo decía «listo, ya está
            // cambiado». Un fallo silencioso que además miente.
            let etiqueta = t.trim_start_matches('`').trim();
            let bajo = etiqueta.to_ascii_lowercase();
            for prefijo in [
                "edit:",
                "edit ",
                "editar:",
                "editar ",
                "edición:",
                "edicion:",
            ] {
                if let Some(stripped) = bajo.strip_prefix(prefijo) {
                    let r = etiqueta[etiqueta.len() - stripped.len()..].trim();
                    if !r.is_empty() {
                        ruta = Some(r.to_string());
                        cuerpo.clear();
                    }
                    break;
                }
            }
            continue;
        }
        if ruta.is_some() {
            cuerpo.push_str(linea);
            cuerpo.push('\n');
        }
    }
    fuera
}

fn trocear(
    cuerpo: &str,
    ruta: &str,
    raiz: &Path,
    busca_m: &str,
    separa_m: &str,
    cambia_m: &str,
) -> Option<Edicion> {
    // El modelo escribe la ruta mirando la `cwd` que le damos, pero también
    // copia el estilo de lo que ha visto un millón de veces, que es POSIX. Sin
    // traducir, un `/c/Users/...` no cuenta como absoluto en Windows y se
    // colgaría de la raíz del proyecto: se editaría un fichero que no es, o
    // ninguno, y sin ruido.
    let nativa = crate::rutas::a_windows(ruta);
    let ruta = nativa.as_ref();
    let ruta_completa = if Path::new(ruta).is_absolute() {
        PathBuf::from(ruta)
    } else {
        raiz.join(ruta)
    };

    // Sin marcadores, el bloque entero es el contenido de un fichero nuevo.
    if !cuerpo.contains(busca_m) {
        let c = cuerpo.trim_end();
        if c.is_empty() {
            return None;
        }
        return Some(Edicion {
            ruta: ruta_completa,
            busca: String::new(),
            cambia: format!("{c}\n"),
        });
    }

    let tras_busca = cuerpo.split_once(busca_m)?.1;
    let (busca, resto) = tras_busca.split_once(separa_m)?;
    let (cambia, _) = resto.split_once(cambia_m)?;

    Some(Edicion {
        ruta: ruta_completa,
        busca: busca.trim_start_matches('\n').trim_end().to_string(),
        cambia: cambia.trim_start_matches('\n').trim_end().to_string(),
    })
}

/// ¿Parece que quiso editar aunque no se haya podido leer?
///
/// El peor fallo posible de este módulo no es no aplicar un cambio: es no
/// aplicarlo mientras el modelo dice «listo, ya está cambiado». Se detecta
/// que hay marcadores de edición sueltos para poder avisar en vez de callar.
pub fn parece_edicion_fallida(md: &str) -> bool {
    md.contains("<<<<<<< BUSCA") && md.contains(">>>>>>> CAMBIA")
}

/// El tramo que de verdad cambia: en qué línea empieza, cuántas se van y
/// cuántas entran.
///
/// No es un diff de verdad: se recorta lo que coincide por los dos extremos y
/// lo de en medio se da por cambiado entero. Un algoritmo de diferencias real
/// sería más bonito y más código, y para un bloque de edición —que ya viene
/// acotado a un trozo— acertaría en lo mismo.
///
/// Lo comparten el recuento y el diff porque miden exactamente lo mismo: si
/// cada uno lo calculara a su manera, un día dirían cosas distintas del mismo
/// cambio.
fn tramo(p: &Preparado) -> (usize, usize, usize) {
    let viejas: Vec<&str> = p.viejo.lines().collect();
    let nuevas: Vec<&str> = p.nuevo.lines().collect();
    let comun_inicio = viejas
        .iter()
        .zip(&nuevas)
        .take_while(|(a, b)| a == b)
        .count();
    let comun_final = viejas[comun_inicio..]
        .iter()
        .rev()
        .zip(nuevas[comun_inicio..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let quitadas = viejas.len().saturating_sub(comun_inicio + comun_final);
    let puestas = nuevas.len().saturating_sub(comun_inicio + comun_final);
    (comun_inicio, quitadas, puestas)
}

/// Las líneas que cambian, para enseñarlas.
pub fn resumen(p: &Preparado) -> (usize, usize) {
    let (_, quitadas, puestas) = tramo(p);
    (quitadas, puestas)
}

/// Cuántas líneas de alrededor se enseñan a cada lado del cambio.
///
/// Dos. Con una no se sabe dónde cae; con cinco, un cambio de una línea ocupa
/// once y el panel deja de ser una conversación.
pub const CONTEXTO: usize = 2;

/// Tope de líneas por lado. Un fichero reescrito entero no puede llevarse la
/// conversación por delante: lo que pasa de aquí se cuenta, no se pinta.
pub const MAX_DIFF: usize = 24;

/// El cambio línea a línea y con su número, para verlo.
///
/// Existe porque «−7 +3 líneas» dice cuánto cambia y no dice qué, y un cambio
/// que no se ve no se revisa: se acepta. En modo libre pesa el doble —ahí ya
/// no hay una pulsación por medio—, y entonces esto es lo único que queda
/// entre un fichero tocado y enterarse.
///
/// Sale como un bloque ```` ```diff ````: es lo que el panel sabe pintar en
/// rojo y verde, y lo que un `/copiar` deja pegable en cualquier sitio.
/// Cada línea lleva delante su signo y su número **en su propio fichero** —las
/// que se van, el de antes; las que entran, el de después—, que es el número
/// que sirve para ir a mirarlo.
pub fn diff(p: &Preparado) -> String {
    let (desde, quitadas, puestas) = tramo(p);
    if quitadas == 0 && puestas == 0 {
        return String::new();
    }
    let viejas: Vec<&str> = p.viejo.lines().collect();
    let nuevas: Vec<&str> = p.nuevo.lines().collect();

    let mut s = String::from("```diff\n");
    let antes = desde.saturating_sub(CONTEXTO);
    for (i, l) in viejas[antes..desde].iter().enumerate() {
        s.push_str(&format!(" {:>4} {l}\n", antes + i + 1));
    }
    for (i, l) in viejas[desde..desde + quitadas]
        .iter()
        .take(MAX_DIFF)
        .enumerate()
    {
        s.push_str(&format!("-{:>4} {l}\n", desde + i + 1));
    }
    if quitadas > MAX_DIFF {
        s.push_str(&format!(
            "-     … y {} líneas más que se van\n",
            quitadas - MAX_DIFF
        ));
    }
    for (i, l) in nuevas[desde..desde + puestas]
        .iter()
        .take(MAX_DIFF)
        .enumerate()
    {
        s.push_str(&format!("+{:>4} {l}\n", desde + i + 1));
    }
    if puestas > MAX_DIFF {
        s.push_str(&format!(
            "+     … y {} líneas más que entran\n",
            puestas - MAX_DIFF
        ));
    }
    let sigue = desde + quitadas;
    for (i, l) in viejas[sigue..].iter().take(CONTEXTO).enumerate() {
        s.push_str(&format!(" {:>4} {l}\n", sigue + i + 1));
    }
    s.push_str("```");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODIGO: &str = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";

    #[test]
    fn el_caso_normal_es_exacto() {
        let (r, como) = aplicar(CODIGO, "    let x = 1;", "    let x = 2;").unwrap();
        assert_eq!(como, Como::Exacto);
        assert!(r.contains("let x = 2;"));
        assert!(r.contains("println!"), "no se lleva por delante lo demás");
    }

    /// La coincidencia exacta es por subcadena, no por línea: buscar
    /// `let x = 1;` lo encuentra dentro de la línea sangrada. Es lo correcto
    /// —permite cambiar un trozo de línea— y por eso el escalón del sangrado
    /// solo entra en juego con bloques de varias líneas.
    #[test]
    fn una_subcadena_la_encuentra_el_escalon_exacto() {
        let (r, como) = aplicar(CODIGO, "let x = 1;", "let x = 42;").unwrap();
        assert_eq!(como, Como::Exacto);
        assert!(
            r.contains("    let x = 42;"),
            "sin descolocar la línea: {r:?}"
        );
    }

    /// El fallo más común de los modelos pequeños: recuerdan el código pero no
    /// con cuánto sangrado llevaba cada línea.
    #[test]
    fn perdona_el_sangrado() {
        // El modelo lo escribe sin sangrar, el fichero lo tiene con cuatro
        // espacios. Como bloque de dos líneas, ya no es una subcadena.
        let busca = "let x = 1;
println!(\"{x}\");";
        let cambia = "let x = 42;
println!(\"vale {x}\");";
        let (r, como) = aplicar(CODIGO, busca, cambia).unwrap();
        assert_eq!(como, Como::SinSangrado);
        assert!(
            r.contains("    let x = 42;") && r.contains("    println!(\"vale {x}\");"),
            "conserva el sangrado del fichero, no el del modelo: {r:?}"
        );
    }

    #[test]
    fn perdona_el_centro_de_un_bloque_largo() {
        let texto = "inicio\nfn a() {\n    uno();\n    dos();\n    tres();\n}\nfin\n";
        // El modelo transcribió mal la línea de en medio.
        let busca = "fn a() {\n    uno();\n    DOS_MAL();\n    tres();\n}";
        let (r, como) = aplicar(texto, busca, "fn a() {\n    nuevo();\n}").unwrap();
        assert_eq!(como, Como::PorAnclas);
        assert!(r.contains("nuevo()"));
        assert!(r.starts_with("inicio\n") && r.trim_end().ends_with("fin"));
    }

    /// Cambiar el sitio equivocado de un fichero es peor que no cambiar nada.
    #[test]
    fn si_hay_varias_coincidencias_no_elige() {
        let texto = "let x = 1;\nalgo();\nlet x = 1;\n";
        assert_eq!(
            aplicar(texto, "let x = 1;", "let x = 2;"),
            Err(Fallo::Ambiguo(2))
        );
    }

    #[test]
    fn si_no_esta_lo_dice() {
        assert_eq!(
            aplicar(CODIGO, "no existe esta línea", "x"),
            Err(Fallo::NoEncontrado)
        );
    }

    /// Las anclas son la estrategia más permisiva; con dos líneas no hay
    /// centro que perdonar y no debe colarse nada por ahí.
    #[test]
    fn las_anclas_no_se_aplican_a_bloques_cortos() {
        let texto = "uno\ndos\ntres\n";
        assert_eq!(por_anclas(texto, "uno\ntres", "x").unwrap(), None);
    }

    #[test]
    fn conserva_el_salto_final_o_su_ausencia() {
        let con = "a\nb\n";
        let sin = "a\nb";
        assert!(aplicar(con, "b", "c").unwrap().0.ends_with('\n'));
        assert!(!aplicar(sin, "b", "c").unwrap().0.ends_with('\n'));
    }

    #[test]
    fn saca_un_bloque_de_edicion() {
        let md = "Cambio esto:\n\n```edit: src/app.rs\n<<<<<<< BUSCA\nlet x = 1;\n=======\nlet x = 2;\n>>>>>>> CAMBIA\n```\n";
        let e = extraer(md, Path::new("/proyecto"));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].ruta, PathBuf::from("/proyecto/src/app.rs"));
        assert_eq!(e[0].busca, "let x = 1;");
        assert_eq!(e[0].cambia, "let x = 2;");
    }

    #[test]
    fn un_bloque_sin_marcadores_es_un_fichero_nuevo() {
        let md = "```edit: notas.md\n# Título\ncontenido\n```";
        let e = extraer(md, Path::new("/p"));
        assert_eq!(e.len(), 1);
        assert!(e[0].busca.is_empty(), "sin buscar nada = crear");
        assert_eq!(e[0].cambia, "# Título\ncontenido\n");
    }

    /// El modelo no siempre escribe la etiqueta igual. Exigir una forma exacta
    /// hacía que el bloque se pintara como un comando, el cambio no se
    /// aplicara, y el modelo dijera «listo» igualmente.
    #[test]
    fn tolera_como_se_escriba_la_etiqueta() {
        let cuerpo = "<<<<<<< BUSCA\na\n=======\nb\n>>>>>>> CAMBIA";
        for etiqueta in ["edit: x.rs", "edit x.rs", "editar: x.rs", "EDIT: x.rs"] {
            let md = format!("```{etiqueta}\n{cuerpo}\n```");
            let e = extraer(&md, Path::new("/p"));
            assert_eq!(e.len(), 1, "no reconoció «{etiqueta}»");
            assert_eq!(e[0].ruta, PathBuf::from("/p/x.rs"), "con «{etiqueta}»");
        }
    }

    #[test]
    fn detecta_una_edicion_que_no_se_pudo_leer() {
        assert!(parece_edicion_fallida(
            "```algo\n<<<<<<< BUSCA\na\n=======\nb\n>>>>>>> CAMBIA\n```"
        ));
        assert!(!parece_edicion_fallida("```bash\nls\n```"));
    }

    #[test]
    fn no_confunde_un_bloque_de_comando() {
        assert!(extraer("```bash\nls -la\n```", Path::new("/p")).is_empty());
        assert!(extraer("```rust\nfn main() {}\n```", Path::new("/p")).is_empty());
    }

    #[test]
    fn respeta_una_ruta_absoluta() {
        let md = "```edit: /etc/hosts\n<<<<<<< BUSCA\na\n=======\nb\n>>>>>>> CAMBIA\n```";
        assert_eq!(
            extraer(md, Path::new("/p"))[0].ruta,
            PathBuf::from("/etc/hosts")
        );
    }

    #[test]
    fn el_resumen_cuenta_lo_que_cambia() {
        let p = Preparado {
            ruta: PathBuf::new(),
            como: Como::Exacto,
            viejo: "a\nb\nc\n".into(),
            nuevo: "a\nB1\nB2\nc\n".into(),
        };
        assert_eq!(resumen(&p), (1, 2), "una fuera, dos dentro");
    }

    /// Editar un `deploy.sh` no puede dejarlo sin poder ejecutarse: los
    /// permisos son del fichero, no de quien lo edita.
    #[test]
    #[cfg(unix)]
    fn editar_no_le_quita_el_bit_de_ejecucion() {
        use std::os::unix::fs::PermissionsExt;

        let d = carpeta("permisos");

        // Varios modos a propósito, y no solo 0o755. `open` aplica
        // `modo & ~umask`, así que 0o755 es justo el que sobrevive intacto a
        // la umask 022 más común: probar solo con él daba un verde que no
        // valía nada. 0o664 y 0o775 son los de un repositorio compartido, y
        // eran los que se rompían.
        for pedido in [0o755u32, 0o664, 0o666, 0o770, 0o600, 0o775] {
            let ruta = d.join(format!("guion-{pedido:o}.sh"));
            std::fs::write(&ruta, "#!/bin/sh\necho uno\n").unwrap();
            std::fs::set_permissions(&ruta, std::fs::Permissions::from_mode(pedido)).unwrap();

            escribir(&edicion_de(&ruta, "echo uno", "echo dos")).unwrap();

            let modo = std::fs::metadata(&ruta).unwrap().permissions().mode() & 0o777;
            assert!(
                std::fs::read_to_string(&ruta).unwrap().contains("echo dos"),
                "el cambio sí se aplicó"
            );
            assert_eq!(modo, pedido, "los permisos son del fichero, no de la umask");
        }
    }

    /// Lo que se enseña y lo que se escribe tienen que ser la misma ruta.
    ///
    /// Seguir el enlace es lo correcto, pero decidirlo al escribir dejaba al
    /// usuario aprobando «notas.txt» mientras los bytes aterrizaban en el otro
    /// extremo del enlace, que puede estar fuera del proyecto.
    #[test]
    #[cfg(unix)]
    fn la_ruta_que_se_ensena_es_donde_se_escribe() {
        let d = carpeta("ruta-visible");
        let real = d.join("destino-real.txt");
        let enlace = d.join("lo-que-se-ve.txt");
        std::fs::write(&real, "uno\n").unwrap();
        std::os::unix::fs::symlink(&real, &enlace).unwrap();

        let p = preparar(&Edicion {
            ruta: enlace.clone(),
            busca: "uno".into(),
            cambia: "dos".into(),
        })
        .unwrap();

        // `p.ruta` es lo que se pinta en la conversación antes de aceptar.
        assert_eq!(
            std::fs::canonicalize(&p.ruta).unwrap(),
            std::fs::canonicalize(&real).unwrap(),
            "se enseña el destino de verdad, no el enlace"
        );

        escribir(&p).unwrap();
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "dos\n");
    }

    #[test]
    fn un_bloque_a_medias_no_produce_una_edicion_rota() {
        // Sin el cierre, no hay nada que aplicar.
        let md = "```edit: x.rs\n<<<<<<< BUSCA\na\n=======\nb\n```";
        assert!(extraer(md, Path::new("/p")).is_empty());
    }

    /// Una carpeta propia por prueba: dos tests que compartan fichero se
    /// pisan al correr en paralelo, y un fallo intermitente en una prueba de
    /// permisos es peor que no tenerla.
    fn carpeta(nombre: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vsion-ed-{}-{nombre}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn edicion_de(ruta: &Path, busca: &str, cambia: &str) -> Preparado {
        preparar(&Edicion {
            ruta: ruta.to_path_buf(),
            busca: busca.into(),
            cambia: cambia.into(),
        })
        .unwrap()
    }

    /// El temporal nace cerrado y acaba con el modo exacto que se pidió.
    ///
    /// La carrera de verdad —el instante entre escribir y corregir permisos—
    /// no la puede observar un test de un solo hilo, y perseguirla con hilos
    /// daría una prueba intermitente. Lo que sí se comprueba son las dos
    /// propiedades de las que depende: que el modo final es el pedido pese a
    /// la umask, y que en ningún momento se pide uno más abierto.
    #[test]
    #[cfg(unix)]
    fn el_temporal_nace_cerrado_y_acaba_con_su_modo() {
        use std::os::unix::fs::PermissionsExt;

        let d = carpeta("temporal");
        // Modos que la umask recortaría: 0o600 era el único que sobrevive
        // intacto a cualquier umask habitual, así que probar solo con él
        // daba un verde que no valía nada.
        for pedido in [0o755u32, 0o664, 0o666, 0o600] {
            let tmp = d.join(format!("recien-nacido-{pedido:o}"));
            let guarda = crear_con_modo(&tmp, Some(pedido), "hola").unwrap();

            let modo = std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777;
            assert_eq!(modo, pedido, "la umask no manda sobre los permisos");
            assert_eq!(std::fs::read_to_string(&tmp).unwrap(), "hola");

            // Y no se escribe encima de lo que ya hubiera: puede no ser nuestro.
            assert_eq!(
                crear_con_modo(&tmp, Some(pedido), "otra cosa")
                    .map_err(|e| e.kind())
                    .err(),
                Some(std::io::ErrorKind::AlreadyExists)
            );
            assert_eq!(std::fs::read_to_string(&tmp).unwrap(), "hola");

            // Al soltar la guarda sin renombrar, el temporal se recoge.
            drop(guarda);
            assert!(!tmp.exists(), "abandonado: se borra");
        }
    }

    /// Editar por un enlace simbólico cambia el fichero de destino y deja el
    /// enlace donde estaba. Renombrar encima lo convertía en fichero normal.
    #[test]
    #[cfg(unix)]
    fn editar_por_un_enlace_no_se_lleva_el_enlace_por_delante() {
        let d = carpeta("enlace");
        let real = d.join("real.txt");
        let enlace = d.join("enlace.txt");
        std::fs::write(&real, "uno\n").unwrap();
        std::os::unix::fs::symlink(&real, &enlace).unwrap();

        escribir(&edicion_de(&enlace, "uno", "dos")).unwrap();

        assert!(
            std::fs::symlink_metadata(&enlace)
                .unwrap()
                .file_type()
                .is_symlink(),
            "el enlace sigue siendo un enlace"
        );
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "dos\n",
            "el cambio llegó al fichero de verdad"
        );
    }

    /// Una edición correcta no deja basura al lado del fichero del usuario.
    #[test]
    fn no_queda_ningun_temporal_tirado() {
        let d = carpeta("basura");
        let f = d.join("cosa.txt");
        std::fs::write(&f, "uno\n").unwrap();

        escribir(&edicion_de(&f, "uno", "dos")).unwrap();

        let sobrantes: Vec<PathBuf> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().contains(".vsion"))
            .collect();
        assert!(sobrantes.is_empty(), "quedó basura: {sobrantes:?}");
    }

    /// Una escritura que falla de verdad no deja basura.
    ///
    /// El test anterior comprobaba el camino feliz, donde el `rename` se lleva
    /// el temporal exista o no la guarda: pasaba igual sin el arreglo. Aquí el
    /// `rename` falla porque el destino es un directorio, que es un fallo
    /// determinista y del lado correcto de la creación.
    #[test]
    fn una_escritura_que_falla_no_deja_basura() {
        let d = carpeta("fallo");
        let estorbo = d.join("soy-un-directorio");
        std::fs::create_dir(&estorbo).unwrap();

        let p = Preparado {
            ruta: estorbo.clone(),
            como: Como::Nuevo,
            viejo: String::new(),
            nuevo: "lo que sea".into(),
        };
        assert!(
            escribir(&p).is_err(),
            "no se puede renombrar sobre un directorio"
        );

        let sobrantes: Vec<PathBuf> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().contains(".vsion"))
            .collect();
        assert!(sobrantes.is_empty(), "quedó basura: {sobrantes:?}");
    }

    /// Dos escrituras seguidas no se disputan el mismo temporal.
    ///
    /// Con un nombre estable por sesión, cualquier resto dejaba el fichero
    /// imposible de editar hasta reiniciar — y `panic = "abort"` en release
    /// significa que la guarda no corre ante un pánico, así que los restos
    /// existen.
    #[test]
    fn cada_intento_usa_un_temporal_distinto() {
        let f = Path::new("/tmp/cosa.txt");
        assert_ne!(ruta_temporal(f), ruta_temporal(f));
    }

    /// Un enlace que no se puede resolver no se pisa: se para y se dice.
    ///
    /// Antes, cualquier fallo de `canonicalize` caía en «pues escribo donde me
    /// dijeron», y eso sustituía el enlace por un fichero normal dejando el
    /// destino real intacto — mientras se anunciaba que el cambio se escribió.
    #[test]
    #[cfg(unix)]
    fn un_enlace_que_no_se_puede_resolver_no_se_pisa() {
        let d = carpeta("irresoluble");
        let cerrado = d.join("cerrado");
        std::fs::create_dir(&cerrado).unwrap();
        std::fs::write(cerrado.join("secreto.txt"), "importante\n").unwrap();

        let enlace = d.join("enlace.txt");
        std::os::unix::fs::symlink(cerrado.join("secreto.txt"), &enlace).unwrap();

        // Sin permiso de paso, `canonicalize` falla con algo que no es NotFound.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cerrado, std::fs::Permissions::from_mode(0o000)).unwrap();

        let r = preparar(&Edicion {
            ruta: enlace.clone(),
            busca: String::new(),
            cambia: "pisado\n".into(),
        });

        std::fs::set_permissions(&cerrado, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(r.is_err(), "no se escribe sin saber a qué fichero apunta");
        assert!(
            std::fs::symlink_metadata(&enlace)
                .unwrap()
                .file_type()
                .is_symlink(),
            "y el enlace sigue en pie"
        );
        assert_eq!(
            std::fs::read_to_string(cerrado.join("secreto.txt")).unwrap(),
            "importante\n"
        );
    }

    /// Un enlace roto se sigue hasta su destino y se crea, en vez de borrarlo.
    #[test]
    #[cfg(unix)]
    fn un_enlace_roto_lleva_a_su_destino() {
        let d = carpeta("roto");
        let destino = d.join("todavia-no-existe.txt");
        let enlace = d.join("enlace.txt");
        std::os::unix::fs::symlink(&destino, &enlace).unwrap();

        let p = preparar(&Edicion {
            ruta: enlace.clone(),
            busca: String::new(),
            cambia: "contenido\n".into(),
        })
        .unwrap();
        escribir(&p).unwrap();

        assert_eq!(std::fs::read_to_string(&destino).unwrap(), "contenido\n");
        assert!(
            std::fs::symlink_metadata(&enlace)
                .unwrap()
                .file_type()
                .is_symlink(),
            "el enlace no se toca"
        );
    }

    /// Y si la maniobra se queda a medias, el temporal se recoge solo.
    #[test]
    fn un_temporal_abandonado_se_recoge() {
        let d = carpeta("recogida");
        let tmp = d.join("a-medias.vsion0");
        std::fs::write(&tmp, "lo que sea").unwrap();

        {
            let _guarda = Temporal {
                ruta: tmp.clone(),
                logrado: false,
            };
        }
        assert!(!tmp.exists(), "abandonado: se borra");

        std::fs::write(&tmp, "lo que sea").unwrap();
        {
            let mut guarda = Temporal {
                ruta: tmp.clone(),
                logrado: false,
            };
            guarda.logrado = true;
        }
        assert!(
            tmp.exists(),
            "logrado: el rename ya se lo llevó, no se toca"
        );
    }

    /// El diff enseña lo que cambia con su número, y lo de alrededor para
    /// saber dónde cae.
    #[test]
    fn el_diff_ensena_lo_que_cambia_con_su_numero() {
        let p = Preparado {
            ruta: "x.rs".into(),
            como: Como::Exacto,
            viejo: "a\nb\nc\nd\ne\nf\n".into(),
            nuevo: "a\nb\nc\nZ\ne\nf\n".into(),
        };
        let d = diff(&p);
        assert!(d.starts_with("```diff\n"), "{d}");
        assert!(d.ends_with("```"), "{d}");
        assert!(d.contains("-   4 d\n"), "{d}");
        assert!(d.contains("+   4 Z\n"), "{d}");
        // Dos de contexto por lado, ni una más.
        assert!(d.contains("    2 b\n") && d.contains("    3 c\n"), "{d}");
        assert!(!d.contains(" 1 a"), "{d}");
        assert!(d.contains("    5 e\n") && d.contains("    6 f\n"), "{d}");
    }

    /// Un fichero nuevo es todo lo que entra, sin nada que se vaya.
    #[test]
    fn un_fichero_nuevo_sale_entero_como_puesto() {
        let p = Preparado {
            ruta: "x.rs".into(),
            como: Como::Exacto,
            viejo: String::new(),
            nuevo: "uno\ndos\n".into(),
        };
        let d = diff(&p);
        assert!(d.contains("+   1 uno\n"), "{d}");
        assert!(d.contains("+   2 dos\n"), "{d}");
        assert!(!d.contains('-'), "{d}");
    }

    /// Un fichero reescrito entero no puede llevarse la conversación por
    /// delante: el diff se corta y dice cuánto falta.
    #[test]
    fn un_cambio_enorme_se_corta_y_lo_dice() {
        let viejo: String = (0..200).map(|i| format!("v{i}\n")).collect();
        let nuevo: String = (0..200).map(|i| format!("n{i}\n")).collect();
        let p = Preparado {
            ruta: "x.rs".into(),
            como: Como::Exacto,
            viejo,
            nuevo,
        };
        let d = diff(&p);
        let fuera = d.lines().filter(|l| l.starts_with('-')).count();
        let dentro = d.lines().filter(|l| l.starts_with('+')).count();
        // Las del tope, mas la que cuenta las que faltan.
        assert_eq!(fuera, MAX_DIFF + 1, "{d}");
        assert_eq!(dentro, MAX_DIFF + 1, "{d}");
        assert!(d.contains(&format!("y {} líneas más que se van", 200 - MAX_DIFF)));
        assert!(d.contains(&format!("y {} líneas más que entran", 200 - MAX_DIFF)));
    }

    /// El recuento y el diff miden lo mismo, y eso no puede depender de que
    /// nadie los toque por separado.
    #[test]
    fn el_recuento_y_el_diff_cuentan_lo_mismo() {
        let p = Preparado {
            ruta: "x.rs".into(),
            como: Como::Exacto,
            viejo: "a\nb\nc\nd\n".into(),
            nuevo: "a\nX\nY\nZ\nd\n".into(),
        };
        let (fuera, dentro) = resumen(&p);
        let d = diff(&p);
        assert_eq!(fuera, d.lines().filter(|l| l.starts_with('-')).count());
        assert_eq!(dentro, d.lines().filter(|l| l.starts_with('+')).count());
    }
}
