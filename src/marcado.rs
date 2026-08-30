//! Markdown, lo justo para una terminal.
//!
//! El agente responde en markdown: `**negrita**`, `` `código` ``, bloques de
//! comando. Sin interpretarlo, en pantalla salen los asteriscos y las comillas
//! —feos y, peor, engañosos: parecen parte de lo que quiso decir—. Y el
//! resaltado que existía para distinguir un comando de una explicación se
//! perdía por completo.
//!
//! No es un renderizador de markdown. Es lo que de verdad aparece en las
//! respuestas de un agente de terminal, y nada más: negrita, código en línea,
//! bloques de comando y listas. Meter tablas, citas o enlaces sería añadir
//! superficie de fallo a cambio de algo que casi nunca llega.

/// Un trozo de línea, ya sabiendo cómo hay que pintarlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trozo {
    Normal(String),
    /// `**así**`.
    Fuerte(String),
    /// `` `así` `` — un comando, una ruta, un nombre de fichero.
    Codigo(String),
}

/// Parte una línea en trozos según su marcado.
///
/// Es deliberadamente tolerante: un asterisco suelto o una comilla sin cerrar
/// se quedan como texto normal en vez de comerse el resto de la línea. En una
/// respuesta generada eso pasa, y perder media frase por un carácter sería un
/// arreglo peor que el problema.
pub fn inline(linea: &str) -> Vec<Trozo> {
    let mut trozos = Vec::new();
    let mut normal = String::new();
    let bytes: Vec<char> = linea.chars().collect();
    let mut i = 0;

    while i < bytes.len() {
        // **negrita**
        if bytes[i] == '*'
            && i + 1 < bytes.len()
            && bytes[i + 1] == '*'
            && let Some(fin) = buscar(&bytes, i + 2, "**")
        {
            empujar(&mut trozos, &mut normal);
            trozos.push(Trozo::Fuerte(bytes[i + 2..fin].iter().collect()));
            i = fin + 2;
            continue;
        }
        // `código`
        if bytes[i] == '`'
            && let Some(fin) = buscar(&bytes, i + 1, "`")
            && fin > i + 1
        {
            empujar(&mut trozos, &mut normal);
            trozos.push(Trozo::Codigo(bytes[i + 1..fin].iter().collect()));
            i = fin + 1;
            continue;
        }
        normal.push(bytes[i]);
        i += 1;
    }

    empujar(&mut trozos, &mut normal);
    trozos
}

fn buscar(cs: &[char], desde: usize, marca: &str) -> Option<usize> {
    let m: Vec<char> = marca.chars().collect();
    let mut i = desde;
    while i + m.len() <= cs.len() {
        if cs[i..i + m.len()] == m[..] {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn empujar(trozos: &mut Vec<Trozo>, normal: &mut String) {
    if !normal.is_empty() {
        trozos.push(Trozo::Normal(std::mem::take(normal)));
    }
}

/// Cómo hay que tratar una línea entera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Linea {
    /// Texto con marcado en línea.
    Texto(Vec<Trozo>),
    /// Dentro de un bloque de comando: se pinta tal cual, sin interpretar
    /// nada. Ahí un asterisco es un asterisco y hay que verlo.
    Comando(String),
    /// Dentro de un bloque `diff`: una línea de un cambio, con su signo ya
    /// separado. No la escribe el modelo —la monta vsioncode al preparar una
    /// edición—, y por eso no se trata como comando: nadie va a ejecutarla.
    Cambio { signo: char, texto: String },
    /// La marca de apertura o cierre de un bloque, que no se pinta.
    Delimitador { abre: bool, lenguaje: String },
    /// Encabezado (`## así`).
    Titulo(String),
}

/// Estado para ir línea a línea sabiendo si estamos dentro de un bloque.
#[derive(Debug, Default)]
pub struct Lector {
    pub en_bloque: bool,
    /// El bloque abierto es un `diff`. Se guarda aparte de `en_bloque` porque
    /// el lenguaje solo viene en la valla que abre, y la decisión hay que
    /// tomarla en cada una de las líneas de dentro.
    pub en_diff: bool,
}

impl Lector {
    pub fn linea(&mut self, cruda: &str) -> Linea {
        let t = cruda.trim_start();

        // La valla no siempre abre línea: los modelos pequeños escriben
        // «ejecuta esto:```bash» de corrido. Si se exige que empiece la
        // línea, el bloque no se abre nunca y el comando se pinta como si
        // fuera prosa — o peor, la explicación entera se lee como comando.
        //
        // Lo que va delante de la valla se pierde a propósito: es media frase
        // de introducción, y recuperarla obligaría a devolver dos líneas desde
        // aquí para ganar «ejecuta esto:».
        let t = match t.find("```") {
            Some(0) | None => t,
            Some(i) => &t[i..],
        };

        if t.starts_with("```") {
            let abre = !self.en_bloque;
            let lenguaje = t.trim_start_matches('`').trim().to_string();
            self.en_bloque = abre;
            self.en_diff = abre && lenguaje == "diff";
            return Linea::Delimitador { abre, lenguaje };
        }

        if self.en_diff {
            let mut cs = cruda.chars();
            let signo = cs.next().unwrap_or(' ');
            return Linea::Cambio {
                signo,
                texto: cs.as_str().to_string(),
            };
        }

        if self.en_bloque {
            return Linea::Comando(cruda.to_string());
        }

        if let Some(resto) = t.strip_prefix("#### ").or_else(|| {
            t.strip_prefix("### ")
                .or_else(|| t.strip_prefix("## ").or_else(|| t.strip_prefix("# ")))
        }) {
            return Linea::Titulo(resto.to_string());
        }

        Linea::Texto(inline(cruda))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Trozo {
        Trozo::Normal(s.into())
    }

    /// El caso que rompía con modelos pequeños: la valla pegada al final de
    /// la frase. Sin esto el bloque no se abría y el comando no aparecía.
    #[test]
    fn abre_el_bloque_con_la_valla_pegada_al_texto() {
        let mut l = Lector::default();
        assert!(matches!(
            l.linea("Para ver la red, ejecuta:```bash"),
            Linea::Delimitador { abre: true, .. }
        ));
        assert!(matches!(l.linea("ip a"), Linea::Comando(c) if c == "ip a"));
    }

    #[test]
    fn el_texto_sin_marcado_se_queda_igual() {
        assert_eq!(inline("hola qué tal"), vec![n("hola qué tal")]);
    }

    /// El caso de la queja: en pantalla salían los asteriscos.
    #[test]
    fn saca_la_negrita() {
        assert_eq!(
            inline("**Ya tienes instaladas:** nmap"),
            vec![Trozo::Fuerte("Ya tienes instaladas:".into()), n(" nmap")]
        );
    }

    #[test]
    fn saca_el_codigo_en_linea() {
        assert_eq!(
            inline("sustituye a `netstat`, obsoleto"),
            vec![
                n("sustituye a "),
                Trozo::Codigo("netstat".into()),
                n(", obsoleto")
            ]
        );
    }

    #[test]
    fn los_mezcla() {
        assert_eq!(
            inline("**`iftop`** — muestra el tráfico"),
            vec![Trozo::Fuerte("`iftop`".into()), n(" — muestra el tráfico")]
        );
    }

    /// Una marca sin cerrar no puede comerse el resto de la línea: en texto
    /// generado pasa, y perder media frase sería peor que ver un asterisco.
    #[test]
    fn una_marca_sin_cerrar_no_rompe_nada() {
        assert_eq!(inline("2 ** 8 son 256"), vec![n("2 ** 8 son 256")]);
        assert_eq!(inline("el fichero `roto"), vec![n("el fichero `roto")]);
        assert_eq!(inline("**a medias"), vec![n("**a medias")]);
    }

    #[test]
    fn no_confunde_una_comilla_vacia() {
        assert_eq!(inline("`` vacío"), vec![n("`` vacío")]);
    }

    #[test]
    fn respeta_los_acentos_y_la_enye() {
        assert_eq!(
            inline("**año** de ñu"),
            vec![Trozo::Fuerte("año".into()), n(" de ñu")]
        );
    }

    #[test]
    fn reconoce_los_bloques_de_comando() {
        let mut l = Lector::default();
        assert!(matches!(
            l.linea("```bash"),
            Linea::Delimitador { abre: true, .. }
        ));
        assert_eq!(l.linea("which nmap"), Linea::Comando("which nmap".into()));
        assert!(matches!(
            l.linea("```"),
            Linea::Delimitador { abre: false, .. }
        ));
        assert!(matches!(l.linea("y luego"), Linea::Texto(_)));
    }

    /// Dentro de un bloque, un asterisco es un asterisco: es un comando y hay
    /// que poder leerlo exactamente como se va a ejecutar.
    #[test]
    fn dentro_de_un_bloque_no_se_interpreta_nada() {
        let mut l = Lector::default();
        l.linea("```bash");
        assert_eq!(
            l.linea("rm *.tmp && echo `date`"),
            Linea::Comando("rm *.tmp && echo `date`".into())
        );
    }

    #[test]
    fn reconoce_los_titulos() {
        let mut l = Lector::default();
        assert_eq!(l.linea("## Resumen"), Linea::Titulo("Resumen".into()));
        // Pero una almohadilla suelta no es un título.
        assert!(matches!(l.linea("#hashtag"), Linea::Texto(_)));
    }

    /// Un bloque `diff` no es un bloque de comandos: sus líneas se marcan
    /// como cambio para poder pintarlas, y nadie las va a ejecutar.
    #[test]
    fn un_bloque_diff_no_es_un_comando() {
        let mut l = Lector::default();
        assert!(matches!(
            l.linea("```diff"),
            Linea::Delimitador { abre: true, .. }
        ));
        assert_eq!(
            l.linea("-1317     let x = 1;"),
            Linea::Cambio {
                signo: '-',
                texto: "1317     let x = 1;".into()
            }
        );
        assert_eq!(
            l.linea("+1317     let x = 2;"),
            Linea::Cambio {
                signo: '+',
                texto: "1317     let x = 2;".into()
            }
        );
        assert_eq!(
            l.linea(" 1318     }"),
            Linea::Cambio {
                signo: ' ',
                texto: "1318     }".into()
            }
        );
        assert!(matches!(
            l.linea("```"),
            Linea::Delimitador { abre: false, .. }
        ));
        // Y al cerrarlo se vuelve a la normalidad: el bloque siguiente sí es
        // de comandos.
        assert!(matches!(l.linea("```bash"), Linea::Delimitador { .. }));
        assert_eq!(l.linea("ls -l"), Linea::Comando("ls -l".into()));
    }

    /// Una línea vacía dentro de un diff no puede reventar el partido del
    /// signo: eso sería un panic en mitad de la conversación.
    #[test]
    fn una_linea_vacia_dentro_del_diff_no_revienta() {
        let mut l = Lector::default();
        l.linea("```diff");
        assert_eq!(
            l.linea(""),
            Linea::Cambio {
                signo: ' ',
                texto: String::new()
            }
        );
    }
}
