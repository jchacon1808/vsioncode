//! Las dos formas de nombrar un fichero cuando el shell es Git Bash.
//!
//! Git Bash vive en el sistema de ficheros virtual de MSYS2: dice
//! `/c/Users/juan/proyecto` donde Windows dice `C:\Users\juan\proyecto`.
//! vsioncode es un binario **nativo**, así que `std::fs` solo entiende la
//! segunda forma, y todo lo que llega del shell —el marcador `cwd`, un fichero
//! soltado en el panel, la ruta que escribe el modelo mirando esa `cwd`— llega
//! en la primera.
//!
//! Las dos funciones se aplican **en la frontera**: lo que entra del shell se
//! traduce una vez al entrar y lo que sale hacia él una vez al salir. Dentro
//! solo hay rutas nativas. Repartir la traducción por dentro es cómo se acaba
//! traduciendo dos veces, que en estas rutas no es idempotente.
//!
//! No se usa `cygpath`. Existe y hace justo esto, pero es un proceso por
//! llamada y por aquí pasa cada comando que se ejecuta.
//!
//! Fuera de Windows no hay nada que traducir y las dos devuelven lo que
//! reciben. La decisión es `cfg!` y no `#[cfg]` a propósito: así la conversión
//! se compila —y se prueba— en los dos sistemas, que es lo único que permite
//! sujetar esto con tests que corren en Linux.

use std::borrow::Cow;

/// De lo que dice el shell a lo que entiende `std::fs`.
pub fn a_windows(ruta: &str) -> Cow<'_, str> {
    if cfg!(windows) {
        convertir_a_windows(ruta, casa().as_deref())
    } else {
        Cow::Borrowed(ruta)
    }
}

/// De lo que entiende `std::fs` a lo que espera el shell.
pub fn a_posix(ruta: &str) -> Cow<'_, str> {
    if cfg!(windows) {
        convertir_a_posix(ruta)
    } else {
        Cow::Borrowed(ruta)
    }
}

/// ¿Está este programa donde el sistema busca los suyos?
///
/// La tercera frontera, y la que menos lo parece. Recorrer el `PATH` a mano
/// —que es lo que hace `which`— tiene dos trampas en Windows y las dos dan la
/// misma respuesta equivocada, «no está»:
///
/// - **El separador no es `:`.** Allí es `;`, y con `:` cada `C:\...` se parte
///   en dos trozos que no existen. Se usa `split_paths`, que ya lo sabe.
/// - **El nombre no lleva la extensión.** `claude` es `claude.exe` o
///   `claude.cmd`, y `join("claude")` no encuentra ninguno de los dos. Las
///   extensiones que cuentan las declara `%PATHEXT%`.
///
/// Importa porque de esta respuesta cuelgan decisiones que se ven: si se
/// ofrecen subagentes de suscripción, y si la pantalla de arranque dice que
/// falta Claude Code. Contestar «no está» sobre algo que sí está es el mismo
/// agujero negro que arrancar sin cerebro, visto desde dentro.
pub fn en_el_path(programa: &str) -> bool {
    // Con una barra dentro no es un nombre, es una ruta: se mira donde dice.
    if programa.contains('/') || (cfg!(windows) && programa.contains('\\')) {
        return std::path::Path::new(programa).is_file();
    }

    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let extensiones = extensiones_ejecutables();

    std::env::split_paths(&path).any(|d| {
        let base = d.join(programa);
        base.is_file() || extensiones.iter().any(|e| base.with_extension(e).is_file())
    })
}

/// Las extensiones que hacen ejecutable a un fichero. Fuera de Windows,
/// ninguna: ahí lo que manda es el bit de ejecución, no el nombre.
fn extensiones_ejecutables() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter_map(|e| {
            let e = e.trim().trim_start_matches('.');
            (!e.is_empty()).then(|| e.to_ascii_lowercase())
        })
        .collect()
}

fn casa() -> Option<String> {
    dirs::home_dir().map(|p| p.display().to_string())
}

/// Una ruta que empieza por `\\` o por `//` es UNC —un recurso de red, o el
/// `\\wsl$\Ubuntu\...` de una distribución— y no es de nadie de aquí. Se deja
/// pasar entera: traducirla la rompería, y no traducirla la deja funcionando.
fn es_unc(ruta: &str) -> bool {
    ruta.starts_with("\\\\") || ruta.starts_with("//")
}

/// `/c/...` → `C:\...`, y `~` → `%USERPROFILE%`.
///
/// Lo que no se reconoce se devuelve tal cual. Es deliberado: `/tmp` y `/usr`
/// en Git Bash cuelgan del directorio donde se instaló Git, que desde aquí no
/// se sabe, y una traducción inventada apunta a un fichero equivocado sin
/// avisar. Mejor una ruta que falla al abrirse que una que abre otra cosa.
fn convertir_a_windows<'a>(ruta: &'a str, casa: Option<&str>) -> Cow<'a, str> {
    if ruta.is_empty() || es_unc(ruta) {
        return Cow::Borrowed(ruta);
    }

    // Ya es nativa: solo se uniforman las barras.
    if letra_de_unidad(ruta).is_some() {
        return barras(ruta, '/', '\\');
    }

    if let Some(resto) = ruta.strip_prefix('~')
        && (resto.is_empty() || resto.starts_with('/') || resto.starts_with('\\'))
    {
        let Some(casa) = casa else {
            return Cow::Borrowed(ruta);
        };
        let cola = barras(resto, '/', '\\');
        return Cow::Owned(format!("{}{}", casa.trim_end_matches('\\'), cola));
    }

    if let Some((unidad, resto)) = unidad_msys(ruta) {
        let cola = barras(resto, '/', '\\');
        return Cow::Owned(format!("{}:\\{}", unidad, cola.trim_start_matches('\\')));
    }

    Cow::Borrowed(ruta)
}

/// `C:\...` → `/c/...`. Una relativa solo cambia de barras.
fn convertir_a_posix(ruta: &str) -> Cow<'_, str> {
    if ruta.is_empty() || es_unc(ruta) {
        return Cow::Borrowed(ruta);
    }

    if let Some(unidad) = letra_de_unidad(ruta) {
        let resto = barras(&ruta[2..], '\\', '/');
        return Cow::Owned(format!(
            "/{}/{}",
            unidad.to_ascii_lowercase(),
            resto.trim_start_matches('/')
        ));
    }

    barras(ruta, '\\', '/')
}

/// `C:` al principio, y solo si lo que sigue es una barra o el final. `C:foo`
/// es una ruta relativa a la unidad, otra cosa distinta y muy rara: se deja.
fn letra_de_unidad(ruta: &str) -> Option<char> {
    let mut c = ruta.chars();
    let letra = c.next().filter(|l| l.is_ascii_alphabetic())?;
    if c.next() != Some(':') {
        return None;
    }
    match c.next() {
        None | Some('/') | Some('\\') => Some(letra.to_ascii_uppercase()),
        _ => None,
    }
}

/// `/c` o `/c/loquesea`. Dos letras (`/cc/...`) no es una unidad: es un
/// directorio que se llama así.
fn unidad_msys(ruta: &str) -> Option<(char, &str)> {
    let resto = ruta.strip_prefix('/')?;
    let mut c = resto.chars();
    let letra = c.next().filter(|l| l.is_ascii_alphabetic())?;
    match c.next() {
        None => Some((letra.to_ascii_uppercase(), "")),
        Some('/') => Some((letra.to_ascii_uppercase(), &resto[2..])),
        _ => None,
    }
}

fn barras(s: &str, de: char, a: char) -> Cow<'_, str> {
    if s.contains(de) {
        Cow::Owned(s.replace(de, &a.to_string()))
    } else {
        Cow::Borrowed(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASA: Option<&str> = Some("C:\\Users\\juan");

    fn aw(r: &str) -> String {
        convertir_a_windows(r, CASA).into_owned()
    }

    fn ap(r: &str) -> String {
        convertir_a_posix(r).into_owned()
    }

    #[test]
    fn la_unidad_de_msys_se_convierte() {
        assert_eq!(aw("/c/Users/juan/proyecto"), "C:\\Users\\juan\\proyecto");
        assert_eq!(aw("/d/datos"), "D:\\datos");
        assert_eq!(aw("/c"), "C:\\");
        assert_eq!(aw("/c/"), "C:\\");
    }

    #[test]
    fn y_la_vuelta() {
        assert_eq!(ap("C:\\Users\\juan\\proyecto"), "/c/Users/juan/proyecto");
        assert_eq!(ap("C:/Users/juan"), "/c/Users/juan");
        assert_eq!(ap("C:\\"), "/c/");
    }

    /// Ir y volver tiene que devolver lo mismo, que es lo que evita que una
    /// ruta se degrade cuando pasa dos veces por la frontera.
    #[test]
    fn ida_y_vuelta() {
        for r in ["/c/Users/juan/proyecto", "/d/datos/x.rs", "/c/"] {
            assert_eq!(ap(&aw(r)), r);
        }
    }

    /// En Windows las rutas de usuario llevan espacios, y son la norma.
    #[test]
    fn los_espacios_no_estorban() {
        assert_eq!(
            aw("/c/Program Files/Git/bin/bash.exe"),
            "C:\\Program Files\\Git\\bin\\bash.exe"
        );
        assert_eq!(ap("C:\\Program Files\\Git"), "/c/Program Files/Git");
    }

    /// Una ruta de WSL o de red no es nuestra. Se deja intacta en los dos
    /// sentidos: traducirla la rompe, y no tocarla la deja funcionando.
    #[test]
    fn lo_unc_se_deja_en_paz() {
        for r in [
            "\\\\wsl$\\Ubuntu\\home\\juan",
            "\\\\servidor\\compartido",
            "//servidor/compartido",
        ] {
            assert_eq!(aw(r), r);
            assert_eq!(ap(r), r);
        }
    }

    #[test]
    fn la_virgulilla_apunta_al_perfil() {
        assert_eq!(aw("~"), "C:\\Users\\juan");
        assert_eq!(
            aw("~/.config/vsioncode"),
            "C:\\Users\\juan\\.config\\vsioncode"
        );
        // `~juan` es el home de otro usuario: no se sabe cuál, no se inventa.
        assert_eq!(aw("~juan/x"), "~juan/x");
    }

    /// Lo que no se reconoce se devuelve entero. La alternativa —adivinar— es
    /// la que abre el fichero equivocado sin decirlo.
    #[test]
    fn lo_que_no_se_sabe_traducir_se_devuelve_igual() {
        assert_eq!(aw("/tmp/x"), "/tmp/x");
        assert_eq!(aw("/usr/bin/vim"), "/usr/bin/vim");
        assert_eq!(aw("/cc/no-es-unidad"), "/cc/no-es-unidad");
        assert_eq!(aw("src/main.rs"), "src/main.rs");
        assert_eq!(aw(""), "");
    }

    /// Una nativa que entra por la frontera de entrada ya está bien: lo único
    /// que cambia son las barras.
    #[test]
    fn una_ruta_ya_nativa_sobrevive() {
        assert_eq!(aw("C:\\Users\\juan"), "C:\\Users\\juan");
        assert_eq!(aw("C:/Users/juan"), "C:\\Users\\juan");
        // `C:foo` es relativa a la unidad, no absoluta. No se toca.
        assert_eq!(aw("C:foo"), "C:foo");
        assert_eq!(ap("C:foo"), "C:foo");
    }

    /// Fuera de Windows esto no existe: cualquier ruta pasa entera, incluida
    /// una que en Linux es un nombre de fichero legal con barras invertidas.
    #[test]
    #[cfg(not(windows))]
    fn en_linux_no_se_traduce_nada() {
        assert_eq!(a_windows("/c/Users/juan"), "/c/Users/juan");
        assert_eq!(
            a_posix("/home/juan/raro\\nombre"),
            "/home/juan/raro\\nombre"
        );
    }
}
