//! Reparto de teclas entre vsioncode y el shell.
//!
//! ## El problema
//!
//! Un shell real se come **todas** las teclas. `Ctrl+A` es «inicio de línea»,
//! `Ctrl+E` «fin de línea», `Ctrl+R` busca en el historial, `Tab` completa,
//! `Ctrl+C` interrumpe. Prácticamente todo `Ctrl+letra` de la `a` a la `z` ya
//! significa algo que la gente tiene memorizado en los dedos.
//!
//! Robar cualquiera de esas teclas rompe músculo de años. Y no robar ninguna
//! deja la aplicación sin forma de hablarle.
//!
//! ## La solución: Alt es nuestro, Ctrl es del shell
//!
//! Readline ocupa en bash `M-b f d t u l c r y p n . # & * ? ~ 0-9 -`. Quedan
//! libres, entre otras, **a, e, g, h, i, j, k, m, o, q, s, w, x, z**. De ahí
//! salen todos los atajos de vsioncode.
//!
//! La consecuencia importante: **`Ctrl` queda intacto**. Ni uno solo de los
//! atajos de readline, vim o emacs se pierde. Es lo contrario de lo que hacen
//! los multiplexores: tmux se queda `Ctrl+B` y zellij `Ctrl+G`, y en zellij
//! llegó a ser imposible mandar `Ctrl+G` a las aplicaciones de dentro.
//!
//! ## Y además, teclas de función
//!
//! Alt está bien para quien ya sabe. Pero el sentido de vsioncode es que no
//! haga falta saberse nada, así que las teclas de función hacen lo mismo y se
//! pintan siempre en la barra: se descubren mirando, no leyendo un manual.
//!
//! Tienen un pero — `htop` usa F1-F10, `mc` también — y por eso mientras haya
//! un programa ocupando la pantalla, sus teclas vuelven a ser suyas. Los
//! atajos con Alt siguen funcionando ahí, que para eso se eligieron.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::Focus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Traer lo que haya en el portapapeles del sistema.
    Pegar,
    Quit,
    ToggleFocus,
    ToggleAgentPane,
    ExplainLast,
    ToggleCapture,
    CycleMode,
    AcceptProposal,
    DismissProposal,
    Submit,
    /// Cortar lo que el agente esté haciendo.
    Abortar,
    /// Encender o apagar el modo de voz.
    Voz,
    /// Encender o apagar el modo libre.
    Libre,
    /// Mandarle al agente lo que se ve ahora mismo en la pantalla del shell.
    Pantalla,
    ScrollChatUp,
    ScrollChatDown,
    Help,
}

pub struct Binding {
    pub keys: &'static str,
    pub what: &'static str,
}

/// La tabla que se pinta en la ayuda. Es la misma fuente de verdad que
/// [`resolve_with`]: si se añade un atajo, se añade aquí.
pub const HELP: &[Binding] = &[
    Binding {
        keys: "Alt+A  · F2",
        what: "cambiar el foco entre el shell y el agente",
    },
    Binding {
        keys: "Alt+E  · F3",
        what: "explicar lo último que pasó en la terminal",
    },
    Binding {
        keys: "Alt+↵  · F5",
        what: "poner el comando propuesto en mi línea (no lo ejecuta)",
    },
    Binding {
        keys: "Alt+W  · F6",
        what: "pausar o reanudar la captura",
    },
    Binding {
        keys: "Alt+M  · F7",
        what: "modo: observar · sugerir · conversar · ejecutar",
    },
    Binding {
        keys: "Alt+Z  · F4",
        what: "colapsar o mostrar el panel del agente",
    },
    Binding {
        keys: "Alt+J/K",
        what: "mover el panel del agente sin soltar el shell",
    },
    Binding {
        keys: "Alt+Q  · F12",
        what: "salir de vsioncode",
    },
    Binding {
        keys: "Alt+V  · F8",
        what: "modo de voz: hablar y que te conteste",
    },
    Binding {
        keys: "Alt+L  · F9",
        what: "modo libre: sin techo de pasos, para trabajos grandes",
    },
    Binding {
        keys: "Alt+S  · F10",
        what: "que el agente lea lo que hay ahora en la pantalla",
    },
    Binding {
        keys: "Alt+H  · F1",
        what: "esta ayuda — Esc la cierra",
    },
    Binding {
        keys: "Esc · ^C",
        what: "parar al agente, o volver al shell",
    },
    Binding {
        keys: "/",
        what: "menú de comandos: ↑↓ elegir, Tab completar, Esc apartarlo",
    },
    Binding {
        keys: "←",
        what: "con la caja vacía, la barra de delegar: ↑ elige agente, ↵ envía",
    },
    Binding {
        keys: "Enter",
        what: "enviar la pregunta (con el foco en el agente)",
    },
    Binding {
        keys: "Ctrl+cualquier cosa",
        what: "va al shell intacto: ^A ^C ^D ^E ^K ^L ^R ^U ^W ^Z siguen siendo tuyos",
    },
    Binding {
        keys: "(Alt+L)",
        what: "la única tecla de readline que se toma: era «downcase-word»",
    },
];

pub fn resolve(k: KeyEvent, focus: Focus) -> Option<Action> {
    resolve_with(k, focus, false)
}

/// Decide si una tecla es de vsioncode. `None` significa «que la reciba el
/// shell, tal cual».
///
/// `fullscreen` indica que hay un programa ocupando la pantalla alterna
/// (`vim`, `htop`, `less`).
pub fn resolve_with(k: KeyEvent, focus: Focus, fullscreen: bool) -> Option<Action> {
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

    // Regla número uno, sin excepciones: Ctrl es del shell.
    if ctrl && !alt {
        // Salvo dentro de la caja de escritura del agente, que no es el shell.
        if focus == Focus::Agent {
            match k.code {
                KeyCode::Char('n') => return Some(Action::DismissProposal),
                // En la caja del agente, ^C corta al agente en vez de ir al
                // shell: es lo que espera cualquiera que lo pulse ahí.
                KeyCode::Char('c') => return Some(Action::Abortar),
                // Pegar de verdad: una terminal solo transmite texto, así que
                // para una imagen hay que ir a buscarla al portapapeles del
                // escritorio. Solo con el foco en el agente; en el shell,
                // ^V sigue siendo del shell como todo lo demás.
                KeyCode::Char('v') => return Some(Action::Pegar),
                _ => {}
            }
        }
        return None;
    }

    // Atajos con Alt: funcionan siempre, incluso sobre un programa a pantalla
    // completa, porque ninguno de ellos los usa.
    if alt {
        let a = match k.code {
            KeyCode::Char('a') => Some(Action::ToggleFocus),
            KeyCode::Char('e') => Some(Action::ExplainLast),
            KeyCode::Char('w') => Some(Action::ToggleCapture),
            KeyCode::Char('m') => Some(Action::CycleMode),
            KeyCode::Char('z') => Some(Action::ToggleAgentPane),
            KeyCode::Char('h') => Some(Action::Help),
            KeyCode::Char('v') => Some(Action::Voz),
            KeyCode::Char('l') => Some(Action::Libre),
            KeyCode::Char('s') => Some(Action::Pantalla),
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('j') => Some(Action::ScrollChatDown),
            KeyCode::Char('k') => Some(Action::ScrollChatUp),
            KeyCode::Enter => Some(Action::AcceptProposal),
            _ => None,
        };
        if a.is_some() {
            return a;
        }
    }

    // La salida de emergencia nunca se pierde, pase lo que pase.
    if k.code == KeyCode::F(12) {
        return Some(Action::Quit);
    }

    // Mientras un programa ocupe la pantalla, sus teclas de función son suyas.
    if fullscreen && focus == Focus::Shell {
        return None;
    }

    match (k.code, focus) {
        (KeyCode::F(1), _) => Some(Action::Help),
        (KeyCode::F(2), _) => Some(Action::ToggleFocus),
        (KeyCode::F(3), _) => Some(Action::ExplainLast),
        (KeyCode::F(4), _) => Some(Action::ToggleAgentPane),
        (KeyCode::F(5), _) => Some(Action::AcceptProposal),
        (KeyCode::F(6), _) => Some(Action::ToggleCapture),
        (KeyCode::F(7), _) => Some(Action::CycleMode),
        (KeyCode::F(8), _) => Some(Action::Voz),
        (KeyCode::F(9), _) => Some(Action::Libre),
        (KeyCode::F(10), _) => Some(Action::Pantalla),

        // Con el foco en el agente, la caja de escritura es nuestra.
        (KeyCode::Enter, Focus::Agent) => Some(Action::Submit),
        // Esc para al agente si está trabajando; si no, devuelve el foco al
        // shell. La tecla de «para esto» es la misma en los dos casos.
        (KeyCode::Esc, Focus::Agent) => Some(Action::Abortar),
        (KeyCode::PageUp, Focus::Agent) => Some(Action::ScrollChatUp),
        (KeyCode::PageDown, Focus::Agent) => Some(Action::ScrollChatDown),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: m,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// La prueba que de verdad importa. Si esta falla, vsioncode le ha robado
    /// al usuario un atajo que lleva años usando.
    #[test]
    fn ni_una_sola_tecla_de_control_es_nuestra() {
        for c in 'a'..='z' {
            let ev = k(KeyCode::Char(c), KeyModifiers::CONTROL);
            // (con el foco en el shell; dentro de la caja del agente hay
            // excepciones documentadas, ver el test de ^C)
            assert!(
                resolve(ev, Focus::Shell).is_none(),
                "^{} tiene que llegar al shell intacto",
                c.to_ascii_uppercase()
            );
        }
    }

    #[test]
    fn las_teclas_normales_van_al_shell() {
        for c in ['a', 'z', '/', '1', 'ñ', 'é'] {
            assert!(resolve(k(KeyCode::Char(c), KeyModifiers::NONE), Focus::Shell).is_none());
        }
        // Tab es de la compleción de bash, no nuestra.
        assert!(resolve(k(KeyCode::Tab, KeyModifiers::NONE), Focus::Shell).is_none());
        // Esc es el prefijo Meta de readline y el modo normal de vim.
        assert!(resolve(k(KeyCode::Esc, KeyModifiers::NONE), Focus::Shell).is_none());
        // Enter en el shell es del shell.
        assert!(resolve(k(KeyCode::Enter, KeyModifiers::NONE), Focus::Shell).is_none());
    }

    /// Los `Alt` de readline se respetan, con **una** excepción deliberada.
    ///
    /// `Alt+L` es `downcase-word`: pasa a minúsculas la palabra siguiente. Se
    /// toma para el modo libre a petición expresa, y con el ojo abierto: es de
    /// los atajos de readline que prácticamente nadie usa a propósito, y el
    /// modo libre sí se usa a diario.
    ///
    /// Que esté escrito aquí es lo importante. Robar una tecla puede ser una
    /// decisión razonable; lo que no vale es hacerlo sin enterarse.
    #[test]
    fn no_pisamos_los_alt_que_usa_readline() {
        // M-b, M-f, M-d, M-u, M-c, M-r, M-y, M-t, M-p, M-n son de readline.
        for c in ['b', 'f', 'd', 'u', 'c', 'r', 'y', 't', 'p', 'n'] {
            assert!(
                resolve(k(KeyCode::Char(c), KeyModifiers::ALT), Focus::Shell).is_none(),
                "Alt+{c} lo usa readline"
            );
        }
    }

    #[test]
    fn la_unica_excepcion_es_alt_l_y_es_a_propositio() {
        assert_eq!(
            resolve(k(KeyCode::Char('l'), KeyModifiers::ALT), Focus::Shell),
            Some(Action::Libre),
            "si esto cambia, revisa el comentario de arriba"
        );
    }

    #[test]
    fn nuestros_atajos_responden() {
        let cases = [
            ('a', Action::ToggleFocus),
            ('e', Action::ExplainLast),
            ('w', Action::ToggleCapture),
            ('m', Action::CycleMode),
            ('z', Action::ToggleAgentPane),
            ('s', Action::Pantalla),
            ('q', Action::Quit),
        ];
        for (c, want) in cases {
            assert_eq!(
                resolve(k(KeyCode::Char(c), KeyModifiers::ALT), Focus::Shell),
                Some(want)
            );
        }
    }

    #[test]
    fn un_programa_a_pantalla_completa_recupera_sus_teclas_de_funcion() {
        // htop usa F1-F10: mientras corre, son suyas.
        for n in 1..=10 {
            assert!(
                resolve_with(k(KeyCode::F(n), KeyModifiers::NONE), Focus::Shell, true).is_none(),
                "F{n} debe llegar al programa"
            );
        }
        // Pero los atajos con Alt siguen ahí, y la salida de emergencia también.
        assert_eq!(
            resolve_with(k(KeyCode::Char('e'), KeyModifiers::ALT), Focus::Shell, true),
            Some(Action::ExplainLast)
        );
        assert_eq!(
            resolve_with(k(KeyCode::F(12), KeyModifiers::NONE), Focus::Shell, true),
            Some(Action::Quit)
        );
    }

    #[test]
    fn con_el_foco_en_el_agente_enter_envia() {
        assert_eq!(
            resolve(k(KeyCode::Enter, KeyModifiers::NONE), Focus::Agent),
            Some(Action::Submit)
        );
        assert_eq!(
            resolve(k(KeyCode::Esc, KeyModifiers::NONE), Focus::Agent),
            Some(Action::Abortar)
        );
    }

    /// ^C en la caja del agente para al agente. Pero en el shell sigue siendo
    /// del shell: ahí es lo que interrumpe el proceso en marcha.
    #[test]
    fn control_c_solo_es_nuestro_dentro_de_la_caja() {
        let c = k(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(resolve(c, Focus::Agent), Some(Action::Abortar));
        assert!(resolve(c, Focus::Shell).is_none());
    }

    #[test]
    fn aceptar_una_propuesta_no_usa_tab() {
        // Tab es de bash. Insertar el comando propuesto es Alt+Enter.
        assert!(resolve(k(KeyCode::Tab, KeyModifiers::NONE), Focus::Shell).is_none());
        assert_eq!(
            resolve(k(KeyCode::Enter, KeyModifiers::ALT), Focus::Shell),
            Some(Action::AcceptProposal)
        );
    }

    /// La tabla de ayuda no puede quedarse desfasada en silencio.
    #[test]
    fn la_ayuda_menciona_todos_los_atajos_con_alt() {
        let texto: String = HELP.iter().map(|b| b.keys).collect::<Vec<_>>().join(" ");
        for tecla in [
            "Alt+A", "Alt+E", "Alt+W", "Alt+M", "Alt+Z", "Alt+Q", "Alt+H", "Alt+S",
        ] {
            assert!(texto.contains(tecla), "falta {tecla} en la ayuda");
        }
    }

    /// Y al revés: que ninguna tecla de la ayuda sea una promesa vacía.
    ///
    /// Una tecla documentada que no hiciera nada sería peor que no
    /// documentarla: se prueba, no pasa nada, y a partir de ahí el resto de la
    /// tabla tampoco se cree. El otro test recorre la tabla buscando teclas
    /// que falten; este recorre las que hay comprobando que responden.
    #[test]
    fn cada_tecla_de_funcion_de_la_ayuda_hace_algo() {
        for b in HELP {
            for trozo in b.keys.split_whitespace() {
                let Some(n) = trozo.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) else {
                    continue;
                };
                assert!(
                    resolve_with(k(KeyCode::F(n), KeyModifiers::NONE), Focus::Shell, false)
                        .is_some(),
                    "la ayuda anuncia F{n} («{}») y no está mapeada",
                    b.what
                );
            }
        }
    }
}
