//! Los colores, por lo que significan y no por cómo se llaman.
//!
//! Antes cada sitio elegía el suyo —`.cyan()` aquí, `.magenta()` allá— y
//! cambiar el aspecto de algo obligaba a buscarlo por todo el código, con el
//! riesgo de dejarse la mitad. Aquí un color tiene un **papel**: lo que está
//! enfocado, lo que es secundario, lo que ha ido mal. El papel se decide una
//! vez y el resto del código lo pide por su nombre.
//!
//! ## Por qué 16 colores y no una paleta propia
//!
//! Podríamos fijar valores RGB exactos y verlos idénticos en todas partes.
//! Sería un error: quien usa una terminal ha elegido su tema, y un programa
//! que planta sus propios azules encima desentona con todo lo demás que tiene
//! abierto. Con los 16 de siempre, vsioncode se ve como se ve **su** terminal.
//! Es también el único modo de que funcione igual por SSH, dentro de tmux y en
//! una consola sin nada.
//!
//! ## `NO_COLOR`
//!
//! Se respeta de verdad. Estuvo prometido en un comentario y sin implementar,
//! que es la peor de las tres opciones posibles.

use ratatui::style::{Color, Modifier, Style};

/// Qué papel juega algo en la pantalla.
///
/// La lista es corta a propósito: en cuanto hay quince papeles, nadie sabe
/// cuál toca y se vuelve a elegir el color a ojo, que es de lo que veníamos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Papel {
    /// Lo que tiene el foco o se puede accionar ahora.
    Acento,
    /// Texto normal.
    Texto,
    /// Metadatos: rutas, tiempos, modelo, ayudas de teclas.
    Tenue,
    /// Lo que ha dicho el humano.
    Humano,
    /// Lo que ha dicho el agente.
    Agente,
    /// Lo que dice vsioncode de sí mismo.
    Sistema,
    /// Un comando: lo que va a ejecutarse.
    Comando,
    /// El agente ha parado y espera a que lo lances tú.
    ///
    /// Pariente del verde del comando y a propósito —lo que espera ES un
    /// comando—, pero un tono aparte: si fuera el mismo, el aviso se leería
    /// como una línea más de las que ya hay tres en pantalla. Y va con glifo
    /// además de color, como todo lo que importa aquí.
    Espera,
    /// El distintivo de un modo en la barra: fondo apagado, letra clara.
    ///
    /// La barra está siempre en pantalla, así que un fondo saturado es una
    /// mancha permanente en el borde de la vista. Apagado se lee igual de bien
    /// y deja de reclamar el ojo cada vez que pasa por ahí.
    ModoObservar,
    ModoSugerir,
    ModoConversar,
    /// El único que sigue siendo intenso, y a propósito: cuando el agente
    /// puede lanzar comandos por su cuenta, eso no puede pasar desapercibido.
    ModoEjecutar,
    /// Un fragmento de código dentro de una frase.
    Codigo,
    /// El fondo sobre el que se lee lo que ha escrito el humano.
    ///
    /// Sustituye a la etiqueta «tú» y a su barra de color: un fondo tenue
    /// ajustado al texto dice de quién es sin gritarlo, y no gasta ni una
    /// columna en decorado.
    FondoHumano,
    /// Salió bien.
    Exito,
    /// Salió mal.
    Error,
    /// Ojo con esto.
    Aviso,
    /// Una línea que se va en un cambio.
    ///
    /// Van con fondo y no solo con letra de color: un diff se lee de un
    /// vistazo por bloques, no línea a línea, y para eso hace falta que el
    /// bloque tenga forma. El fondo es apenas un tinte —lo que se lee es el
    /// código, no la banda—, y el signo de delante lo dice igual sin color.
    Quitado,
    /// Una línea que entra en un cambio.
    Puesto,
}

/// La paleta en uso.
#[derive(Debug, Clone, Copy)]
pub struct Tema {
    /// Si es `false`, todo se pinta sin color y la jerarquía la llevan el
    /// grosor, la posición y los glifos.
    pub color: bool,
}

impl Default for Tema {
    fn default() -> Self {
        Self::detectar()
    }
}

impl Tema {
    /// Mira el entorno una vez.
    ///
    /// `NO_COLOR` cuenta por estar definida, aunque esté vacía: así lo dice su
    /// especificación y así lo espera quien la usa.
    pub fn detectar() -> Self {
        let sin_color = std::env::var_os("NO_COLOR").is_some()
            || std::env::var("TERM").is_ok_and(|t| t == "dumb");
        Self { color: !sin_color }
    }

    /// El estilo de un papel.
    pub fn estilo(&self, p: Papel) -> Style {
        if !self.color {
            // Sin color queda el peso, que es lo único que sobrevive a un
            // terminal monocromo y a quien no distingue estos tonos.
            return match p {
                // Sin color, el humano recupera la negrita: es lo único que
                // queda para saber de quién es la línea cuando el fondo gris
                // no se puede pintar.
                Papel::Humano | Papel::Acento | Papel::Comando => {
                    Style::new().add_modifier(Modifier::BOLD)
                }
                Papel::Tenue | Papel::Sistema => Style::new().add_modifier(Modifier::DIM),
                // Sin color, un modo se distingue por el vídeo inverso; si no,
                // se perdería entre el resto de la barra.
                Papel::ModoObservar
                | Papel::ModoSugerir
                | Papel::ModoConversar
                | Papel::ModoEjecutar => Style::new().add_modifier(Modifier::REVERSED),
                Papel::Error | Papel::Espera => Style::new().add_modifier(Modifier::BOLD),
                // Sin color el diff lo llevan el signo y el número, que están
                // en el texto. Atenuar lo que se va y dejar entero lo que
                // entra basta para saber cuál es cuál.
                Papel::Quitado => Style::new().add_modifier(Modifier::DIM),
                _ => Style::new(),
            };
        }

        match p {
            // Un cian apagado, no el cian de terminal a plena potencia: sale
            // en los bordes de los dos paneles, en los títulos y en los menús,
            // o sea en casi toda la pantalla a la vez. A plena intensidad, lo
            // que enmarca el trabajo compite con el trabajo.
            Papel::Acento => Style::new().fg(Color::Indexed(73)),
            Papel::Texto => Style::new(),
            Papel::Tenue => Style::new().fg(Color::DarkGray),
            Papel::Humano => Style::new(),
            // Los dos puntos del agente, en un gris distinto del de vsioncode:
            // se distinguen entre sí sin que ninguno reclame atención.
            Papel::Agente => Style::new().fg(Color::Indexed(245)),
            // Apenas por encima del negro. Ocupa el ancho entero del panel,
            // así que cualquier gris con cuerpo se convierte en una banda que
            // pesa más que lo que hay escrito encima.
            // Ámbar apagado: marca el fragmento sin convertir una tabla de
            // comandos en un cartel luminoso.
            Papel::Codigo => Style::new().fg(Color::Indexed(137)),
            Papel::FondoHumano => Style::new().bg(Color::Indexed(235)),
            Papel::ModoObservar => Style::new().fg(Color::Indexed(250)).bg(Color::Indexed(238)),
            Papel::ModoSugerir => Style::new().fg(Color::Indexed(253)).bg(Color::Indexed(24)),
            Papel::ModoConversar => Style::new().fg(Color::Indexed(253)).bg(Color::Indexed(96)),
            // Rojo apagado, pero rojo: el aviso no se suaviza hasta dejar de
            // avisar.
            Papel::ModoEjecutar => Style::new().fg(Color::Indexed(255)).bg(Color::Indexed(88)),
            Papel::Sistema => Style::new().fg(Color::DarkGray),
            // Verde apagado, no el verde de terminal a plena potencia: un
            // comando propuesto tiene que distinguirse del texto, no dominar
            // la pantalla desde el otro lado de la habitación.
            Papel::Comando => Style::new().fg(Color::Indexed(114)),
            // Un verde más claro y más amarillo que el del comando: al lado se
            // distingue, y solo. No es un aviso —no ha ido nada mal—, es que
            // el trabajo está parado esperándote.
            Papel::Espera => Style::new().fg(Color::Indexed(150)),
            Papel::Exito => Style::new().fg(Color::Indexed(114)),
            Papel::Error => Style::new().fg(Color::Red),
            Papel::Aviso => Style::new().fg(Color::Yellow),
            // Un rojo y un verde muy oscuros, del orden del fondo del humano:
            // un diff largo ocupa media pantalla, y a plena saturación deja de
            // poder leerse el código que hay encima. La letra se queda en su
            // color normal a propósito — el fondo dice el signo, el texto dice
            // el código.
            Papel::Quitado => Style::new().bg(Color::Indexed(52)),
            Papel::Puesto => Style::new().bg(Color::Indexed(22)),
        }
    }

    /// Atajo para lo tenue, que es lo que más se pide.
    pub fn tenue(&self) -> Style {
        self.estilo(Papel::Tenue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La promesa que estuvo escrita en un comentario durante meses sin
    /// cumplirse. Con `NO_COLOR` no puede quedar ni un color puesto.
    /// La espera es pariente del comando, no el comando: si fuera el mismo
    /// verde, «te toca a ti» se leería como una línea más de las tres que ya
    /// hay en pantalla.
    #[test]
    fn la_espera_tiene_su_propio_verde() {
        let t = Tema { color: true };
        assert_ne!(t.estilo(Papel::Espera).fg, t.estilo(Papel::Comando).fg);
        assert_ne!(t.estilo(Papel::Espera).fg, t.estilo(Papel::Sistema).fg);
        // Y en monocromo sigue diciéndolo, que es donde el color no ayuda.
        let mono = Tema { color: false };
        assert!(
            mono.estilo(Papel::Espera)
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn sin_color_no_se_pinta_nada_de_color() {
        let t = Tema { color: false };
        for p in [
            Papel::Acento,
            Papel::Texto,
            Papel::Tenue,
            Papel::Humano,
            Papel::Agente,
            Papel::Sistema,
            Papel::Comando,
            Papel::Espera,
            Papel::Exito,
            Papel::Error,
            Papel::Aviso,
        ] {
            let e = t.estilo(p);
            assert!(e.fg.is_none(), "«{p:?}» sigue teniendo color");
            assert!(e.bg.is_none(), "«{p:?}» sigue teniendo fondo");
        }
    }

    /// Y aun sin color hay que poder distinguir lo importante: si todo queda
    /// exactamente igual, la pantalla se vuelve ilegible en vez de sobria.
    #[test]
    fn sin_color_queda_la_jerarquia() {
        let t = Tema { color: false };
        assert!(
            t.estilo(Papel::Humano)
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(t.estilo(Papel::Tenue).add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn con_color_los_papeles_no_se_confunden() {
        let t = Tema { color: true };
        assert_ne!(t.estilo(Papel::Error), t.estilo(Papel::Exito));
        assert_ne!(t.estilo(Papel::Sistema), t.estilo(Papel::Agente));
    }

    /// Lo que identifica al humano es un fondo, no una tinta: si algún día
    /// vuelve a ser color de letra, el bloque deja de leerse como bloque.
    #[test]
    fn al_humano_lo_marca_el_fondo() {
        let t = Tema { color: true };
        assert!(t.estilo(Papel::FondoHumano).bg.is_some());
        assert!(t.estilo(Papel::Humano).fg.is_none());
    }
}
