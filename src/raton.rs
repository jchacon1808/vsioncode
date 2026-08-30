//! El ratón.
//!
//! ## Por qué hay que capturarlo, aunque tenga un coste
//!
//! Dentro de la pantalla alterna, casi todos los emuladores traducen la rueda
//! a flechas arriba y abajo. Y en un shell, la flecha arriba es «el comando
//! anterior». Así que hacer scroll en vsioncode, sin capturar el ratón,
//! no movía la pantalla: te iba escribiendo tu historial en la línea. Es
//! exactamente lo contrario de lo que quiere quien gira la rueda.
//!
//! Capturarlo lo arregla, pero tiene un precio conocido: **el emulador deja
//! de poder seleccionar texto con el ratón**, porque los eventos ya no le
//! llegan. Es la queja recurrente que arrastran otras TUIs de agentes.
//!
//! Dos salidas, y conviene conocer las dos:
//!
//! - **`Mayús` + arrastrar** salta la captura en casi todos los emuladores
//!   (xterm, kitty, ghostty, gnome-terminal, iTerm2). Es lo que hay que hacer
//!   para copiar y pegar, y por eso se dice en la ayuda.
//! - **`/raton`** apaga la captura del todo cuando eso no baste.
//!
//! ## Y no le robamos el ratón a quien lo pidió
//!
//! Si el programa que está corriendo dentro del shell activó el ratón —`vim`,
//! `htop`, `mc`, `less` con `--mouse`—, sus eventos se le reenvían tal cual.
//! Robárselos sería el mismo error que robarle las teclas de función.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

/// Qué zona de la pantalla se ha tocado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zona {
    Shell,
    Agente,
    /// La caja de escritura del agente.
    Caja,
    Barra,
}

/// Qué hay que hacer con un evento del ratón.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accion {
    /// Poner el foco en una zona.
    Foco(Zona),
    /// Mover el scrollback del shell. Positivo sube (hacia lo viejo).
    DesplazarShell(i32),
    /// Mover la conversación.
    DesplazarChat(i32),
    /// Reenviárselo al programa de dentro, ya codificado.
    AlPrograma(Vec<u8>),
    /// No hacer nada.
    Nada,
}

/// Cuántas líneas mueve un golpe de rueda.
///
/// Tres es lo que usan los emuladores por costumbre. Con una sola línea el
/// scroll se siente pegajoso; con más de cinco se pierde el sitio.
const PASO: i32 = 3;

fn dentro(a: Rect, x: u16, y: u16) -> bool {
    x >= a.x && x < a.x + a.width && y >= a.y && y < a.y + a.height
}

/// Decide qué hacer con un evento del ratón.
///
/// `programa_quiere_raton` es cierto cuando el programa de dentro del shell
/// activó el ratón; entonces manda él.
pub fn resolver(
    ev: MouseEvent,
    shell: Option<Rect>,
    agente: Option<Rect>,
    caja: Option<Rect>,
    programa_quiere_raton: bool,
) -> Accion {
    let (x, y) = (ev.column, ev.row);
    let en_shell = shell.is_some_and(|s| dentro(s, x, y));

    // El programa de dentro pidió el ratón: es suyo mientras el puntero esté
    // sobre él.
    if en_shell && programa_quiere_raton {
        if let Some(bytes) = shell.and_then(|s| codificar(ev, s)) {
            return Accion::AlPrograma(bytes);
        }
        return Accion::Nada;
    }

    match ev.kind {
        MouseEventKind::ScrollUp => {
            if en_shell {
                Accion::DesplazarShell(PASO)
            } else if agente.is_some_and(|a| dentro(a, x, y)) {
                Accion::DesplazarChat(PASO)
            } else {
                Accion::Nada
            }
        }
        MouseEventKind::ScrollDown => {
            if en_shell {
                Accion::DesplazarShell(-PASO)
            } else if agente.is_some_and(|a| dentro(a, x, y)) {
                Accion::DesplazarChat(-PASO)
            } else {
                Accion::Nada
            }
        }
        // Un clic pone el foco donde has clicado. Es lo que hace cualquier
        // interfaz con dos paneles, y no tenerlo obligaba a usar Alt+A para
        // algo que la mano ya estaba señalando.
        MouseEventKind::Down(MouseButton::Left) => {
            if en_shell {
                Accion::Foco(Zona::Shell)
            } else if caja.is_some_and(|c| dentro(c, x, y)) {
                Accion::Foco(Zona::Caja)
            } else if agente.is_some_and(|a| dentro(a, x, y)) {
                Accion::Foco(Zona::Agente)
            } else {
                Accion::Nada
            }
        }
        _ => Accion::Nada,
    }
}

/// Codifica un evento para el programa de dentro, en SGR (`CSI < b ; x ; y M/m`).
///
/// SGR es el que se usa hoy: el modo antiguo mete las coordenadas en un byte y
/// se rompe pasada la columna 223, cosa que ocurre en cualquier pantalla ancha.
fn codificar(ev: MouseEvent, shell: Rect) -> Option<Vec<u8>> {
    // Coordenadas relativas al interior del panel, y en base 1.
    let col = ev.column.checked_sub(shell.x + 1)? + 1;
    let fila = ev.row.checked_sub(shell.y + 1)? + 1;

    let (boton, suelta) = match ev.kind {
        MouseEventKind::Down(b) => (codigo_boton(b), false),
        MouseEventKind::Up(b) => (codigo_boton(b), true),
        MouseEventKind::Drag(b) => (codigo_boton(b) + 32, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
        MouseEventKind::Moved => (35, false),
    };

    let final_ = if suelta { 'm' } else { 'M' };
    Some(format!("\x1b[<{boton};{col};{fila}{final_}").into_bytes())
}

fn codigo_boton(b: MouseButton) -> u16 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn ev(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn zonas() -> (Option<Rect>, Option<Rect>, Option<Rect>) {
        let shell = Some(Rect::new(0, 0, 60, 30));
        let agente = Rect::new(60, 0, 40, 30);
        let caja = Rect::new(61, 26, 38, 3);
        (shell, Some(agente), Some(caja))
    }

    /// Lo que motivó todo esto: girar la rueda sobre el shell tiene que mover
    /// la pantalla, no escribir el comando anterior en la línea.
    #[test]
    fn la_rueda_sobre_el_shell_desplaza_la_pantalla() {
        let (s, a, c) = zonas();
        assert_eq!(
            resolver(ev(MouseEventKind::ScrollUp, 10, 10), s, a, c, false),
            Accion::DesplazarShell(PASO)
        );
        assert_eq!(
            resolver(ev(MouseEventKind::ScrollDown, 10, 10), s, a, c, false),
            Accion::DesplazarShell(-PASO)
        );
    }

    #[test]
    fn la_rueda_sobre_el_agente_mueve_la_conversacion() {
        let (s, a, c) = zonas();
        assert_eq!(
            resolver(ev(MouseEventKind::ScrollUp, 70, 10), s, a, c, false),
            Accion::DesplazarChat(PASO)
        );
    }

    #[test]
    fn un_clic_pone_el_foco_donde_clicas() {
        let (s, a, c) = zonas();
        let clic = |x, y| {
            resolver(
                ev(MouseEventKind::Down(MouseButton::Left), x, y),
                s,
                a,
                c,
                false,
            )
        };
        assert_eq!(clic(10, 10), Accion::Foco(Zona::Shell));
        assert_eq!(clic(70, 10), Accion::Foco(Zona::Agente));
        // La caja de escritura está dentro del panel del agente, y gana ella.
        assert_eq!(clic(70, 27), Accion::Foco(Zona::Caja));
    }

    /// Si `vim` o `htop` pidieron el ratón, es suyo. Quitárselo sería el mismo
    /// error que quitarle las teclas de función.
    #[test]
    fn si_el_programa_pidio_el_raton_se_lo_damos() {
        let (s, a, c) = zonas();
        match resolver(
            ev(MouseEventKind::Down(MouseButton::Left), 10, 10),
            s,
            a,
            c,
            true,
        ) {
            Accion::AlPrograma(b) => {
                let t = String::from_utf8(b).unwrap();
                assert!(t.starts_with("\x1b[<0;"), "debería ser SGR: {t:?}");
            }
            otro => panic!("esperaba reenvío, vino {otro:?}"),
        }
        // Pero solo dentro del shell: en el panel del agente seguimos mandando.
        assert_eq!(
            resolver(ev(MouseEventKind::ScrollUp, 70, 10), s, a, c, true),
            Accion::DesplazarChat(PASO)
        );
    }

    /// El modo antiguo mete las coordenadas en un byte y se rompe pasada la
    /// columna 223. SGR no, y por eso se usa SGR.
    #[test]
    fn las_coordenadas_grandes_no_se_rompen() {
        let ancho = Rect::new(0, 0, 300, 50);
        let b = codificar(ev(MouseEventKind::Down(MouseButton::Left), 250, 40), ancho).unwrap();
        assert_eq!(String::from_utf8(b).unwrap(), "\x1b[<0;250;40M");
    }

    #[test]
    fn las_coordenadas_son_relativas_al_interior_del_panel() {
        // El panel empieza en x=60 y tiene borde: el interior arranca en 61.
        let panel = Rect::new(60, 0, 40, 30);
        let b = codificar(ev(MouseEventKind::Down(MouseButton::Left), 61, 1), panel).unwrap();
        assert_eq!(String::from_utf8(b).unwrap(), "\x1b[<0;1;1M");
    }

    #[test]
    fn soltar_el_boton_termina_en_minuscula() {
        let r = Rect::new(0, 0, 80, 24);
        let b = codificar(ev(MouseEventKind::Up(MouseButton::Left), 5, 5), r).unwrap();
        assert!(String::from_utf8(b).unwrap().ends_with('m'));
    }

    /// Con el panel del shell oculto, la rueda sobre esa zona no puede
    /// desplazar algo que no se está viendo.
    #[test]
    fn sin_panel_de_shell_no_se_desplaza_el_shell() {
        let (_, a, c) = zonas();
        let r = resolver(ev(MouseEventKind::ScrollUp, 10, 10), None, a, c, false);
        assert_ne!(r, Accion::DesplazarShell(PASO));
    }

    #[test]
    fn fuera_de_todo_no_pasa_nada() {
        let (s, a, c) = zonas();
        assert_eq!(
            resolver(ev(MouseEventKind::ScrollUp, 10, 90), s, a, c, false),
            Accion::Nada
        );
    }
}
