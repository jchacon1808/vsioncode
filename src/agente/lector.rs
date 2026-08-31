//! Las tres herramientas de un subagente, escritas aquí.
//!
//! Un subagente de `claude` recibe `Read`, `Grep` y `Glob` del propio proceso.
//! Uno servido por HTTP no recibe nada: la API de chat solo sabe de texto. Sin
//! esto, un subagente de Groq o de NVIDIA solo podría opinar sobre lo que ya
//! supiera, que es justo lo contrario de para lo que existe un subagente
//! —leerse el proyecto para no ensuciar la ventana de quien pregunta—.
//!
//! ## Por qué texto y no llamada a herramientas
//!
//! Por lo mismo que el cerebro propone comandos en bloques ```` ```bash ````
//! y no con `tool_calls`: ahí es donde los proveedores se diferencian y donde
//! los modelos pequeños fallan. Un protocolo de líneas lo cumple cualquiera,
//! incluido un modelo local, y cuando se equivoca se ve en el texto en vez de
//! romper la petición entera con un 400.
//!
//! ## Qué NO puede hacer
//!
//! Ejecutar, escribir, salir del directorio de trabajo o leer un fichero
//! enorme entero. El invariante de vsioncode es el mismo aquí que en el
//! `claude -p`: un subagente lee y nada más. Y como estas herramientas las
//! escribimos nosotros, la promesa no depende de que otro proceso la respete.

use std::path::{Path, PathBuf};

/// Cuánto se le deja leer de una vez.
///
/// Lo que vuelve va a la ventana del subagente y de ahí, resumido, a la del
/// principal. Sin tope, un `LEER Cargo.lock` se come el contexto entero y el
/// informe sale peor por tener más material, no mejor.
///
/// Los números salieron de la primera prueba real, y no de una estimación: con
/// 400 líneas y 40 KB, un `LEER src/agente.rs` mandaba once mil tokens y Groq
/// devolvía **413** —el límite del plan gratuito son ocho mil por minuto—. Un
/// tope generoso aquí no es generosidad: es un subagente que no responde.
const MAX_LINEAS: usize = 200;
const MAX_BYTES: usize = 12 * 1024;
/// Coincidencias de una búsqueda. Más que esto no es un hallazgo, es ruido.
const MAX_HALLAZGOS: usize = 40;
/// Ficheros que enseña un listado.
const MAX_ENTRADAS: usize = 120;
/// Ficheros que se abren en una búsqueda antes de rendirse.
const MAX_FICHEROS: usize = 4_000;
/// Un fichero más grande que esto no se busca: o es un binario o es un volcado.
const MAX_FICHERO_BYTES: u64 = 2 * 1024 * 1024;

/// Carpetas que no se miran nunca.
///
/// No es una preferencia de estilo: `target/` y `node_modules/` tienen más
/// ficheros que el proyecto entero, y una búsqueda que los recorra tarda
/// segundos y devuelve coincidencias de código ajeno.
const IGNORADAS: &[&str] = &[".git", "target", "node_modules", ".venv", "__pycache__"];

/// Lo que el subagente pide, ya entendido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peticion {
    /// `LEER ruta` o `LEER ruta:120-260`.
    Leer {
        ruta: String,
        desde: Option<usize>,
        hasta: Option<usize>,
    },
    /// `BUSCAR texto` o `BUSCAR texto EN ruta`.
    Buscar {
        texto: String,
        donde: Option<String>,
    },
    /// `LISTAR ruta` o `LISTAR *.rs`.
    Listar { patron: String },
}

/// Saca las peticiones de lo que ha respondido el subagente.
///
/// Se aceptan dentro y fuera de una valla de código: un modelo pequeño mete
/// las órdenes en un bloque la mitad de las veces, y rechazarlas por eso sería
/// gastar una ronda entera en enseñarle un formalismo que no cambia nada.
///
/// El verbo tiene que abrir la línea. Sin esa condición, una frase como «voy a
/// leer src/app.rs» se convertiría en una petición, y entonces el subagente no
/// podría hablar de leer sin leer.
pub fn peticiones(texto: &str) -> Vec<Peticion> {
    let mut v = Vec::new();
    for linea in texto.lines() {
        let l = linea.trim().trim_start_matches(['-', '*', '`', ' ']).trim();
        let Some((verbo, resto)) = l.split_once(char::is_whitespace) else {
            continue;
        };
        let resto = resto.trim().trim_end_matches('`').trim();
        if resto.is_empty() {
            continue;
        }
        match verbo.to_ascii_uppercase().as_str() {
            "LEER" => v.push(rango(resto)),
            "BUSCAR" => {
                let (texto, donde) = match partir_en(resto) {
                    Some((t, d)) => (t.to_string(), Some(d.to_string())),
                    None => (resto.to_string(), None),
                };
                v.push(Peticion::Buscar { texto, donde });
            }
            "LISTAR" => v.push(Peticion::Listar {
                patron: resto.to_string(),
            }),
            _ => {}
        }
        if v.len() >= MAX_PETICIONES {
            break;
        }
    }
    v
}

/// Cuántas se atienden en una misma respuesta.
///
/// Pedir diez cosas de golpe no es explorar, es volcar el proyecto en la
/// ventana. Con cuatro caben las que de verdad van juntas —un fichero y los
/// dos sitios donde se usa— y lo demás pasa a la ronda siguiente.
pub const MAX_PETICIONES: usize = 4;

/// `ruta:120-260`, `ruta:120-` o `ruta` a secas.
fn rango(resto: &str) -> Peticion {
    let Some((ruta, r)) = resto.rsplit_once(':') else {
        return Peticion::Leer {
            ruta: resto.to_string(),
            desde: None,
            hasta: None,
        };
    };
    let (d, h) = match r.split_once('-') {
        Some((a, b)) => (a.trim().parse().ok(), b.trim().parse().ok()),
        None => (r.trim().parse().ok(), None),
    };
    if d.is_none() {
        // No era un rango: era una ruta con dos puntos dentro.
        return Peticion::Leer {
            ruta: resto.to_string(),
            desde: None,
            hasta: None,
        };
    }
    Peticion::Leer {
        ruta: ruta.to_string(),
        desde: d,
        hasta: h,
    }
}

/// Parte por el « EN » que separa qué se busca de dónde.
fn partir_en(resto: &str) -> Option<(&str, &str)> {
    let bajo = resto.to_ascii_lowercase();
    let pos = bajo.rfind(" en ")?;
    let (t, d) = resto.split_at(pos);
    Some((t.trim(), d[4..].trim()))
}

/// Lee del disco, y solo del disco que le toca.
pub struct Lector {
    raiz: PathBuf,
}

impl Lector {
    /// El directorio de trabajo es la frontera. Fuera de él no se lee nada.
    pub fn nuevo(cwd: &str) -> Self {
        Self {
            raiz: std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd)),
        }
    }

    /// Atiende una petición y devuelve lo que se le va a enseñar al modelo.
    ///
    /// Nunca falla hacia arriba: un error es también una respuesta útil —«ese
    /// fichero no existe» le ahorra al subagente inventarse su contenido— y
    /// cortar la ronda por una ruta mal escrita desperdiciaría la llamada.
    pub fn atender(&self, p: &Peticion) -> String {
        match p {
            Peticion::Leer { ruta, desde, hasta } => self.leer(ruta, *desde, *hasta),
            Peticion::Buscar { texto, donde } => self.buscar(texto, donde.as_deref()),
            Peticion::Listar { patron } => self.listar(patron),
        }
    }

    /// Resuelve una ruta relativa sin dejar salir de la raíz.
    ///
    /// Con `canonicalize`, que es lo único que descarta un `..` de verdad:
    /// comprobar el texto de la ruta deja pasar enlaces simbólicos y rutas que
    /// suben y vuelven a bajar.
    fn dentro(&self, ruta: &str) -> Result<PathBuf, String> {
        let p = Path::new(ruta);
        let completa = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.raiz.join(p)
        };
        let real = std::fs::canonicalize(&completa)
            .map_err(|_| format!("«{ruta}» no existe (o no puedo abrirlo)."))?;
        if !real.starts_with(&self.raiz) {
            return Err(format!(
                "«{ruta}» está fuera del directorio de trabajo. Solo puedo leer dentro."
            ));
        }
        Ok(real)
    }

    /// Cómo se nombra un fichero al enseñarlo: relativo, como en un `grep`.
    fn corta(&self, p: &Path) -> String {
        p.strip_prefix(&self.raiz)
            .unwrap_or(p)
            .display()
            .to_string()
    }

    fn leer(&self, ruta: &str, desde: Option<usize>, hasta: Option<usize>) -> String {
        let real = match self.dentro(ruta) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let Ok(texto) = std::fs::read_to_string(&real) else {
            return format!("«{ruta}» no es texto legible.");
        };

        let total = texto.lines().count();
        let desde = desde.unwrap_or(1).max(1);
        let hasta = hasta.unwrap_or(desde + MAX_LINEAS - 1).min(total);
        let hasta = hasta.min(desde + MAX_LINEAS - 1);

        let mut salida = String::new();
        let mut bytes = 0;
        let mut ultima = desde;
        for (n, l) in texto.lines().enumerate().skip(desde.saturating_sub(1)) {
            let n = n + 1;
            if n > hasta {
                break;
            }
            bytes += l.len();
            if bytes > MAX_BYTES {
                break;
            }
            salida.push_str(&format!("{n:>5}  {l}\n"));
            ultima = n;
        }

        // Que el corte se vea. Un fichero truncado en silencio es la forma más
        // fácil de que un subagente concluya que algo «no está en el código».
        let cola = if ultima < total {
            format!(
                "\n… hasta aquí. {ruta} tiene {total} líneas; sigue con LEER {ruta}:{}-",
                ultima + 1
            )
        } else {
            String::new()
        };
        format!("--- {} ({total} líneas)\n{salida}{cola}", self.corta(&real))
    }

    fn buscar(&self, texto: &str, donde: Option<&str>) -> String {
        let raiz = match donde {
            Some(d) => match self.dentro(d) {
                Ok(r) => r,
                Err(e) => return e,
            },
            None => self.raiz.clone(),
        };
        let aguja = texto.to_ascii_lowercase();
        let mut hallazgos = Vec::new();

        for f in self.ficheros(&raiz) {
            if hallazgos.len() >= MAX_HALLAZGOS {
                break;
            }
            let Ok(contenido) = std::fs::read_to_string(&f) else {
                continue;
            };
            for (n, l) in contenido.lines().enumerate() {
                if l.to_ascii_lowercase().contains(&aguja) {
                    let l = l.trim();
                    let corte: String = l.chars().take(160).collect();
                    hallazgos.push(format!("{}:{}: {corte}", self.corta(&f), n + 1));
                    if hallazgos.len() >= MAX_HALLAZGOS {
                        break;
                    }
                }
            }
        }

        if hallazgos.is_empty() {
            return format!("«{texto}»: ninguna coincidencia.");
        }
        format!(
            "«{texto}» — {} coincidencia(s){}:\n{}",
            hallazgos.len(),
            if hallazgos.len() >= MAX_HALLAZGOS {
                " (hay más; afina la búsqueda)"
            } else {
                ""
            },
            hallazgos.join("\n")
        )
    }

    /// Lista lo que hay. El patrón es una carpeta, o un `*.ext`, o un trozo de
    /// nombre: no hay glob completo porque no hace falta ninguno.
    fn listar(&self, patron: &str) -> String {
        let (raiz, filtro) = match patron.rsplit_once('/') {
            Some((d, f)) if f.contains('*') || f.is_empty() => (d.to_string(), f.to_string()),
            _ if patron.contains('*') => (".".to_string(), patron.to_string()),
            _ => (patron.to_string(), String::new()),
        };
        let raiz = match self.dentro(if raiz.is_empty() { "." } else { &raiz }) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let sufijo = filtro.trim_start_matches('*').to_ascii_lowercase();

        let mut v: Vec<String> = Vec::new();
        for f in self.ficheros(&raiz) {
            if !sufijo.is_empty() && !f.to_string_lossy().to_ascii_lowercase().ends_with(&sufijo) {
                continue;
            }
            let tam = f.metadata().map(|m| m.len()).unwrap_or(0);
            v.push(format!("{}  ({tam} B)", self.corta(&f)));
            if v.len() >= MAX_ENTRADAS {
                break;
            }
        }
        if v.is_empty() {
            return format!("«{patron}»: nada que listar.");
        }
        format!("{} fichero(s):\n{}", v.len(), v.join("\n"))
    }

    /// Los ficheros de un árbol, sin lo que no se mira nunca.
    ///
    /// Recursivo a mano y no con un crate: es media pantalla de código y el
    /// binario de vsioncode es una característica declarada, no un residuo.
    fn ficheros(&self, raiz: &Path) -> Vec<PathBuf> {
        if raiz.is_file() {
            return vec![raiz.to_path_buf()];
        }
        let mut pendientes = vec![raiz.to_path_buf()];
        let mut v = Vec::new();
        while let Some(d) = pendientes.pop() {
            let Ok(entradas) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entradas.flatten() {
                let p = e.path();
                let nombre = e.file_name().to_string_lossy().to_string();
                if nombre.starts_with('.') || IGNORADAS.contains(&nombre.as_str()) {
                    continue;
                }
                match e.file_type() {
                    Ok(t) if t.is_dir() => pendientes.push(p),
                    Ok(t)
                        if t.is_file()
                            && e.metadata().map(|m| m.len()).unwrap_or(0) <= MAX_FICHERO_BYTES =>
                    {
                        v.push(p);
                    }
                    _ => {}
                }
                if v.len() >= MAX_FICHEROS {
                    return v;
                }
            }
        }
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entiende_las_tres_ordenes() {
        let p = peticiones("Voy a mirarlo.\nLEER src/app.rs\nBUSCAR techo EN src/\nLISTAR *.rs");
        assert_eq!(
            p,
            vec![
                Peticion::Leer {
                    ruta: "src/app.rs".into(),
                    desde: None,
                    hasta: None
                },
                Peticion::Buscar {
                    texto: "techo".into(),
                    donde: Some("src/".into())
                },
                Peticion::Listar {
                    patron: "*.rs".into()
                },
            ]
        );
    }

    /// Dentro de una valla también valen: los modelos pequeños las meten ahí
    /// la mitad de las veces y perder una ronda por eso sale caro.
    #[test]
    fn valen_dentro_de_un_bloque() {
        let p = peticiones("```\nLEER src/voz.rs:10-40\n```");
        assert_eq!(
            p,
            vec![Peticion::Leer {
                ruta: "src/voz.rs".into(),
                desde: Some(10),
                hasta: Some(40)
            }]
        );
    }

    /// Hablar de leer no es leer. Sin esto, el subagente no podría escribir su
    /// informe sin disparar otra ronda.
    #[test]
    fn una_frase_no_es_una_orden() {
        assert!(peticiones("voy a leer src/app.rs y luego buscar el techo").is_empty());
        assert!(peticiones("El informe: no encontré nada.").is_empty());
    }

    /// Una ruta con dos puntos no se confunde con un rango.
    #[test]
    fn los_dos_puntos_solo_son_rango_si_llevan_numero() {
        assert_eq!(
            peticiones("LEER src/a:b.rs"),
            vec![Peticion::Leer {
                ruta: "src/a:b.rs".into(),
                desde: None,
                hasta: None
            }]
        );
    }

    /// Un fichero del sistema que existe seguro y está fuera del proyecto. La
    /// prueba usaba `/etc/passwd` a secas y en Windows no existe: allí el
    /// lector contestaba «no existe» —que es cierto— en vez de «fuera del
    /// directorio», y la prueba fallaba sin que la defensa tuviera nada malo.
    /// Para comprobar que **no se sale**, el fichero de fuera tiene que estar.
    #[cfg(windows)]
    const FUERA: &str = r"C:\Windows\win.ini";
    #[cfg(not(windows))]
    const FUERA: &str = "/etc/passwd";

    #[test]
    fn no_se_sale_del_directorio() {
        let l = Lector::nuevo(env!("CARGO_MANIFEST_DIR"));
        let fuera = l.atender(&Peticion::Leer {
            ruta: FUERA.into(),
            desde: None,
            hasta: None,
        });
        assert!(fuera.contains("fuera del directorio"), "{fuera}");
        let arriba = l.atender(&Peticion::Leer {
            ruta: "../../../etc/hosts".into(),
            desde: None,
            hasta: None,
        });
        assert!(
            arriba.contains("fuera del directorio") || arriba.contains("no existe"),
            "{arriba}"
        );
    }

    #[test]
    fn lee_un_trozo_y_dice_que_hay_mas() {
        let l = Lector::nuevo(env!("CARGO_MANIFEST_DIR"));
        let t = l.atender(&Peticion::Leer {
            ruta: "src/agente/lector.rs".into(),
            desde: Some(1),
            hasta: Some(5),
        });
        assert!(t.contains("    1  //!"), "{t}");
        assert!(t.contains("sigue con LEER"), "{t}");
    }

    #[test]
    fn busca_dentro_del_proyecto() {
        let l = Lector::nuevo(env!("CARGO_MANIFEST_DIR"));
        let t = l.atender(&Peticion::Buscar {
            texto: "MAX_HALLAZGOS".into(),
            donde: Some("src/agente".into()),
        });
        assert!(t.contains("lector.rs:"), "{t}");
    }
}
