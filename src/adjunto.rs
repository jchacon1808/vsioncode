//! Pegar ficheros e imágenes en la conversación.
//!
//! Al arrastrar un fichero a una terminal, lo que llega no es el fichero: es
//! su **ruta**, en texto. Aquí se reconoce esa ruta, se lee lo que apunta y se
//! le pone delante al agente, que es lo que la persona quería decir al soltar
//! el fichero.
//!
//! ## La regla que evita el desastre
//!
//! Se adjunta **solo si todo lo pegado son rutas que existen**. Ni una parte:
//! todo. Pegar un párrafo que casualmente contiene `/etc/passwd` no puede
//! acabar mandándole ese fichero a un modelo, y pegar un fragmento de log con
//! rutas dentro tiene que seguir siendo texto pegado y nada más.
//!
//! ## Lo que no hace
//!
//! No ejecuta, no sigue enlaces a directorios y no manda nada por su cuenta:
//! el adjunto se enseña en pantalla y viaja con **tu** siguiente mensaje. El
//! invariante del proyecto no cambia porque haya una vía nueva de entrada.

use std::path::{Path, PathBuf};

/// Tope de lo que se lee de un fichero de texto.
///
/// No está para ahorrar, está para que soltar por error un volcado de dos
/// gigas no se lleve por delante la ventana de contexto —y la factura— sin
/// que nadie lo haya visto venir.
pub const MAX_TEXTO: usize = 128 * 1024;

/// Tope de una imagen, ya en base64.
///
/// Las APIs que las aceptan rondan los 5 MB por imagen; el base64 abulta un
/// tercio más que el fichero, así que el límite real está sobre los 3,7 MB de
/// fichero. Se queda por debajo a propósito.
pub const MAX_IMAGEN: usize = 3 * 1024 * 1024;

/// Qué es lo que se ha soltado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clase {
    /// Texto: código, logs, configuración. Va tal cual.
    Texto,
    /// Una imagen. Solo la ven los modelos que sepan mirarla.
    Imagen,
    /// Ni una cosa ni otra: un binario, un zip, un vídeo.
    Opaco,
}

/// Un fichero soltado en la conversación.
#[derive(Debug, Clone)]
pub struct Adjunto {
    pub ruta: PathBuf,
    pub clase: Clase,
    /// Lo que ocupa en disco.
    pub bytes: u64,
    /// El contenido, si es texto y cabe.
    pub texto: Option<String>,
}

impl Adjunto {
    /// Cómo se le nombra en pantalla: el nombre del fichero basta.
    pub fn nombre(&self) -> String {
        self.ruta
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.ruta.display().to_string())
    }

    /// El tamaño en algo que se lea de un vistazo.
    pub fn tamano(&self) -> String {
        match self.bytes {
            b if b < 1024 => format!("{b} B"),
            b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.0),
            b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        }
    }
}

/// Extensiones que se tratan como imagen.
const IMAGENES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Saca las rutas de un texto pegado, **solo si todo son rutas existentes**.
///
/// Devuelve vacío en cuanto algo no lo sea, que es lo que mantiene el pegado
/// de texto normal funcionando como siempre.
pub fn rutas(pegado: &str) -> Vec<PathBuf> {
    let mut limpio = pegado.trim();
    if limpio.is_empty() {
        return Vec::new();
    }

    // GNOME antepone su propia cabecera al copiar ficheros desde el gestor:
    // «x-special/nautilus-clipboard», luego «copy», y las URIs debajo. Sin
    // saltarla, la primera línea no es una ruta y se descartaba todo.
    if let Some(resto) = limpio.strip_prefix("x-special/nautilus-clipboard") {
        limpio = resto.trim_start();
    }
    for verbo in ["copy\n", "cut\n"] {
        if let Some(resto) = limpio.strip_prefix(verbo) {
            limpio = resto;
        }
    }
    let limpio = limpio.trim();

    // Una lista de URIs viene una por línea, y ahí un espacio suelto es parte
    // del nombre y no un separador. Partir por espacios rompería justo los
    // nombres que traen espacios, que son los que trajeron el problema.
    if limpio.lines().all(|l| l.trim().starts_with("file://")) {
        let mut salida = Vec::new();
        for l in limpio.lines().filter(|l| !l.trim().is_empty()) {
            let Some(r) = normalizar(l.trim()) else {
                return Vec::new();
            };
            if !r.is_file() {
                return Vec::new();
            }
            salida.push(r);
        }
        return salida;
    }

    // Al soltar varios ficheros llegan separados por espacios y, si el nombre
    // lleva espacios, escapados con barra. Se parte por espacios que no vengan
    // precedidos de barra.
    let mut piezas: Vec<String> = Vec::new();
    let mut actual = String::new();
    let mut escapando = false;
    for c in limpio.chars() {
        if escapando {
            actual.push(c);
            escapando = false;
            continue;
        }
        match c {
            '\\' => escapando = true,
            ' ' | '\n' | '\t' => {
                if !actual.is_empty() {
                    piezas.push(std::mem::take(&mut actual));
                }
            }
            _ => actual.push(c),
        }
    }
    if !actual.is_empty() {
        piezas.push(actual);
    }

    let mut salida = Vec::new();
    for p in piezas {
        let Some(ruta) = normalizar(&p) else {
            return Vec::new();
        };
        if !ruta.is_file() {
            // Un directorio tampoco cuenta: no hay nada que adjuntar y
            // mandarle su listado no es lo que nadie ha pedido.
            return Vec::new();
        }
        salida.push(ruta);
    }
    salida
}

/// Deshace el `%XX` de una URI.
///
/// Sin esto, copiar una imagen desde el gestor de ficheros no funcionaba: la
/// URI trae los espacios como `%20` y los acentos en UTF-8 escapado, así que
/// «~/Imágenes/Capturas de pantalla/x.png» llegaba como una ruta que no existe
/// y se pegaba como texto. Es el caso más común de todos, no un borde raro.
fn desescapar(s: &str) -> String {
    let b = s.as_bytes();
    let mut salida: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            salida.push(v);
            i += 3;
            continue;
        }
        salida.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&salida).into_owned()
}

/// Deja una pieza pegada en una ruta utilizable.
fn normalizar(pieza: &str) -> Option<PathBuf> {
    let p = pieza.trim().trim_matches(['"', '\'']);
    // Los gestores de ficheros de escritorio sueltan URIs.
    let (p, era_uri) = match p.strip_prefix("file://") {
        Some(resto) => (resto, true),
        None => (p, false),
    };
    if p.is_empty() {
        return None;
    }
    let desescapada;
    let p = if era_uri {
        desescapada = desescapar(p);
        desescapada.as_str()
    } else {
        p
    };
    // La otra frontera de entrada: lo que suelta la terminal viene en la
    // forma del shell, y aquí abajo se va a llamar a `std::fs`.
    let nativa = crate::rutas::a_windows(p);
    let p = nativa.as_ref();
    let expandida = match p.strip_prefix("~/") {
        Some(resto) => dirs::home_dir()?.join(resto),
        None => PathBuf::from(p),
    };
    // Una ruta relativa suelta («hola», «vale») haría que cualquier palabra
    // que coincida con un fichero del directorio actual se tratase como
    // adjunto. Se exige ruta absoluta o algo que se vea que lo es.
    if !expandida.is_absolute() && !p.starts_with("./") && !p.starts_with("../") {
        return None;
    }
    Some(expandida)
}

/// Clasifica por extensión y, si hace falta, mirando dentro.
pub fn clasificar(ruta: &Path) -> Clase {
    let ext = ruta
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if IMAGENES.contains(&ext.as_str()) {
        return Clase::Imagen;
    }
    // Sin extensión fiable, decide el contenido: un byte cero no aparece en
    // texto y sí en cualquier binario.
    match std::fs::read(ruta) {
        Ok(bytes) => {
            let cabecera = &bytes[..bytes.len().min(8192)];
            if cabecera.contains(&0) {
                Clase::Opaco
            } else {
                Clase::Texto
            }
        }
        Err(_) => Clase::Opaco,
    }
}

/// Lee un fichero para adjuntarlo.
pub fn leer(ruta: &Path) -> std::io::Result<Adjunto> {
    let meta = std::fs::metadata(ruta)?;
    let clase = clasificar(ruta);
    let bytes = meta.len();

    let texto = if clase == Clase::Texto {
        let crudo = std::fs::read_to_string(ruta)?;
        Some(if crudo.len() > MAX_TEXTO {
            let mut corte = MAX_TEXTO;
            while corte > 0 && !crudo.is_char_boundary(corte) {
                corte -= 1;
            }
            format!(
                "{}\n\n[…el fichero sigue: {} en total, cortado aquí.]",
                &crudo[..corte],
                bytes
            )
        } else {
            crudo
        })
    } else {
        None
    };

    Ok(Adjunto {
        ruta: ruta.to_path_buf(),
        clase,
        bytes,
        texto,
    })
}

/// Una imagen lista para viajar en el cuerpo de la petición.
///
/// El dialecto de OpenAI las quiere como `data:` URI dentro de un bloque
/// `image_url`, así que se codifica el fichero tal cual: no se recomprime ni
/// se reescala. Reescalar aquí sería decidir por el modelo qué detalle puede
/// permitirse ver, y una captura de pantalla vive precisamente del detalle.
pub fn partes(a: &Adjunto) -> Option<(&'static str, String)> {
    use base64::Engine as _;
    if a.clase != Clase::Imagen || a.bytes as usize > MAX_IMAGEN {
        return None;
    }
    let bytes = std::fs::read(&a.ruta).ok()?;
    let tipo = match a
        .ruta
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    };
    Some((
        tipo,
        base64::engine::general_purpose::STANDARD.encode(&bytes),
    ))
}

/// Las imágenes de un lote, en crudo: tipo y base64.
pub fn imagenes_crudas(adjuntos: &[Adjunto]) -> Vec<(&'static str, String)> {
    adjuntos.iter().filter_map(partes).collect()
}

pub fn data_uri(a: &Adjunto) -> Option<String> {
    let (tipo, b64) = partes(a)?;
    Some(format!("data:{tipo};base64,{b64}"))
}

/// Las imágenes de un lote que caben y se pueden mandar.
pub fn imagenes(adjuntos: &[Adjunto]) -> Vec<String> {
    adjuntos.iter().filter_map(data_uri).collect()
}

/// El texto con el que los adjuntos entran en el turno.
///
/// Va con el mensaje de la persona, no en uno aparte: soltar un fichero es
/// parte de lo que está diciendo, no un turno propio.
pub fn para_el_modelo(adjuntos: &[Adjunto]) -> String {
    let mut s = String::new();
    for a in adjuntos {
        match a.clase {
            Clase::Texto => {
                s.push_str(&format!(
                    "\n\n--- {} ({}) ---\n{}\n--- fin de {} ---",
                    a.ruta.display(),
                    a.tamano(),
                    a.texto.as_deref().unwrap_or(""),
                    a.nombre()
                ));
            }
            Clase::Imagen => {
                // Si la imagen viaja de verdad, esto solo la sitúa; si no
                // cabe, es lo único que le llega y tiene que decirlo claro.
                let cabe = a.bytes as usize <= MAX_IMAGEN;
                s.push_str(&if cabe {
                    format!("\n\n[Imagen adjunta: {}]", a.nombre())
                } else {
                    format!(
                        "\n\n[El usuario adjuntó la imagen {} ({}), demasiado grande para \
                         mandártela. No la he visto; pregúntale qué hay en ella.]",
                        a.nombre(),
                        a.tamano()
                    )
                });
            }
            Clase::Opaco => {
                s.push_str(&format!(
                    "\n\n[El usuario ha adjuntado {} ({}), que no es texto y no puedo \
                     enseñarte. Si hace falta mirarlo, propón un comando.]",
                    a.ruta.display(),
                    a.tamano()
                ));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporal(nombre: &str, contenido: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("vsion-adj-{}-{nombre}", std::process::id()));
        std::fs::write(&p, contenido).unwrap();
        p
    }

    #[test]
    fn reconoce_una_ruta_soltada() {
        let f = temporal("uno.txt", b"hola");
        let r = rutas(f.to_str().unwrap());
        assert_eq!(r, vec![f.clone()]);
        let _ = std::fs::remove_file(f);
    }

    /// La regla que evita el desastre: si algo de lo pegado no es una ruta,
    /// no se adjunta nada y el texto sigue siendo texto.
    #[test]
    fn un_texto_con_una_ruta_dentro_no_es_un_adjunto() {
        let f = temporal("dos.txt", b"hola");
        let pegado = format!("mira el fallo que sale en {}", f.display());
        assert!(rutas(&pegado).is_empty());
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn el_texto_normal_no_se_confunde_nunca() {
        assert!(rutas("hola qué tal").is_empty());
        assert!(rutas("").is_empty());
        assert!(rutas("/no/existe/esto.txt").is_empty());
    }

    /// Una palabra suelta que coincida con un fichero del directorio actual no
    /// puede convertirse en un adjunto por accidente.
    #[test]
    fn una_ruta_relativa_suelta_no_cuenta() {
        assert!(rutas("Cargo.toml").is_empty());
    }

    #[test]
    fn quita_comillas_y_el_esquema_de_los_escritorios() {
        let f = temporal("tres.txt", b"hola");
        let entre_comillas = format!("\"{}\"", f.display());
        assert_eq!(rutas(&entre_comillas), vec![f.clone()]);
        let uri = format!("file://{}", f.display());
        assert_eq!(rutas(&uri), vec![f.clone()]);
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn distingue_imagen_de_texto_y_de_binario() {
        let img = temporal("captura.png", b"\x89PNG\r\n");
        assert_eq!(clasificar(&img), Clase::Imagen);
        let txt = temporal("codigo.rs", b"fn main() {}");
        assert_eq!(clasificar(&txt), Clase::Texto);
        let bin = temporal("cosa.dat", b"\x00\x01\x02");
        assert_eq!(clasificar(&bin), Clase::Opaco);
        for f in [img, txt, bin] {
            let _ = std::fs::remove_file(f);
        }
    }

    #[test]
    fn un_fichero_enorme_se_corta_y_se_dice() {
        let grande = "x".repeat(MAX_TEXTO + 5000);
        let f = temporal("grande.log", grande.as_bytes());
        let a = leer(&f).unwrap();
        let t = a.texto.unwrap();
        assert!(t.len() < grande.len());
        assert!(t.contains("cortado aquí"));
        let _ = std::fs::remove_file(f);
    }

    /// Dos capturas pegadas seguidas no pueden compartir fichero.
    ///
    /// El contenido no se lee al pegar sino al enviar el mensaje, así que
    /// mientras tanto los dos adjuntos esperan apuntando a su ruta: con el
    /// mismo nombre, la segunda pisa a la primera y al modelo le llegan dos
    /// veces la misma imagen.
    #[test]
    fn dos_pegados_seguidos_no_comparten_fichero() {
        let primera = portapapeles::ruta_pegado("png");
        let segunda = portapapeles::ruta_pegado("png");
        assert_ne!(primera, segunda, "{} se repite", primera.display());
        assert_eq!(primera.extension().unwrap(), "png");
        assert_eq!(primera.parent(), Some(std::env::temp_dir().as_path()));
    }

    /// Un pegado por sesión y sesiones hay muchas: lo que quedó de un
    /// vsioncode muerto se barre, y lo de este proceso se queda —puede estar
    /// esperando a que envíes el mensaje—.
    #[test]
    fn barre_los_pegados_de_una_sesion_muerta_y_respeta_los_vivos() {
        // 4194304 es el tope de PID de Linux, así que nunca es un proceso.
        let muerto = std::env::temp_dir().join("vsioncode-pegado-4194304-7.png");
        std::fs::write(&muerto, b"x").unwrap();

        let mio = portapapeles::ruta_pegado("png");
        std::fs::write(&mio, b"x").unwrap();
        // El barrido va dentro de pedir un nombre nuevo.
        let _ = portapapeles::ruta_pegado("png");

        assert!(!muerto.exists(), "no barrió el de la sesión muerta");
        assert!(mio.exists(), "se llevó por delante un adjunto en uso");
        let _ = std::fs::remove_file(mio);
    }

    #[test]
    fn el_tamano_se_lee_de_un_vistazo() {
        let mut a = Adjunto {
            ruta: PathBuf::from("/x/y.txt"),
            clase: Clase::Texto,
            bytes: 512,
            texto: None,
        };
        assert_eq!(a.tamano(), "512 B");
        a.bytes = 2048;
        assert_eq!(a.tamano(), "2 KB");
        a.bytes = 5 * 1024 * 1024;
        assert_eq!(a.tamano(), "5.0 MB");
    }
}

// ------------------------------------------------------------- portapapeles

/// El portapapeles del sistema.
///
/// Una terminal **no transmite imágenes**: por el pegado solo viaja texto. Así
/// que para pegar una captura como se pega en cualquier otro sitio, hay que ir
/// a buscarla al portapapeles del escritorio.
///
/// ## Por qué esto sí va dentro del binario
///
/// Rompe el criterio del resto del proyecto —whisper, piper y el modelo viven
/// fuera— y se hizo a sabiendas. Pegar una captura se usa a diario, y hacerlo
/// depender de que cada uno instale `wl-clipboard` convierte una función en un
/// trámite. El coste se midió antes de decidir: 653 KB, casi todos del
/// protocolo Wayland y del cliente X11.
///
/// Queda el camino externo como respaldo, porque cuesta cero y cubre los casos
/// donde `arboard` no puede: una sesión sin escritorio, o un servidor gráfico
/// que no responde.
pub mod portapapeles {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Qué había en el portapapeles.
    pub enum Contenido {
        /// Una imagen, ya volcada a un fichero temporal.
        Imagen(PathBuf),
        Texto(String),
        /// Estaba vacío, o no se pudo leer.
        Nada,
        /// No hay ni biblioteca ni programa con que leerlo. Pasa por SSH, que
        /// es donde no existe un portapapeles gráfico que leer.
        SinHerramienta,
    }

    /// Lee lo que haya, con la imagen por delante del texto.
    ///
    /// El orden importa: al copiar una imagen, muchos programas ofrecen
    /// **además** una versión en texto —el nombre del fichero, o un trozo de
    /// HTML—. Mirando el texto primero, pegar una captura acabaría pegando su
    /// nombre, que es justo lo que se quería evitar.
    pub fn leer() -> Contenido {
        match arboard::Clipboard::new() {
            Ok(mut c) => {
                if let Ok(img) = c.get_image()
                    && let Some(ruta) = volcar_png(&img)
                {
                    return Contenido::Imagen(ruta);
                }
                match c.get_text() {
                    Ok(t) if !t.trim().is_empty() => Contenido::Texto(t),
                    _ => externo(),
                }
            }
            // Sin servidor gráfico al que conectarse: queda el respaldo.
            Err(_) => externo(),
        }
    }

    /// El temporal donde aterriza una imagen del portapapeles.
    ///
    /// Cada pegado necesita el suyo. El contenido no se lee al pegar sino al
    /// enviar el mensaje —los adjuntos esperan en `App::adjuntos` a que lo
    /// mandes—, así que un nombre repetido no da un error: hace que la segunda
    /// captura pise a la primera y que al modelo le lleguen dos iguales.
    ///
    /// Lo decide un contador y no el reloj porque dos pegados seguidos caen en
    /// el mismo milisegundo. Los dos caminos —`arboard` y el externo— pasan por
    /// aquí para que el fallo no pueda volver por un solo lado.
    pub fn ruta_pegado(ext: &str) -> PathBuf {
        static PEGADOS: AtomicU64 = AtomicU64::new(0);
        let n = PEGADOS.fetch_add(1, Ordering::Relaxed);

        // Es el único punto por el que pasan todos los pegados, así que es
        // donde toca barrer lo que quedó de sesiones anteriores.
        limpiar_huerfanos();

        std::env::temp_dir().join(format!("vsioncode-pegado-{}-{n}.{ext}", std::process::id()))
    }

    /// Borra los pegados que dejó un vsioncode que ya no está.
    ///
    /// Igual que `pty.rs` con sus ficheros de arranque, y aquí pesa más: una
    /// captura ocupa megas y se pega a diario, así que sin barrerlas `/tmp`
    /// acumula todas las de todas las sesiones. Las del proceso vivo no se
    /// tocan: puede tener una esperando a que envíes el mensaje.
    fn limpiar_huerfanos() {
        let Ok(entradas) = std::fs::read_dir(std::env::temp_dir()) else {
            return;
        };
        for e in entradas.flatten() {
            let nombre = e.file_name();
            let Some(resto) = nombre
                .to_str()
                .and_then(|n| n.strip_prefix("vsioncode-pegado-"))
            else {
                continue;
            };
            // `vsioncode-pegado-<pid>-<n>.<ext>`. Lo que no encaje con eso es
            // del diseño anterior, que compartía un fichero por proceso: se
            // borra sin más.
            match resto.split('-').next().and_then(|p| p.parse::<u32>().ok()) {
                Some(pid) if crate::sistema::vivo(pid) => {}
                _ => {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    /// Guarda la imagen del portapapeles como PNG en un temporal.
    ///
    /// A un fichero, y no a memoria, porque lo que se adjunta son ficheros:
    /// así la imagen pegada y la que arrastras del escritorio recorren
    /// exactamente el mismo camino a partir de aquí.
    fn volcar_png(img: &arboard::ImageData) -> Option<PathBuf> {
        let ruta = ruta_pegado("png");

        let fichero = std::fs::File::create(&ruta).ok()?;
        let mut enc = png::Encoder::new(
            std::io::BufWriter::new(fichero),
            img.width as u32,
            img.height as u32,
        );
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().ok()?.write_image_data(&img.bytes).ok()?;
        Some(ruta)
    }

    // ------------------------------------------------------------- respaldo

    /// El camino de siempre: pedírselo a `wl-paste` o a `xclip`.
    fn externo() -> Contenido {
        let Some(orden) = herramienta() else {
            return Contenido::SinHerramienta;
        };
        let tipos = tipos(orden);

        if let Some(tipo) = tipos.iter().find(|t| t.starts_with("image/"))
            && let Some(datos) = bytes(orden, tipo)
        {
            let ext = tipo.rsplit('/').next().unwrap_or("png");
            let ruta = ruta_pegado(ext);
            if std::fs::write(&ruta, datos).is_ok() {
                return Contenido::Imagen(ruta);
            }
        }

        match bytes(orden, "text/plain") {
            Some(b) => Contenido::Texto(String::from_utf8_lossy(&b).to_string()),
            None => Contenido::Nada,
        }
    }

    /// La orden que toca en este escritorio.
    ///
    /// Wayland primero: en una sesión Wayland `DISPLAY` suele estar puesto
    /// igualmente por XWayland, así que mirar X11 antes daría con el
    /// portapapeles equivocado.
    fn herramienta() -> Option<&'static str> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() && existe("wl-paste") {
            return Some("wl-paste");
        }
        if std::env::var_os("DISPLAY").is_some() && existe("xclip") {
            return Some("xclip");
        }
        None
    }

    fn existe(orden: &str) -> bool {
        Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {orden}"))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn tipos(orden: &str) -> Vec<String> {
        let salida = match orden {
            "wl-paste" => Command::new("wl-paste").arg("--list-types").output(),
            _ => Command::new("xclip")
                .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
                .output(),
        };
        salida
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn bytes(orden: &str, tipo: &str) -> Option<Vec<u8>> {
        let o = match orden {
            "wl-paste" => Command::new("wl-paste")
                .args(["--no-newline", "--type", tipo])
                .output(),
            _ => Command::new("xclip")
                .args(["-selection", "clipboard", "-t", tipo, "-o"])
                .output(),
        }
        .ok()?;
        o.status
            .success()
            .then_some(o.stdout)
            .filter(|b| !b.is_empty())
    }

    /// Qué decir cuando no hay forma de leerlo.
    pub fn como_instalarlo() -> &'static str {
        "No encuentro portapapeles. Por SSH no hay ninguno que leer; \
         en un escritorio, arrastra el fichero al panel en vez de pegarlo."
    }
}
