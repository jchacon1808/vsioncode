//! Skills: manuales que el modelo puede pedir cuando le hacen falta.
//!
//! Una skill es una carpeta con un `SKILL.md` dentro: un encabezado con
//! `name` y `description`, y debajo instrucciones en prosa sobre cómo hacer
//! algo bien. El formato es abierto y es el mismo que usan Claude Code y
//! `agents`, así que cualquier skill de por ahí vale aquí tal cual.
//!
//! **El formato se comparte; el armario no.** vsioncode lee sus propias
//! carpetas —`~/.config/vsioncode/skills` y `.vsioncode/skills` del
//! proyecto— y no las de Claude Code. Son dos programas: lo que instales
//! para uno no aparece en el otro sin que tú lo pongas. Para traerte una de
//! fuera está `/skills instalar`, que copia el `SKILL.md` aquí.
//!
//! ## Cómo se activa, y por qué así
//!
//! Al arrancar el cerebro se le da un **catálogo**: una línea por skill, con
//! el nombre y el principio de su descripción. Nada más — el cuerpo entero de
//! diez skills serían ciento cincuenta mil caracteres pagados en cada turno,
//! para usar uno.
//!
//! Cuando el modelo ve que una le viene al caso, escribe en una línea suelta
//! `NECESITO-SKILL: nombre`, y vsioncode le manda el fichero entero en el
//! turno siguiente. Es el mismo patrón que [`crate::ai::PETICION_LIBRE`]: una
//! palabra exacta, no una intención adivinada por parecido de texto.
//!
//! ## Qué NO hace
//!
//! Leer una skill es leer un fichero de texto. **No ejecuta nada**: ni los
//! `scripts/` que algunas traen dentro, ni los bloques ` ```bash ` de sus
//! ejemplos. Si de leerla el modelo concluye que hay que ejecutar algo, lo
//! propone como cualquier otro comando y pasa por [`crate::safety`] igual.
//! Una capacidad nueva con un camino propio «porque es distinta» sería el
//! principio del fin del modelo de seguridad.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Cuánto se enseña de cada descripción en el catálogo.
///
/// Las descripciones reales rondan los 600 caracteres porque están escritas
/// para que un modelo decida si la skill aplica. En el catálogo eso se paga
/// en cada turno y multiplicado por el número de skills instaladas, así que
/// se corta: para saber que algo *existe* basta el principio, y al pedirla
/// llega entera.
const MAX_DESCRIPCION: usize = 180;

/// Cuántas skills caben en el catálogo.
///
/// Con diez instaladas sobra; hay quien tiene cincuenta, y ahí el catálogo
/// empezaría a desplazar de la ventana lo que de verdad importa.
const MAX_EN_CATALOGO: usize = 30;

/// Tope de lo que se inyecta al leer una skill.
///
/// La más gorda de las que hay medidas ocupa 27 KB. El tope no está para
/// ahorrar, está para que un fichero enorme por error no se lleve por delante
/// la ventana de contexto entera sin avisar.
const MAX_CUERPO: usize = 48 * 1024;

/// Una skill encontrada en el disco.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Con lo que se la pide. Del `name:` del encabezado, o de la carpeta.
    pub nombre: String,
    /// Para qué sirve, tal cual lo dice su autor.
    pub descripcion: String,
    /// El `SKILL.md`.
    pub ruta: PathBuf,
}

/// La palabra con la que el modelo pide una skill.
pub const PETICION: &str = "NECESITO-SKILL:";

/// La palabra con la que el modelo dice **de dónde** traerse una skill.
///
/// El mismo patrón que [`PETICION`] y por el mismo motivo: una palabra exacta
/// en una línea suelta, no una intención adivinada por parecido de texto.
/// Detrás va un origen de los que entiende [`comando_instalar`].
///
/// Existe para que `/skills instalar <lo que buscas>` sea un paso y no seis:
/// el modelo busca y dice de dónde, y **vsioncode arma el comando**. Ese
/// reparto es lo que conserva el freno — el último paso no puede ser suyo,
/// porque lo que se está trayendo son las instrucciones que va a seguir él.
pub const PROPUESTA: &str = "SKILL-DE:";

/// La carpeta de skills de vsioncode, la de casa.
///
/// Al lado de `proveedores.toml`, porque es configuración del programa y no
/// del proyecto.
pub fn carpeta() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("vsioncode/skills")
}

/// La carpeta de skills de un proyecto.
pub fn carpeta_de(cwd: &Path) -> PathBuf {
    cwd.join(".vsioncode/skills")
}

/// De dónde puede venir una skill de fuera.
#[derive(Debug, PartialEq, Eq)]
enum Origen {
    /// Una carpeta que ya está en el disco.
    Local(String),
    /// Un `SKILL.md` suelto en la web.
    Fichero { url: String, nombre: String },
    /// Un repositorio. `sub` es una skill concreta dentro; sin ella se traen
    /// todas las que tenga.
    Repo {
        url: String,
        rama: Option<String>,
        sub: Option<String>,
    },
}

/// El comando que trae una skill de fuera, listo para que lo lances **tú**.
///
/// vsioncode no baja nada por su cuenta. Instalar sale a la red y escribe en
/// el disco, que son justo las dos cosas que este programa no hace a tus
/// espaldas: arma el comando, lo deja en tu línea y lo pulsas tú. Así pasa por
/// [`crate::safety`] como cualquier otro y, sobre todo, lo lees antes — que lo
/// que se está trayendo son instrucciones que luego va a seguir el modelo.
pub fn comando_instalar(origen: &str, destino: &Path) -> Result<String, String> {
    let dest = comilla(&destino.to_string_lossy());
    match entender(origen)? {
        // Un `cp -r` de la carpeta tal cual solo vale si la carpeta *es* la
        // skill. Lo que se apunta la mitad de las veces es el armario entero
        // —`~/.claude/skills`, con diez dentro—, y eso copiado a pelo deja un
        // `skills/tui-design/SKILL.md` que `descubrir` no ve, porque solo mira
        // un nivel: no falla, no dice nada, y no hay ninguna skill. Buscar el
        // `SKILL.md` y copiar su carpeta acierta en los dos casos.
        Origen::Local(ruta) => Ok(format!(
            "mkdir -p {dest} && find {} -maxdepth 2 -name SKILL.md -not -path '*/.git/*' \
             -printf '%h\\0' | xargs -0 -r -I@ cp -r @ {dest}/{}",
            comilla(&ruta),
            recuento(&dest)
        )),

        Origen::Fichero { url, nombre } => Ok(format!(
            "mkdir -p {dest}/{nombre} && curl -fsSL {} -o {dest}/{nombre}/SKILL.md{}",
            comilla(&url),
            recuento(&dest)
        )),

        Origen::Repo { url, rama, sub } => {
            let rama = rama
                .map(|r| format!(" --branch {}", comilla(&r)))
                .unwrap_or_default();
            let clon = format!(
                "tmp=$(mktemp -d) && git clone --depth 1{rama} --quiet {} \"$tmp\"",
                comilla(&url)
            );
            // Con `sub`, esa carpeta. Sin él, todas las que tengan SKILL.md:
            // un repositorio de skills suele traer varias y bajarlas una a una
            // sería pedirle al usuario que lea el árbol a mano.
            let copia = match sub {
                Some(s) => format!("cp -r \"$tmp\"/{} {dest}/", comilla(&s)),
                None => format!(
                    "find \"$tmp\" -name SKILL.md -not -path '*/.git/*' -printf '%h\\0' \
                     | xargs -0 -r -I@ cp -r @ {dest}/"
                ),
            };
            Ok(format!(
                "{clon} && mkdir -p {dest} && {copia} && rm -rf \"$tmp\"{}",
                recuento(&dest)
            ))
        }
    }
}

/// ¿Esto es un sitio de donde traerse algo, o es una frase?
///
/// Lo que separa `/skills instalar usuario/repo` de `/skills instalar skills de
/// anthropic para diseño`. Se pregunta con lo que ya sabe leer `entender`, en
/// vez de con un patrón nuevo: dos formas de decidir lo mismo acabarían
/// discrepando, y la discrepancia sería una búsqueda en internet donde había
/// una ruta, o al revés.
pub fn es_origen(que: &str) -> bool {
    entender(que).is_ok()
}

fn entender(origen: &str) -> Result<Origen, String> {
    let o = origen.trim().trim_end_matches('/');
    if o.is_empty() {
        return Err("Dime de dónde: «usuario/repo», una URL a un SKILL.md, o una carpeta.".into());
    }

    // Una carpeta del disco. El `~` se expande aquí porque el comando lleva
    // comillas simples y dentro de ellas la shell no lo tocaría.
    if o.starts_with('/') || o.starts_with('.') || o.starts_with('~') {
        let ruta = match o.strip_prefix("~/") {
            Some(resto) => dirs::home_dir()
                .ok_or("no sé cuál es tu carpeta personal")?
                .join(resto)
                .to_string_lossy()
                .to_string(),
            None => o.to_string(),
        };
        return Ok(Origen::Local(ruta));
    }

    if o.ends_with("/SKILL.md") && (o.starts_with("http://") || o.starts_with("https://")) {
        let nombre = o
            .rsplit('/')
            .nth(1)
            .filter(|n| !n.is_empty())
            .ok_or("de esa URL no saco el nombre de la skill")?;
        validar_nombre(nombre)?;
        return Ok(Origen::Fichero {
            url: o.to_string(),
            nombre: nombre.to_string(),
        });
    }

    // `https://github.com/u/r`, con o sin `/tree/<rama>/<ruta>` detrás, que es
    // lo que sale al copiar la barra de direcciones del navegador.
    let sin_esquema = o
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    if let Some(resto) = sin_esquema.strip_prefix("github.com/") {
        return github(resto);
    }
    // Otra forja cualquiera: se clona tal cual y se cogen todas.
    if o.starts_with("http://") || o.starts_with("https://") || o.starts_with("git@") {
        return Ok(Origen::Repo {
            url: o.to_string(),
            rama: None,
            sub: None,
        });
    }

    // `usuario/repo` a secas: el atajo de GitHub, que es como se nombran los
    // repositorios de skills por ahí.
    if o.split('/').count() >= 2 && !o.contains(' ') {
        return github(o);
    }

    Err(format!(
        "No entiendo «{origen}». Vale «usuario/repo», una URL de GitHub, una URL a un \
         SKILL.md o una carpeta del disco."
    ))
}

fn github(resto: &str) -> Result<Origen, String> {
    let trozos: Vec<&str> = resto.split('/').filter(|t| !t.is_empty()).collect();
    let (usuario, repo) = match trozos.as_slice() {
        [u, r, ..] => (*u, r.trim_end_matches(".git")),
        _ => return Err("Falta el repositorio: «usuario/repo».".into()),
    };
    let url = format!("https://github.com/{usuario}/{repo}.git");

    // `/tree/<rama>/<ruta…>`: lo que pega el navegador.
    let (rama, sub) = match trozos.get(2) {
        Some(&"tree") | Some(&"blob") => {
            let rama = trozos.get(3).map(|r| r.to_string());
            let sub = (trozos.len() > 4).then(|| trozos[4..].join("/"));
            (rama, sub)
        }
        // `usuario/repo/ruta/a/skill` sin `tree`.
        Some(_) => (None, Some(trozos[2..].join("/"))),
        None => (None, None),
    };
    Ok(Origen::Repo { url, rama, sub })
}

fn validar_nombre(n: &str) -> Result<(), String> {
    if n.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !n.starts_with('.')
    {
        Ok(())
    } else {
        Err(format!("«{n}» no me vale como nombre de skill."))
    }
}

/// La coletilla que enseña el armario al terminar de instalar.
///
/// Instalar es la única cosa de este programa cuyo resultado no se ve por
/// ninguna parte: las skills se leen al arrancar, así que hasta el siguiente
/// reinicio no hay forma de saber si llegó algo. Listar la carpeta cuesta una
/// línea y convierte «reinicia para que la vea» en una comprobación.
fn recuento(dest: &str) -> String {
    format!(" && ls -1 {dest}")
}

/// Entrecomilla para la shell. Lo que viene de fuera no se pega tal cual.
fn comilla(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Dónde se buscan, en orden de prioridad.
///
/// **Solo las suyas.** Antes se leían también `~/.claude/skills` y
/// `~/.agents/skills` con el argumento de aprovechar lo que el usuario ya
/// tenía. El argumento era malo: son dos programas distintos, y que instalar
/// una skill para Claude Code la hiciera aparecer aquí —sin haberla puesto, y
/// escrita para otro sitio— no es aprovechar, es heredar el armario de otro.
/// Aquí se instala lo que se quiere aquí.
///
/// El proyecto manda sobre lo global: una skill del repositorio describe
/// *este* código, y si comparte nombre con una de casa, gana la del
/// repositorio.
fn directorios(cwd: &Path) -> Vec<PathBuf> {
    vec![carpeta_de(cwd), carpeta()]
}

/// Busca las skills instaladas.
///
/// Se deduplica por nombre y gana la de mayor prioridad: así el proyecto puede
/// tener su propia versión de una skill de casa sin que salgan las dos.
pub fn descubrir(cwd: &Path) -> Vec<Skill> {
    let mut vistas: BTreeMap<String, Skill> = BTreeMap::new();

    for dir in directorios(cwd) {
        let Ok(entradas) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entradas.flatten() {
            let md = e.path().join("SKILL.md");
            if !md.is_file() {
                continue;
            }
            let Some(s) = leer_cabecera(&md) else {
                continue;
            };
            vistas.entry(s.nombre.clone()).or_insert(s);
        }
    }

    vistas.into_values().collect()
}

/// Lee el encabezado de un `SKILL.md` sin cargarse el fichero entero.
fn leer_cabecera(ruta: &Path) -> Option<Skill> {
    let texto = std::fs::read_to_string(ruta).ok()?;
    let campos = frontmatter(&texto);

    // Sin `name:` vale el nombre de la carpeta: es lo que hace todo el mundo
    // al referirse a una skill, y descartarla por eso sería perder una skill
    // que funciona por un campo que se puede deducir.
    let nombre = campos
        .get("name")
        .map(String::as_str)
        .filter(|n| !n.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            ruta.parent()?
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })?;

    Some(Skill {
        nombre,
        descripcion: campos.get("description").cloned().unwrap_or_default(),
        ruta: ruta.to_path_buf(),
    })
}

/// Saca los campos del encabezado YAML entre `---` y `---`.
///
/// A mano y no con un analizador de YAML completo: aquí solo hacen falta
/// `clave: valor` y los bloques con `|`, y arrastrar `serde_yaml` por eso
/// serían cientos de kilobytes en un binario cuyo tamaño es una característica
/// declarada del proyecto.
fn frontmatter(texto: &str) -> BTreeMap<String, String> {
    let mut campos = BTreeMap::new();
    let mut lineas = texto.lines();

    if lineas.next().map(str::trim) != Some("---") {
        return campos;
    }

    // La clave cuyo valor sigue en las líneas indentadas de debajo.
    let mut bloque: Option<(String, String)> = None;

    for linea in lineas {
        if linea.trim() == "---" {
            break;
        }

        // Una línea indentada continúa el bloque abierto; si no hay ninguno,
        // es un campo anidado que aquí no interesa.
        let indentada = linea.starts_with(' ') || linea.starts_with('\t');
        if indentada {
            if let Some((_, valor)) = bloque.as_mut() {
                if !valor.is_empty() {
                    valor.push(' ');
                }
                valor.push_str(linea.trim());
            }
            continue;
        }

        if let Some((clave, valor)) = bloque.take() {
            campos.insert(clave, valor);
        }

        let Some((clave, resto)) = linea.split_once(':') else {
            continue;
        };
        let clave = clave.trim().to_ascii_lowercase();
        let resto = resto.trim();

        // `|`, `>`, `|-`… abren un bloque: el valor está debajo.
        if resto.is_empty() || resto.starts_with('|') || resto.starts_with('>') {
            bloque = Some((clave, String::new()));
        } else {
            campos.insert(clave, desentrecomillar(resto).to_string());
        }
    }

    if let Some((clave, valor)) = bloque.take() {
        campos.insert(clave, valor);
    }
    campos
}

fn desentrecomillar(s: &str) -> &str {
    for c in ['"', '\''] {
        if let Some(d) = s.strip_prefix(c).and_then(|r| r.strip_suffix(c)) {
            return d;
        }
    }
    s
}

/// El catálogo que ve el modelo. Vacío si no hay skills.
///
/// Va en el prompt de sistema, así que cada carácter se paga en cada turno:
/// nombre, un resumen corto, y cómo pedirla. Nada más.
pub fn catalogo(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut s = String::from(
        "\n\nTienes acceso a instrucciones especializadas («skills»). Cada una es un \
         documento que explica cómo hacer bien un tipo de tarea. NO son ficheros que \
         tengas que buscar, crear ni instalar: las gestiona vsioncode y te llegan \
         pidiéndolas. Estas son las que hay:\n",
    );
    for k in skills.iter().take(MAX_EN_CATALOGO) {
        s.push_str(&format!("- {}: {}\n", k.nombre, recortar(&k.descripcion)));
    }
    if skills.len() > MAX_EN_CATALOGO {
        s.push_str(&format!(
            "- (y {} más, sin sitio aquí)\n",
            skills.len() - MAX_EN_CATALOGO
        ));
    }
    s.push_str(
        "Si una viene al caso, escribe en una línea suelta exactamente NECESITO-SKILL: \
         nombre y te llega el documento entero en el turno siguiente; entonces sigue sus \
         instrucciones. Esa línea es para vsioncode, no para el usuario: no la anuncies \
         ni la expliques, solo escríbela. Y solo cuando la tarea sea la que describe: \
         pedir una que no toca gasta contexto sin ganar nada. Si te preguntan por ellas, \
         contesta con esta lista y no salgas a mirar el disco: no viven en el directorio \
         de trabajo y no hay ninguna carpeta que crear.",
    );
    s
}

fn recortar(d: &str) -> String {
    let d = d.split_whitespace().collect::<Vec<_>>().join(" ");
    if d.chars().count() <= MAX_DESCRIPCION {
        return d;
    }
    // Se corta en un espacio para no dejar media palabra, y solo si el espacio
    // está razonablemente cerca del final.
    let corte: String = d.chars().take(MAX_DESCRIPCION).collect();
    let corte = match corte.rfind(' ') {
        Some(i) if i > MAX_DESCRIPCION / 2 => &corte[..i],
        _ => &corte[..],
    };
    format!("{corte}…")
}

/// ¿Ha pedido el modelo una skill? Devuelve el nombre.
///
/// La línea tiene que ser justo la petición y nada más. Con el token a media
/// frase, explicarle a alguien qué es `NECESITO-SKILL:` acabaría cargando una
/// skill que nadie quería.
pub fn pedida(texto: &str) -> Option<String> {
    texto.lines().find_map(|l| {
        let t = l.trim();
        let nombre = t.strip_prefix(PETICION)?.trim();
        let nombre = nombre.trim_matches(|c: char| c == '`' || c == '"' || c == '\'');
        (!nombre.is_empty() && !nombre.contains(char::is_whitespace)).then(|| nombre.to_string())
    })
}

/// ¿Ha dicho el modelo de dónde traerse una skill? Devuelve el origen.
///
/// Al revés que [`pedida`], aquí sí puede llevar espacios detrás —una URL con
/// una ruta larga—, así que lo que se exige es que la línea empiece por la
/// palabra y traiga algo. Lo que no valga como origen se rechaza después, al
/// armar el comando: aquí no se adivina, se recoge.
pub fn propuesta(texto: &str) -> Option<String> {
    texto.lines().find_map(|l| {
        let t = l.trim();
        let origen = t.strip_prefix(PROPUESTA)?.trim();
        let origen = origen.trim_matches(|c: char| c == '`' || c == '"' || c == '\'');
        (!origen.is_empty()).then(|| origen.to_string())
    })
}

/// Quita del texto lo que era para vsioncode y no para leerse.
///
/// Las dos marcas, y no solo la petición: el usuario no tiene por qué ver
/// `SKILL-DE: usuario/repo` debajo de la explicación cuando el comando ya está
/// escrito en su línea, diciendo lo mismo y mejor.
pub fn sin_peticion(texto: &str) -> String {
    texto
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.starts_with(PETICION) && !t.starts_with(PROPUESTA)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Busca una skill por nombre, sin distinguir mayúsculas.
pub fn buscar<'a>(skills: &'a [Skill], nombre: &str) -> Option<&'a Skill> {
    let n = nombre.trim().to_ascii_lowercase();
    skills.iter().find(|s| s.nombre.to_ascii_lowercase() == n)
}

/// Lee el cuerpo de una skill para mandárselo al modelo.
///
/// Se recorta el encabezado YAML: ya se le enseñó en el catálogo y aquí solo
/// sería ruido. Lo que se devuelve es el documento, sin envolver ni resumir:
/// el valor de una skill está en las instrucciones que le escribió su autor.
pub fn leer(skill: &Skill) -> std::io::Result<String> {
    let texto = std::fs::read_to_string(&skill.ruta)?;
    let cuerpo = sin_frontmatter(&texto);

    let mut salida = cuerpo.trim().to_string();
    if salida.len() > MAX_CUERPO {
        // Se corta en frontera de carácter: cortar a mitad de un UTF-8
        // multibyte entra en pánico.
        let mut fin = MAX_CUERPO;
        while fin > 0 && !salida.is_char_boundary(fin) {
            fin -= 1;
        }
        salida.truncate(fin);
        salida.push_str("\n\n[…la skill sigue, pero no cabe entera.]");
    }
    Ok(salida)
}

fn sin_frontmatter(texto: &str) -> &str {
    let Some(resto) = texto.strip_prefix("---") else {
        return texto;
    };
    match resto.find("\n---") {
        Some(i) => &resto[i + 4..],
        None => texto,
    }
}

/// El mensaje con el que la skill entra en la conversación.
pub fn inyectar(skill: &Skill, cuerpo: &str) -> String {
    format!(
        "Aquí tienes la skill «{}» que has pedido. Son instrucciones de cómo hacer bien \
         este tipo de tarea; síguelas para lo que estabas haciendo.\n\n\
         --- inicio de la skill ---\n{cuerpo}\n--- fin de la skill ---\n\n\
         Sigue con la tarea. Si necesitas ejecutar algo, dalo en un bloque bash; los \
         scripts que mencione la skill NO se han ejecutado.",
        skill.nombre
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_un_encabezado_de_una_linea() {
        let c = frontmatter("---\nname: foo\ndescription: hace cosas\n---\n# Foo\n");
        assert_eq!(c.get("name").unwrap(), "foo");
        assert_eq!(c.get("description").unwrap(), "hace cosas");
    }

    /// El formato real: media docena de las skills instaladas usan `|`.
    #[test]
    fn lee_una_descripcion_en_bloque() {
        let c = frontmatter(
            "---\nname: ratatui-tui\ndescription: |\n  Build terminal UIs.\n  Use when: (1) apps.\n---\n# Hola\n",
        );
        assert_eq!(c.get("name").unwrap(), "ratatui-tui");
        assert_eq!(
            c.get("description").unwrap(),
            "Build terminal UIs. Use when: (1) apps."
        );
    }

    #[test]
    fn un_fichero_sin_encabezado_no_revienta() {
        assert!(frontmatter("# Solo un título\n").is_empty());
        assert!(frontmatter("").is_empty());
    }

    #[test]
    fn quita_las_comillas() {
        let c = frontmatter("---\nname: \"con comillas\"\n---\n");
        assert_eq!(c.get("name").unwrap(), "con comillas");
    }

    #[test]
    fn el_cuerpo_va_sin_encabezado() {
        let t = "---\nname: x\ndescription: y\n---\n# Título\nCuerpo.\n";
        assert_eq!(sin_frontmatter(t).trim(), "# Título\nCuerpo.");
        assert_eq!(
            sin_frontmatter("# Sin encabezado").trim(),
            "# Sin encabezado"
        );
    }

    /// La petición se reconoce por la línea entera, como la de modo libre.
    /// A media frase, explicar qué es cargaría una skill que nadie quiso.
    #[test]
    fn reconoce_la_peticion() {
        assert_eq!(
            pedida("Esto va de terminales.\nNECESITO-SKILL: tui-design\nSigo."),
            Some("tui-design".into())
        );
        assert_eq!(pedida("  NECESITO-SKILL: foo  "), Some("foo".into()));
        assert_eq!(pedida("NECESITO-SKILL: `foo`"), Some("foo".into()));
    }

    #[test]
    fn no_la_confunde_con_hablar_de_ella() {
        assert!(pedida("puedes escribir NECESITO-SKILL: nombre para pedir una").is_none());
        assert!(pedida("NECESITO-SKILL:").is_none());
        assert!(pedida("una skill de diseño me vendría bien").is_none());
    }

    #[test]
    fn la_peticion_no_se_le_ensena_al_usuario() {
        let limpio = sin_peticion("Voy a mirarlo.\nNECESITO-SKILL: tui-design\nUn momento.");
        assert!(!limpio.contains("NECESITO"));
        assert!(limpio.contains("Voy a mirarlo."));
        assert!(limpio.contains("Un momento."));
    }

    #[test]
    fn sin_skills_no_hay_catalogo() {
        assert!(catalogo(&[]).is_empty());
    }

    #[test]
    fn el_catalogo_lleva_nombre_y_como_pedirla() {
        let s = vec![Skill {
            nombre: "tui-design".into(),
            descripcion: "Diseñar interfaces de terminal.".into(),
            ruta: PathBuf::from("/x/SKILL.md"),
        }];
        let c = catalogo(&s);
        assert!(c.contains("tui-design: Diseñar interfaces de terminal."));
        assert!(c.contains(PETICION));
    }

    /// Cada carácter del catálogo se paga en cada turno, y las descripciones
    /// reales rondan los 600.
    #[test]
    fn recorta_las_descripciones_largas() {
        let larga = "palabra ".repeat(100);
        let r = recortar(&larga);
        assert!(r.chars().count() <= MAX_DESCRIPCION + 1, "{}", r.len());
        assert!(r.ends_with('…'));
        assert!(!r.contains("  "));
    }

    #[test]
    fn buscar_no_distingue_mayusculas() {
        let s = vec![Skill {
            nombre: "TUI-Design".into(),
            descripcion: String::new(),
            ruta: PathBuf::from("/x/SKILL.md"),
        }];
        assert!(buscar(&s, "tui-design").is_some());
        assert!(buscar(&s, "otra").is_none());
    }

    /// Lo que se inyecta tiene que dejar claro que nada se ha ejecutado: una
    /// skill llena de bloques de comandos no es una skill ya aplicada.
    #[test]
    fn el_mensaje_dice_que_no_se_ha_ejecutado_nada() {
        let s = Skill {
            nombre: "x".into(),
            descripcion: String::new(),
            ruta: PathBuf::from("/x/SKILL.md"),
        };
        let m = inyectar(&s, "cuerpo");
        assert!(m.contains("NO se han ejecutado"));
        assert!(m.contains("cuerpo"));
    }

    fn instalar(o: &str) -> String {
        comando_instalar(o, Path::new("/casa/skills")).expect("origen válido")
    }

    #[test]
    fn entiende_el_atajo_de_github() {
        let c = instalar("emilkowalski/skills");
        assert!(
            c.contains("git clone --depth 1 --quiet 'https://github.com/emilkowalski/skills.git'")
        );
        // Sin ruta concreta se traen todas las que tenga el repositorio.
        assert!(c.contains("-name SKILL.md"));
        assert!(c.contains("rm -rf \"$tmp\""));
    }

    /// La URL que sale al copiar la barra del navegador, que es como lo va a
    /// hacer cualquiera.
    #[test]
    fn entiende_la_url_de_una_carpeta_del_navegador() {
        let c = instalar("https://github.com/hyperb1iss/hyperskills/tree/main/skills/tui-design");
        assert!(c.contains("--branch 'main'"));
        assert!(c.contains("cp -r \"$tmp\"/'skills/tui-design' '/casa/skills'/"));
        assert!(!c.contains("-name SKILL.md"), "con ruta no se traen todas");
    }

    #[test]
    fn entiende_un_skill_md_suelto() {
        let c = instalar("https://ejemplo.org/x/tui-design/SKILL.md");
        assert!(c.contains("mkdir -p '/casa/skills'/tui-design"));
        assert!(c.contains("curl -fsSL 'https://ejemplo.org/x/tui-design/SKILL.md'"));
    }

    /// Con una carpeta del disco no se sabe de antemano si es *la* skill o
    /// el armario que las contiene, y el caso frecuente es el segundo:
    /// apuntar a `~/.claude/skills` para traerse lo que ya se tenía. El mismo
    /// comando tiene que servir para los dos, o el que apunta al armario se
    /// queda sin nada y sin aviso.
    #[test]
    fn de_una_carpeta_del_disco_saca_la_skill_este_donde_este() {
        let c = instalar("/tmp/mi-skill");
        assert!(
            c.contains("find '/tmp/mi-skill' -maxdepth 2 -name SKILL.md"),
            "{c}"
        );
        assert!(c.contains("cp -r @ '/casa/skills'/"), "{c}");
        assert!(!c.contains("cp -r '/tmp/mi-skill'"), "copiada a pelo: {c}");
    }

    /// Instalar se ve al reiniciar y no antes, así que el comando termina
    /// enseñando el armario: sin eso no hay forma de saber si llegó algo.
    #[test]
    fn el_comando_ensena_el_armario_al_terminar() {
        for o in [
            "/tmp/mi-skill",
            "usuario/repo",
            "https://ejemplo.org/x/foo/SKILL.md",
        ] {
            let c = instalar(o);
            assert!(c.trim_end().ends_with("ls -1 '/casa/skills'"), "{o}: {c}");
        }
    }

    /// Lo que viene de fuera no se pega crudo en una línea de shell: una
    /// comilla suelta en el origen cerraría la cadena y lo de después sería un
    /// comando más. Entrecomillado, eso es un nombre de carpeta feo y nada más.
    #[test]
    fn entrecomilla_lo_que_viene_de_fuera() {
        let c = instalar("/tmp/x'; rm -rf ~; echo '");
        assert!(
            c.contains(r"'/tmp/x'\''; rm -rf ~; echo '\'''"),
            "la comilla se escapó: {c}"
        );
    }

    #[test]
    fn se_queja_de_lo_que_no_entiende() {
        assert!(comando_instalar("", Path::new("/x")).is_err());
        assert!(comando_instalar("no es nada", Path::new("/x")).is_err());
    }

    /// Lo que separa «tráete esto» de «búscame algo así». Si esto se equivoca,
    /// o se sale a internet teniendo la ruta delante, o se contesta con un
    /// error a quien estaba describiendo lo que quiere.
    #[test]
    fn distingue_un_origen_de_una_descripcion() {
        for si in [
            "emilkowalski/skills",
            "https://github.com/u/r/tree/main/skills/x",
            "https://ejemplo.org/x/foo/SKILL.md",
            "~/skills",
            "/tmp/mi-skill",
            "./skills",
        ] {
            assert!(es_origen(si), "«{si}» es un sitio de donde traerlo");
        }
        for no in [
            "skills de anthropic para diseño",
            "algo que sepa de terminales",
            "busca una skill de rust",
            "",
        ] {
            assert!(!es_origen(no), "«{no}» es lo que quieres, no dónde está");
        }
    }

    /// Un origen puede traer espacios detrás —una URL con ruta larga—, así que
    /// esta marca no puede exigir una sola palabra como la de pedir una skill.
    #[test]
    fn recoge_de_donde_dice_el_modelo() {
        assert_eq!(
            propuesta("La de Emil vale.\nSKILL-DE: emilkowalski/skills\n"),
            Some("emilkowalski/skills".into())
        );
        assert_eq!(
            propuesta("SKILL-DE: `https://github.com/u/r/tree/main/x`"),
            Some("https://github.com/u/r/tree/main/x".into())
        );
        assert!(propuesta("SKILL-DE:").is_none());
        assert!(propuesta("no encontré ninguna que encaje").is_none());
    }

    /// El comando ya está escrito en la línea diciendo de dónde viene: la
    /// marca debajo sería lo mismo dicho peor.
    #[test]
    fn la_propuesta_tampoco_se_le_ensena_al_usuario() {
        let limpio = sin_peticion("Esta es la buena.\nSKILL-DE: u/r\nMírala antes.");
        assert!(!limpio.contains("SKILL-DE"));
        assert!(limpio.contains("Esta es la buena."));
        assert!(limpio.contains("Mírala antes."));
    }

    /// Las de Claude Code son de Claude Code: aquí no se miran.
    #[test]
    fn no_mira_el_armario_de_otro_programa() {
        let dirs = directorios(Path::new("/proyecto"));
        assert!(
            !dirs.iter().any(|d| d.to_string_lossy().contains(".claude")
                || d.to_string_lossy().contains(".agents")),
            "{dirs:?}"
        );
        assert_eq!(dirs[0], PathBuf::from("/proyecto/.vsioncode/skills"));
    }
}
