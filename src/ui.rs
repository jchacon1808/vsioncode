//! La vista. Función pura del estado: nada aquí modifica nada.
//!
//! ## Dos reglas de color que no son estéticas
//!
//! 1. **En el panel del shell no se pinta ningún fondo.** Nunca. Ahí manda el
//!    tema del terminal del usuario; si pintáramos un fondo propio, su
//!    Solarized o su Gruvbox se rompería a medias, que es peor que no tener
//!    tema.
//! 2. **Éxito y fallo nunca se distinguen solo por verde y rojo.** Siempre
//!    llevan glifo y número (`✓ 0`, `✗ 127`), que se leen igual con
//!    daltonismo, en monocromo y con `NO_COLOR`.
//!
//! ## Cómo se sabe qué panel tiene el foco
//!
//! No se puede usar Tab para cambiar de panel: Tab es de la compleción de
//! bash. Así que el foco se señala por tres vías redundantes a la vez: el
//! borde del panel activo, la palabra del modo en la barra en vídeo inverso,
//! y el cursor real, que solo se pinta en el panel que lo tiene. La presencia
//! del cursor es la señal más fuerte y no cuesta nada.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use tui_term::widget::PseudoTerminal;

use crate::app::{App, ChatMsg, Focus, Mode, Role, Vista, plural};
use crate::keymap::HELP;
use crate::marcado::{Lector, Linea, Trozo};
use crate::safety::Risk;
use crate::slash;
use crate::tema::Papel;

/// Altura de la barra de estado.
const STATUS_H: u16 = 1;
/// Altura mínima de la caja de escritura.
const INPUT_H: u16 = 3;
/// Y hasta dónde puede crecer cuando escribes —o dictas— algo largo. Dictando
/// se llega a varias líneas enseguida, y una caja fija de tres deja lo que
/// dijiste fuera de la vista justo cuando querrías repasarlo.
const INPUT_H_MAX: u16 = 8;
/// Por debajo de este ancho el panel lateral ahoga al shell: `ls -l`,
/// `git log --oneline` y `docker ps` empiezan a romper líneas. El shell manda.
const MIN_SPLIT_WIDTH: u16 = 100;
/// Lo que le queda al panel del agente por debajo de la lista de subagentes:
/// sus dos bordes, la caja de escribir y una línea de conversación. Si no
/// caben, la lista no sale — un panel sin conversación no es un panel.
const MIN_AGENTE_H: u16 = 6;
/// Cuántos subagentes se enumeran antes de resumir el resto en una línea.
/// Con más, la lista se come el panel para decir algo que ya dice el recuento.
const MAX_SUBS: usize = 6;
/// Por debajo de esto no hay nada que repartir.
const MIN_W: u16 = 60;
const MIN_H: u16 = 12;

/// Reparto de la pantalla. Es la única fuente de verdad del layout: la usan
/// tanto el dibujado como el cálculo del tamaño del PTY, para que el shell
/// nunca crea que tiene un tamaño distinto del que se le pinta.
fn split(area: Rect, vista: Vista, subs: u16) -> Reparto {
    let [body, status] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(STATUS_H)]).areas(area);

    let (shell, agente) = reparto_del_cuerpo(body, vista);

    // La lista de subagentes sale **del panel del agente** y nunca del shell.
    //
    // Es lo que hace el agente, así que ese es su sitio; y además quitarle
    // filas al shell cambiaría el hueco del pseudoterminal cada vez que se
    // reparte trabajo. El PTY solo se redimensiona al cambiar la ventana, así
    // que entre medias se pintaría un terminal de un tamaño en un hueco de
    // otro: las últimas líneas de lo que estás mirando, cortadas.
    let (agente, lista) = match agente {
        Some(a) if subs > 0 => {
            let alto = subs.min(a.height.saturating_sub(MIN_AGENTE_H));
            if alto == 0 {
                (Some(a), None)
            } else {
                let [arriba, abajo] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(alto)]).areas(a);
                (Some(arriba), Some(abajo))
            }
        }
        otro => (otro, None),
    };

    Reparto {
        shell,
        agente,
        lista,
        status,
    }
}

/// El reparto de la pantalla, de arriba abajo.
struct Reparto {
    shell: Option<Rect>,
    agente: Option<Rect>,
    /// La lista de subagentes, cuando hay más de uno trabajando.
    lista: Option<Rect>,
    status: Rect,
}

/// Shell y panel, sin contar lo que se lleve la lista de subagentes.
fn reparto_del_cuerpo(body: Rect, vista: Vista) -> (Option<Rect>, Option<Rect>) {
    match vista {
        Vista::SoloShell => return (Some(body), None),
        // Toda la pantalla, sin márgenes.
        //
        // Antes se centraba a 96 columnas por el argumento tipográfico de
        // siempre —una línea muy larga obliga al ojo a saltar de un extremo a
        // otro—, y en uso resultó peor: el recuadro flotando en mitad de una
        // pantalla con bandas negras a los lados parece una ventana rota, no
        // una decisión. Aquí manda que el panel ocupe lo que se le ha dado.
        Vista::SoloAgente => return (None, Some(body)),
        Vista::Ambos => {}
    }

    if body.width >= MIN_SPLIT_WIDTH {
        // Ancho de sobra: lado a lado, que es como se lee mejor un log largo
        // junto a una explicación.
        let pct = if body.width >= 120 { 62 } else { 72 };
        let [sh, ag] =
            Layout::horizontal([Constraint::Percentage(pct), Constraint::Fill(1)]).areas(body);
        (Some(sh), Some(ag))
    } else {
        // Estrecho: apilado. Antes que estrangular el shell, se le quita alto.
        let [sh, ag] =
            Layout::vertical([Constraint::Percentage(70), Constraint::Fill(1)]).areas(body);
        (Some(sh), Some(ag))
    }
}

/// Tamaño en filas y columnas que le toca al pseudoterminal.
///
/// Se resta el borde del panel. Pasarle al PTY el tamaño de la ventana entera
/// en vez del hueco real es el motivo clásico de que `vim` se pinte torcido.
/// Las zonas de la pantalla, para saber dónde ha clicado el ratón.
///
/// Sale del mismo [`split`] que dibuja, así que no puede desincronizarse: si
/// el reparto cambia, cambian las dos cosas a la vez.
pub struct Zonas {
    pub shell: Option<Rect>,
    pub agente: Option<Rect>,
    pub caja: Option<Rect>,
}

pub fn zonas(width: u16, height: u16, vista: Vista, subs: u16) -> Zonas {
    let area = Rect::new(0, 0, width, height);
    let Reparto { shell, agente, .. } = split(area, vista, subs);
    let caja = agente.map(|a| {
        let inner = Block::bordered().inner(a);
        let [_, _, caja] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(0),
            Constraint::Length(INPUT_H.min(inner.height.saturating_sub(2))),
        ])
        .areas(inner);
        caja
    });
    Zonas {
        shell,
        agente,
        caja,
    }
}

pub fn shell_dims(width: u16, height: u16, vista: Vista) -> (u16, u16) {
    let area = Rect::new(0, 0, width, height);
    // Sin el argumento de los subagentes, y a propósito: el hueco del shell no
    // depende de ellos —la lista se la quita al panel del agente—, y pedirlo
    // aquí invitaría a que algún día sí dependiera.
    let sh = split(area, vista, 0).shell;
    // Sin panel visible el pseudoterminal conserva su tamaño anterior: no se
    // le redimensiona a nada raro solo por dejar de mirarlo, que haría que los
    // programas de dentro se recolocaran cada vez que cambias de vista.
    let Some(sh) = sh else {
        return (0, 0);
    };
    let inner = Block::bordered().inner(sh);
    (inner.height.max(1), inner.width.max(1))
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.width < MIN_W || area.height < MIN_H {
        let msg = format!(
            "La ventana es de {}×{}. vsioncode necesita al menos {MIN_W}×{MIN_H}.",
            area.width, area.height
        );
        f.render_widget(Paragraph::new(msg).red().wrap(Wrap { trim: true }), area);
        return;
    }

    let r = split(area, app.vista, filas_subagentes(app));

    if let Some(sh) = r.shell {
        draw_shell(f, app, sh);
    }
    if let Some(a) = r.agente {
        draw_agent(f, app, a);
    }
    if let Some(l) = r.lista {
        draw_subagentes(f, app, l);
    }
    draw_status(f, app, r.status);

    // La puesta en marcha va por encima de todo lo demás: mientras no haya a
    // quién preguntar, ninguna otra ventana tiene nada que ofrecer.
    if let Some(b) = &app.bienvenida {
        draw_bienvenida(f, b, area);
        return;
    }

    // La ventana de modelos va por encima de la ayuda: si están las dos, la
    // que se acaba de abrir es la que se está mirando.
    if let Some(sel) = &app.selector {
        draw_selector(f, sel, area);
    } else if app.show_help {
        draw_help(f, area);
    }
}

// -------------------------------------------------------------- bienvenida

/// El distintivo de la primera pantalla: el nombre enmarcado y la versión.
///
/// Un rótulo dibujado con los mismos trazos que los marcos de la interfaz, no
/// un logotipo de caracteres: el nombre se lee de una vez, la versión está
/// donde se busca, y nada de esto compite con lo único que hay que hacer aquí,
/// que es elegir un proveedor.
fn rotulo(ancho: u16) -> Vec<Line<'static>> {
    let nombre = "VSIONCODE";
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));

    // El interior se calcula, no se escribe a mano: una constante desalineada
    // con el contenido deja el marco descuadrado en cuanto cambie la versión.
    let interior = 4 + nombre.chars().count() + 6 + version.chars().count() + 4;
    let hueco = " ".repeat(((ancho as usize).saturating_sub(interior + 2)) / 2);
    let barra = "─".repeat(interior);

    vec![
        Line::from(Span::styled(
            format!("{hueco}╭{barra}╮"),
            tema().estilo(Papel::Acento),
        )),
        Line::from(vec![
            Span::styled(format!("{hueco}│    "), tema().estilo(Papel::Acento)),
            Span::styled(nombre, tema().estilo(Papel::Acento).bold()),
            Span::from("      "),
            Span::from(version).dim(),
            Span::styled("    │", tema().estilo(Papel::Acento)),
        ]),
        Line::from(Span::styled(
            format!("{hueco}╰{barra}╯"),
            tema().estilo(Papel::Acento),
        )),
    ]
}

/// Dónde queda el recuadro de la configuración inicial.
///
/// Pública porque el ratón necesita saberlo: para que un clic en el enlace lo
/// abra hay que traducir la fila de la pantalla a la fila de la lista, y eso
/// exige el mismo rectángulo que se dibuja, no uno parecido.
pub fn zona_bienvenida(area: Rect) -> Rect {
    let w = 76u16.min(area.width.saturating_sub(2));
    let h = 26u16.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// La configuración inicial: qué es esto, a quién preguntar y su clave.
fn draw_bienvenida(f: &mut Frame, b: &crate::bienvenida::Bienvenida, area: Rect) {
    use crate::bienvenida::Paso;

    let popup = zona_bienvenida(area);
    f.render_widget(ratatui::widgets::Clear, popup);

    let pie = match &b.paso {
        Paso::Portada => " ↵ empezar · Esc dejarlo para luego ",
        Paso::Eleccion => {
            " ↑↓ elegir · ↵ configurar · O abrir la página · D predeterminado · Esc terminar "
        }
        Paso::Clave { .. } => " ↵ guardar · Ctrl+V pegar · Esc volver ",
        Paso::Hecho => " ↵ empezar a trabajar ",
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(tema().estilo(Papel::Acento))
        .title(Span::from(" config. inicial ").bold())
        .title_bottom(Span::from(pie).dim());
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let lineas = match &b.paso {
        Paso::Portada => portada(b, inner.width),
        Paso::Eleccion => eleccion(b),
        Paso::Clave { i } => clave(b, *i),
        Paso::Hecho => hecho(b),
    };

    f.render_widget(Paragraph::new(lineas).wrap(Wrap { trim: false }), inner);
}

/// Qué es esto, para quien lo abre por primera vez.
fn portada(b: &crate::bienvenida::Bienvenida, ancho: u16) -> Vec<Line<'static>> {
    let mut l: Vec<Line> = vec![Line::from("")];
    l.extend(rotulo(ancho));
    l.push(Line::from(""));
    l.push(Line::from(
        Span::from("Terminal de trabajo con asistencia de IA integrada.").bold(),
    ));
    l.push(Line::from(""));

    for t in [
        "Un shell completo y un asistente que observa lo que ejecutas comparten",
        "el mismo registro de comandos, salidas y códigos de error. Cuando algo",
        "falla, el contexto ya está donde hace falta: no hay nada que copiar.",
        "",
        "Cuatro modos de trabajo definen quién ejecuta y con cuánta autonomía:",
        "desde solo observar hasta encargarle una tarea completa. Todo ocurre",
        "en tu terminal, a la vista, con límite de pasos y freno inmediato.",
    ] {
        l.push(Line::from(Span::from(t)));
    }

    l.push(Line::from(""));
    if b.ya_configurado() {
        use crate::bienvenida::SERVICIOS;

        // Uno por línea y alineados: en una sola frase separada por comas, con
        // tres proveedores ya no se distingue dónde acaba uno y empieza otro,
        // y el que se usará al arrancar —lo único accionable— se pierde.
        let ancho = b
            .listos
            .iter()
            .map(|&i| SERVICIOS[i].nombre.chars().count())
            .max()
            .unwrap_or(0);

        l.push(Line::from(Span::from("Configurado").bold()));
        for &i in &b.listos {
            let mut fila = vec![
                Span::styled("   ✓  ", tema().estilo(Papel::Exito)),
                Span::styled(
                    format!("{:<ancho$}", SERVICIOS[i].nombre),
                    tema().estilo(Papel::Texto),
                ),
            ];
            if b.es_por_defecto(i) {
                fila.push(Span::styled(
                    "   ● predeterminado",
                    tema().estilo(Papel::Exito),
                ));
            }
            l.push(Line::from(fila));
        }
        l.push(Line::from(""));
        l.push(Line::from(
            Span::from("Puedes añadir otro proveedor, actualizar una clave o cambiar con cuál")
                .dim(),
        ));
        l.push(Line::from(
            Span::from("se arranca. Lo ya configurado no se modifica.").dim(),
        ));
    } else {
        l.push(Line::from(
            Span::from("Queda un paso: elegir el proveedor de modelos. Sirve una suscripción")
                .dim(),
        ));
        l.push(Line::from(
            Span::from("existente o la clave gratuita de cualquiera de los servicios.").dim(),
        ));
    }

    if !b.hay_claude {
        l.push(Line::from(""));
        l.push(Line::from(
            Span::from("«claude» no está disponible en el PATH de este equipo.").dim(),
        ));
    }
    l
}

/// La lista de servicios, con su estado y dónde se saca la clave.
fn eleccion(b: &crate::bienvenida::Bienvenida) -> Vec<Line<'static>> {
    use crate::bienvenida::{Bienvenida, SERVICIOS};

    // Ancho de la columna del nombre, para que la del estado quede alineada.
    // Se calcula del contenido: una constante se desalinea en cuanto entre un
    // servicio con el nombre más largo.
    let ancho_nombre = SERVICIOS
        .iter()
        .map(|s| s.nombre.chars().count())
        .max()
        .unwrap_or(24);

    let mut l = vec![
        Line::from(vec![
            Span::from("Proveedores").bold(),
            Span::from("  ·  ↵ configura el señalado").dim(),
        ]),
        Line::from(""),
    ];

    for (i, s) in SERVICIOS.iter().enumerate() {
        let aqui = b.sel == i;
        let listo = b.esta_listo(i);

        // Tres estados y una sola columna para los tres: lo que importa de un
        // vistazo es si ya funciona, no el matiz.
        let (etiqueta, estilo_etiqueta) = if b.es_por_defecto(i) {
            ("● predeterminado", tema().estilo(Papel::Exito).bold())
        } else if listo {
            ("configurado", tema().estilo(Papel::Exito))
        } else if s.id == "claude" && !b.hay_claude {
            ("no instalado", tema().tenue())
        } else if Bienvenida::ya_en_entorno(i) {
            ("clave detectada", tema().estilo(Papel::Acento))
        } else {
            ("sin configurar", tema().tenue())
        };

        let marca = if listo {
            "✓"
        } else if aqui {
            "▸"
        } else {
            " "
        };
        let estilo_nombre = if aqui {
            tema().estilo(Papel::Acento).bold()
        } else if listo {
            tema().estilo(Papel::Exito)
        } else {
            tema().estilo(Papel::Texto)
        };

        l.push(Line::from(vec![
            Span::styled(format!(" {marca} "), estilo_nombre),
            Span::styled(format!("{:<ancho_nombre$}", s.nombre), estilo_nombre),
            Span::styled(format!("  {etiqueta}"), estilo_etiqueta),
        ]));

        l.push(Line::from(vec![
            Span::from("     "),
            Span::from(s.nota.to_string()).dim(),
        ]));

        // El enlace solo bajo el señalado, y dicho como lo que es: algo que se
        // puede abrir. Una dirección escrita sin más invita a copiarla a mano.
        if aqui {
            let mut fila = vec![Span::from("     ")];
            if s.url.is_empty() {
                fila.push(Span::from("sin página que abrir").dim());
            } else {
                fila.push(Span::styled("↗ ", tema().estilo(Papel::Acento)));
                fila.push(Span::styled(
                    s.url.to_string(),
                    tema().estilo(Papel::Comando),
                ));
                fila.push(Span::from("   [O] abrir · o clic").dim());
            }
            l.push(Line::from(fila));
        }
    }

    let fin = b.es_fila_final(b.sel);
    let marca_fin = if fin { "▸" } else { " " };
    l.push(Line::from(""));
    l.push(Line::from(Span::styled(
        if b.listos.is_empty() {
            format!(" {marca_fin} Continuar sin asistente (solo la terminal)")
        } else {
            format!(" {marca_fin} Guardar y empezar a trabajar")
        },
        if fin {
            tema().estilo(Papel::Acento).bold()
        } else {
            tema().tenue()
        },
    )));

    if let Some(e) = &b.error {
        l.push(Line::from(""));
        l.push(Line::from(Span::styled(
            e.clone(),
            tema().estilo(Papel::Error).bold(),
        )));
    }
    l
}

/// La pantalla que pide la clave de un servicio.
///
/// Dos pasos numerados y un campo, en ese orden: primero de dónde sale la
/// clave, después dónde se pega. Antes era un párrafo con la dirección dentro,
/// y una dirección enterrada en prosa se copia a mano en vez de abrirse.
fn clave(b: &crate::bienvenida::Bienvenida, i: usize) -> Vec<Line<'static>> {
    use crate::bienvenida::{SERVICIOS, corta, ruta_env, tapada};

    let s = &SERVICIOS[i];
    let mut l = vec![
        Line::from(vec![
            Span::styled(s.nombre.to_string(), tema().estilo(Papel::Acento).bold()),
            Span::from("  ·  clave de API").dim(),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  1 ", tema().estilo(Papel::Acento).bold()),
            Span::from("Crea una clave en tu cuenta:"),
        ]),
        Line::from(vec![
            Span::from("      "),
            Span::styled("↗ ", tema().estilo(Papel::Acento)),
            Span::styled(s.url.to_string(), tema().estilo(Papel::Comando)),
        ]),
        Line::from(vec![
            Span::from("      "),
            Span::from("[O] la abre en el navegador.").dim(),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  2 ", tema().estilo(Papel::Acento).bold()),
            Span::from("Pégala con Ctrl+V, o escríbela:"),
        ]),
        Line::from(""),
    ];

    // El campo se dibuja con los mismos trazos que el resto: sin marco, un
    // texto suelto en mitad de la pantalla no se lee como algo donde escribir.
    let ancho = 52usize;
    let contenido = if b.entrada.is_empty() {
        format!("{:<ancho$}", "  esperando la clave…")
    } else {
        format!("{:<ancho$}", format!("  {}", tapada(&b.entrada)))
    };
    let borde = "─".repeat(ancho);
    let estilo_borde = tema().estilo(Papel::Acento);

    l.push(Line::from(Span::styled(
        format!("      ╭{borde}╮"),
        estilo_borde,
    )));
    l.push(Line::from(vec![
        Span::styled("      │", estilo_borde),
        if b.entrada.is_empty() {
            Span::from(contenido).dim()
        } else {
            Span::styled(contenido, tema().estilo(Papel::Comando))
        },
        Span::styled("│", estilo_borde),
    ]));
    l.push(Line::from(Span::styled(
        format!("      ╰{borde}╯"),
        estilo_borde,
    )));

    if let Some(e) = &b.error {
        l.push(Line::from(""));
        l.push(Line::from(vec![
            Span::from("      "),
            Span::styled(e.clone(), tema().estilo(Papel::Error).bold()),
        ]));
    }

    l.push(Line::from(""));
    l.push(Line::from(vec![
        Span::from("  Se guarda como ").dim(),
        Span::styled(s.variable.to_string(), tema().estilo(Papel::Codigo)),
        Span::from(" en ").dim(),
        Span::styled(corta(&ruta_env()), tema().estilo(Papel::Codigo)),
        Span::from(" (permisos 600).").dim(),
    ]));
    l.push(Line::from(
        Span::from("  La configuración solo guarda el nombre de la variable, nunca la clave.")
            .dim(),
    ));
    l
}

/// El resumen final: qué se ha escrito y dónde.
fn hecho(b: &crate::bienvenida::Bienvenida) -> Vec<Line<'static>> {
    let mut l = vec![
        Line::from(Span::styled(
            "Todo listo.",
            tema().estilo(Papel::Exito).bold(),
        )),
        Line::from(""),
    ];
    for r in &b.resumen {
        l.push(Line::from(vec![
            Span::styled("  · ", tema().estilo(Papel::Acento)),
            Span::from(r.clone()),
        ]));
    }
    l.push(Line::from(""));
    for t in [
        "Alt+A lleva el foco al panel del asistente y Alt+E explica lo último",
        "que haya pasado en la terminal. Alt+H enseña el resto de teclas.",
        "",
        "Para cambiar de modelo o de servicio, /modelo. Para volver a esta",
        "pantalla, /configurar.",
    ] {
        l.push(Line::from(Span::from(t).dim()));
    }
    l
}

/// Dónde queda el recuadro de elegir modelo.
///
/// El mínimo legible es una petición, no una imposición: si la terminal es
/// más pequeña que él, manda la terminal. Imponerlo restaba más ancho del que
/// había y el centrado daba la vuelta en `u16`: el recuadro se iba a la
/// columna 32763 y pulsar `/modelo` no enseñaba nada.
fn zona_selector(area: Rect) -> Rect {
    let ancho_pedido = area.width.saturating_sub(4).max(20);
    let alto_pedido = area.height.saturating_sub(2).max(6);
    let w = ancho_pedido.min(area.width);
    let h = alto_pedido.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// La ventana de elegir modelo: servidores como cabecera, modelos debajo.
///
/// Ocupa casi toda la pantalla a propósito. Es una decisión, no un aviso: se
/// viene aquí a comparar una lista, y comparar en un recuadro de veinte
/// columnas es justo lo que hacía falta arreglar.
fn draw_selector(f: &mut Frame, sel: &crate::app::Selector, area: Rect) {
    use crate::app::Fila;

    let popup = zona_selector(area);
    f.render_widget(ratatui::widgets::Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(tema().estilo(Papel::Acento))
        .title(Span::from(" modelos ").bold())
        .title_bottom(Span::from(" ↑↓ elegir · ↵ usar · Esc cerrar ").dim());
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Ventana deslizante sobre la fila señalada, igual que en el menú de
    // barra: con varios servidores la lista pasa de largo de la pantalla.
    let alto = inner.height as usize;
    let desde = sel.sel.saturating_sub(alto.saturating_sub(2));

    // La columna de las notas se ajusta al nombre más largo que haya, en vez
    // de a un número fijo. Con un ancho de tabla, «haiku» dejaba treinta
    // espacios muertos antes de su descripción y la línea se leía partida en
    // dos mitades sin relación.
    let ancho_nombre = sel
        .filas
        .iter()
        .filter_map(|f| match f {
            Fila::Modelo { corto, .. } => Some(corto.chars().count()),
            Fila::Servidor { .. } => None,
        })
        .max()
        .unwrap_or(12)
        .clamp(8, 32);

    let lineas: Vec<Line> = sel
        .filas
        .iter()
        .enumerate()
        .skip(desde)
        .take(alto)
        .map(|(i, fila)| match fila {
            Fila::Servidor { id, detalle } => {
                let mut spans = vec![Span::styled(
                    format!("{} ", id.to_uppercase()),
                    tema().estilo(Papel::Acento).bold(),
                )];
                if !detalle.is_empty() {
                    spans.push(Span::from(format!("· {detalle}")).yellow());
                }
                // Una regla tenue hasta el borde: separa un servidor del
                // anterior sin gastar una línea en blanco por cada uno, que
                // con cuatro servidores es media pantalla de aire.
                let usado = id.chars().count() + 1 + detalle.chars().count() + 2;
                let resto = (inner.width as usize).saturating_sub(usado);
                if resto > 0 {
                    spans.push(Span::from("─".repeat(resto)).dim());
                }
                Line::from(spans)
            }
            Fila::Modelo {
                corto,
                nota,
                activo,
                ..
            } => {
                // El que está en uso se marca con un glifo además del color:
                // en monocromo, o para quien no distingue el cian, el color
                // solo no dice nada.
                let marca = if *activo { "●" } else { " " };
                let elegido = i == sel.sel;
                let estilo = if elegido {
                    Style::new().reversed().bold()
                } else if *activo {
                    Style::new().bold()
                } else {
                    Style::new()
                };
                let hueco = (inner.width as usize).saturating_sub(ancho_nombre + 8);
                Line::from(vec![
                    Span::from("  "),
                    Span::styled(format!("{marca} {corto:<ancho_nombre$}"), estilo),
                    Span::from("  "),
                    Span::from(short(nota, hueco)).dim(),
                ])
            }
        })
        .collect();

    f.render_widget(Paragraph::new(lineas), inner);
}

// -------------------------------------------------------------------- shell

fn draw_shell(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Shell;

    // El borde punteado cuando la captura está pausada es la señal permanente
    // de privacidad: mientras no estemos mirando, se nota sin leer nada.
    //
    // Y con la captura activa, siempre la línea más fina: el foco lo dice el
    // color del borde, no su grosor. Engordar el marco entero para señalar
    // dónde estás mueve mucha tinta para decir muy poco.
    let border = if !app.capture {
        BorderType::Double
    } else {
        BorderType::Rounded
    };

    let mut title = vec![Span::from(" shell ").bold()];
    if let Some(cmd) = app.ledger.running_cmd() {
        title.push(Span::from(format!("· {} ", short(cmd, 28))).dim());
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border)
        .border_style(if focused {
            tema().estilo(Papel::Acento)
        } else {
            Style::new().dark_gray()
        })
        .title(Line::from(title));

    // El candado se coge lo justo. Si el hilo lector lo tiene, se pinta el
    // hueco vacío en vez de esperar: perder un fotograma no se nota, bloquear
    // la interfaz sí.
    let Ok(parser) = app.shell.parser.try_read() else {
        f.render_widget(block, area);
        return;
    };
    let screen = parser.screen();

    let cursor = if focused && !screen.hide_cursor() {
        tui_term::widget::Cursor::default()
    } else {
        let mut c = tui_term::widget::Cursor::default();
        c.hide();
        c
    };

    let term = PseudoTerminal::new(screen).block(block).cursor(cursor);
    f.render_widget(term, area);
}

// -------------------------------------------------------------------- agente

fn draw_agent(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Agent;
    let block = Block::default()
        .borders(Borders::ALL)
        // Siempre la línea más fina: el foco lo dice el color, no el grosor.
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            tema().estilo(Papel::Acento)
        } else {
            Style::new().dark_gray()
        })
        .title(Line::from(vec![
            Span::from(" agente ").bold(),
            Span::from(format!("· {} ", app.model)).dim(),
            // Que el agente pueda salir a internet se ve en el título, siempre.
            if app.web {
                Span::styled("· web ", tema().estilo(Papel::Acento))
            } else {
                Span::from("· sin web ").dim()
            },
            if app.voz_encendida() {
                Span::styled("· 🎙 ", tema().estilo(Papel::Acento))
            } else {
                Span::from("").dim()
            },
        ]));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Mientras se escribe un comando de barra, el menú manda: es lo que la
    // persona está mirando ahora mismo. Si no, el sitio es de la propuesta,
    // que es lo más accionable que hay y no puede quedar enterrada.
    let menu = app.menu();
    // La lista de agentes de la barra de delegar sale por el mismo hueco que
    // el menú: es lo mismo —una lista que se despliega sobre la caja— y darle
    // un sitio propio partiría la pantalla en dos por una diferencia que no
    // existe.
    let agentes = app
        .delegar
        .as_ref()
        .and_then(|d| d.lista.map(|_| d.agentes.len()));
    let extra_h = if let Some(n) = agentes {
        (n as u16 + 1).min(inner.height.saturating_sub(5)).min(12)
    } else if !menu.is_empty() {
        (menu.len() as u16 + 1)
            .min(inner.height.saturating_sub(5))
            // Doce y no ocho: con ocho, once comandos no cabían y no había
            // nada que dijera que quedaban más abajo.
            .min(12)
    } else if app.proposal.is_some() {
        4
    } else {
        0
    };

    // La caja crece con el texto, hasta un tope: si se comiera la
    // conversación entera dejaría de verse a quién le estás hablando.
    // El mismo cálculo que usa la caja al pintarse, para que el hueco que se
    // reserva y lo que se dibuja no discrepen nunca.
    let ancho_caja = inner.width.saturating_sub(3).max(4) as usize;
    // Con la barra abierta, lo que crece es el encargo y no la caja de detrás.
    let escrito = match &app.delegar {
        Some(d) => d.tarea.as_str(),
        None => app.input.as_str(),
    };
    let filas = envolver_entrada(escrito, ancho_caja).0.len() as u16;
    let alto_caja = filas
        .max(1)
        .saturating_add(2)
        .clamp(INPUT_H, INPUT_H_MAX)
        .min(inner.height.saturating_sub(2));

    let [chat_area, extra_area, input_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(extra_h),
        Constraint::Length(alto_caja),
    ])
    .areas(inner);

    draw_chat(f, app, chat_area);
    if let Some(d) = &app.delegar {
        if let Some(sel) = d.lista {
            draw_agentes(f, d, sel, extra_area);
        }
        draw_delegar(f, d, input_area, focused);
        return;
    }
    if !menu.is_empty() {
        draw_menu(f, &menu, app.menu_sel, extra_area);
    } else if extra_h > 0 {
        draw_proposal(f, app, extra_area);
    }
    draw_input(f, app, input_area, focused);
}

/// La barra de delegar, en el sitio de la caja.
///
/// En el sitio y no encima: mientras está abierta, lo que se escribe es un
/// encargo para otro, y dejar las dos cajas a la vista invitaría a escribir en
/// la que no toca. Se abre con la caja vacía, así que no tapa nada escrito.
///
/// Lleva el nombre del agente en el título y no en una línea aparte: es el
/// dato que cambia lo que va a pasar al pulsar Enter, y tiene que verse sin
/// buscarlo.
fn draw_delegar(f: &mut Frame, d: &crate::app::Delegar, area: Rect, focused: bool) {
    if area.height == 0 {
        return;
    }
    // El nombre del agente va siempre; las teclas, solo si caben enteras. Un
    // recordatorio cortado a la mitad —«· ↵ envi»— gasta el mismo sitio y no
    // dice nada, y en un panel estrecho ese sitio es justo el del nombre.
    //
    // El ↑ no está aquí sino en la línea de abajo, y solo mientras la caja
    // está vacía: es cuando se puede pulsar y cuando hace falta saberlo.
    const TECLAS: &str = "· ↵ enviar · Esc cierra ";
    let de = d.proveedor();
    let mut titulo = vec![
        Span::from(" delegar ").bold(),
        Span::styled(
            format!("· {} ", d.agente),
            tema().estilo(Papel::Acento).bold(),
        ),
        Span::from(format!("({de}) ")).dim(),
    ];
    let usado = 14 + d.agente.chars().count() + de.chars().count();
    if area.width as usize >= usado + TECLAS.chars().count() {
        titulo.push(Span::from(TECLAS).dim());
    }
    let b = Block::default()
        .borders(Borders::TOP)
        .border_style(tema().estilo(Papel::Acento))
        .title(Line::from(titulo));
    let inner = b.inner(area);
    f.render_widget(b, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if d.tarea.is_empty() {
        f.render_widget(
            Paragraph::new(Span::from("  qué le encargas — ↑ para elegir con quién va").dim()),
            inner,
        );
        if focused {
            f.set_cursor_position((inner.x + 2, inner.y));
        }
        return;
    }

    let ancho = inner.width.saturating_sub(2).max(4) as usize;
    let (envuelto, _) = envolver_entrada(&d.tarea, ancho);
    let alto = inner.height as usize;
    let (fila_cursor, col_cursor) = posicion_cursor(&d.tarea, ancho, d.cursor);
    let mut desde = envuelto.len().saturating_sub(alto);
    if fila_cursor < desde {
        desde = fila_cursor;
    }
    if fila_cursor >= desde + alto {
        desde = fila_cursor + 1 - alto;
    }

    let lineas: Vec<Line> = envuelto
        .iter()
        .enumerate()
        .skip(desde)
        .map(|(i, l)| {
            Line::from(vec![
                Span::styled(
                    if i == 0 { "» " } else { "  " },
                    tema().estilo(Papel::Acento),
                ),
                Span::from(l.clone()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lineas), inner);

    // El cursor solo si el foco está aquí: con el foco en el shell, dejarlo
    // en la barra pondría el cursor del terminal donde no se está escribiendo.
    if focused {
        let y = inner.y + (fila_cursor - desde) as u16;
        let x = inner.x + 2 + col_cursor as u16;
        f.set_cursor_position((
            x.min(inner.x + inner.width.saturating_sub(1)),
            y.min(inner.y + inner.height.saturating_sub(1)),
        ));
    }
}

/// Los agentes que hay, desplegados sobre la barra.
///
/// La misma lista que ofrecía `/agente`, con el mismo criterio de columna: el
/// nombre realzado y la nota alineada detrás. Que se lea igual aquí y en el
/// menú no es simetría por simetría — es la misma decisión, y leerla de dos
/// formas distintas obliga a aprenderla dos veces.
fn draw_agentes(f: &mut Frame, d: &crate::app::Delegar, sel: usize, area: Rect) {
    if area.height == 0 {
        return;
    }
    let b = Block::default()
        .borders(Borders::TOP)
        .border_style(tema().estilo(Papel::Acento))
        .title(Span::from(" ↑↓ elegir · ↵ usar · Esc volver a la tarea ").dim());
    let inner = b.inner(area);
    f.render_widget(b, area);

    let alto = inner.height as usize;
    let ancho = inner.width as usize;
    if alto == 0 || ancho == 0 {
        return;
    }
    let desde = sel.saturating_sub(alto.saturating_sub(1));
    let visibles = || d.agentes.iter().skip(desde).take(alto);
    let ancho_nombre = visibles()
        .map(|p| p.nombre.chars().count())
        .max()
        .unwrap_or(8)
        .clamp(8, (ancho / 3).clamp(8, 32));

    let lineas: Vec<Line> = visibles()
        .enumerate()
        .map(|(fila, p)| {
            let estilo = if desde + fila == sel {
                Style::new().reversed().bold()
            } else {
                tema().estilo(Papel::Acento)
            };
            // El realce se ciñe al nombre, como en el menú: una barra en vídeo
            // inverso a lo ancho es mucha tinta para decir «esta fila».
            let relleno = " ".repeat(ancho_nombre.saturating_sub(p.nombre.chars().count()) + 1);
            let hueco = ancho.saturating_sub(ancho_nombre + 4);
            Line::from(vec![
                Span::styled(format!(" {} ", p.nombre), estilo),
                Span::from(relleno),
                Span::from(short(&p.detalle, hueco)).dim(),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lineas), inner);
}

/// El menú de comandos, filtrándose y seleccionable con las flechas.
///
/// Aparece solo: nadie tiene que enterarse por un manual de que existe
/// `/modelo`. Y funciona en tres niveles —el comando, sus argumentos y los
/// argumentos de esos— para que elegir un modelo sea señalarlo, no recordarlo.
///
/// La fila elegida va en vídeo inverso a propósito, y no en color: se ve
/// igual en 16 colores, en monocromo y con `NO_COLOR`.
///
/// Dos cosas que no son estéticas:
///
/// - **Las explicaciones van en columna.** Pegadas al nombre, cada fila
///   empezaba su descripción en un sitio distinto y la lista se leía como
///   siete frases sueltas en vez de como una tabla. El ancho sale del
///   contenido y no de un número fijo, igual que en la ventana de `/modelo`:
///   con un ancho de tabla, `opus` dejaba veinte espacios muertos delante de
///   su nota.
/// - **Un servidor no se pinta como uno de sus modelos.** `/modelo` ofrece
///   dos clases de cosa en la misma lista, y sin distinguirlas quedaban
///   cuatro nombres de servidor seguidos de tres modelos de uno de ellos, sin
///   nada que dijera cuál era cuál. Se marca igual que en la ventana: el
///   nombre en mayúsculas y una regla tenue hasta el borde.
fn draw_menu(f: &mut Frame, menu: &[slash::Candidato], sel: usize, area: Rect) {
    if area.height == 0 {
        return;
    }
    let b = Block::default()
        .borders(Borders::TOP)
        .border_style(tema().estilo(Papel::Acento))
        .title(Span::from(" ↑↓ elegir · → o Tab completar · ↵ usar ").dim());
    let inner = b.inner(area);
    f.render_widget(b, area);

    let alto = inner.height as usize;
    // Ventana deslizante: con la lista larga, la fila elegida no puede
    // quedarse fuera de lo que se ve.
    let desde = sel.saturating_sub(alto.saturating_sub(1));

    let ancho = inner.width as usize;
    // El ancho de la columna sale de lo que se ve, no de la lista entera: si
    // un nombre larguísimo está fuera de la ventana, no tiene por qué
    // empujar la columna de las filas que sí se leen.
    let visibles = || menu.iter().skip(desde).take(alto);
    let ancho_nombre = visibles()
        .filter(|c| !c.cabecera)
        .map(|c| c.etiqueta.chars().count())
        .max()
        .unwrap_or(8)
        // Sin pasar de un tercio del panel. En el primer nivel las etiquetas
        // van de «/web» a «/skills [instalar <de dónde>]», y alinear a la más
        // larga dejaría a las descripciones sin sitio, que es peor que la
        // columna torcida. Lo que no quepa en la columna no se rellena.
        .clamp(8, (ancho / 3).clamp(8, 32));

    let lineas: Vec<Line> = visibles()
        .enumerate()
        .map(|(fila, c)| {
            let elegido = desde + fila == sel;

            if c.cabecera && !elegido {
                // Sin vídeo inverso mientras no esté señalada: la regla ya la
                // separa, y una barra a lo ancho por cada servidor sería más
                // tinta de la que merece una cabecera.
                let usado = c.etiqueta.chars().count() + c.detalle.chars().count() + 4;
                let resto = ancho.saturating_sub(usado);
                let mut spans = vec![
                    Span::styled(
                        format!(" {} ", c.etiqueta.to_uppercase()),
                        tema().estilo(Papel::Acento).bold(),
                    ),
                    Span::from(format!("· {} ", short(&c.detalle, ancho / 2))).dim(),
                ];
                if resto > 0 {
                    spans.push(Span::from("─".repeat(resto)).dim());
                }
                return Line::from(spans);
            }

            let estilo = if elegido {
                Style::new().reversed().bold()
            } else {
                tema().estilo(Papel::Acento)
            };
            // El realce se ciñe al nombre y el relleno va detrás: una barra en
            // vídeo inverso del ancho de la columna sería mucha tinta para
            // decir «esta fila», y con nombres cortos tapa media línea.
            let n = c.etiqueta.chars().count();
            let relleno = " ".repeat(ancho_nombre.saturating_sub(n) + 1);
            let hueco = ancho.saturating_sub(ancho_nombre + 4);
            Line::from(vec![
                Span::styled(format!(" {} ", c.etiqueta), estilo),
                Span::from(relleno),
                Span::from(short(&c.detalle, hueco)).dim(),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lineas), inner);
}

/// Convierte un mensaje en líneas listas para pintar.
///
/// Separado del dibujado para poder cachearlo: es la parte cara, y la que se
/// repetía sin motivo.
/// La conversación compuesta, en texto plano.
///
/// Existe para poder **ver** el resultado sin abrir un terminal: el diseño de
/// esta parte se decide mirándola, y hacerlo con un pseudoterminal de por
/// medio convierte cada retoque en una expedición. La usan el banco de pruebas
/// `examples/vista.rs` y los tests de aquí abajo.
pub fn previa(chat: &[ChatMsg], ancho: usize) -> Vec<String> {
    chat.iter()
        .flat_map(|m| componer(m, ancho))
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// La conversación compuesta, **con sus colores**, para mirarla en cualquier
/// terminal.
///
/// [`previa`] aplana los estilos y para el texto vale, pero el color es la
/// mitad de las decisiones de esta parte —qué destaca, qué se apaga, qué tono
/// separa un comando de una espera— y a ojo desde el código no se decide
/// ninguna. Así el banco de pruebas enseña lo que se va a ver de verdad.
pub fn previa_color(chat: &[ChatMsg], ancho: usize) -> Vec<String> {
    use ratatui::style::{Color, Modifier};
    chat.iter()
        .flat_map(|m| componer(m, ancho))
        .map(|l| {
            let mut s = String::new();
            for span in &l.spans {
                let mut codigos: Vec<String> = Vec::new();
                match span.style.fg {
                    Some(Color::Indexed(n)) => codigos.push(format!("38;5;{n}")),
                    Some(Color::DarkGray) => codigos.push("90".into()),
                    Some(Color::Red) => codigos.push("31".into()),
                    Some(Color::Yellow) => codigos.push("33".into()),
                    _ => {}
                }
                if let Some(Color::Indexed(n)) = span.style.bg {
                    codigos.push(format!("48;5;{n}"));
                }
                if span.style.add_modifier.contains(Modifier::DIM) {
                    codigos.push("2".into());
                }
                if span.style.add_modifier.contains(Modifier::BOLD) {
                    codigos.push("1".into());
                }
                if codigos.is_empty() {
                    s.push_str(&span.content);
                } else {
                    s.push_str(&format!(
                        "\x1b[{}m{}\x1b[0m",
                        codigos.join(";"),
                        span.content
                    ));
                }
            }
            s
        })
        .collect()
}

/// El tema, detectado una vez.
///
/// Global y no un campo de `App` porque no cambia en toda la ejecución: sale
/// del entorno al arrancar. Pasarlo como parámetro hasta [`componer`]
/// obligaría además a meterlo en la clave de la caché para nada.
fn tema() -> &'static crate::tema::Tema {
    static T: std::sync::OnceLock<crate::tema::Tema> = std::sync::OnceLock::new();
    T.get_or_init(crate::tema::Tema::detectar)
}

fn componer(m: &ChatMsg, ancho: usize) -> Vec<Line<'static>> {
    let mut lineas: Vec<Line> = Vec::new();
    if m.text.trim().is_empty() && m.role == Role::Agent {
        return lineas;
    }
    // Quién habla se dice de tres maneras distintas, y ninguna es una etiqueta
    // con su nombre: el humano por el fondo de su texto, el agente por dos
    // puntos grises, y vsioncode por una barra fina. Tres marcas discretas
    // valen más que tres nombres repitiéndose en cada línea.
    //
    // Las tres ocupan lo mismo —tres columnas— para que el texto arranque
    // siempre en la misma, y el ojo no tenga que recolocarse en cada turno.
    let (marca, papel) = match m.role {
        Role::User => ("   ", Papel::Humano),
        Role::Agent => ("·· ", Papel::Agente),
        // Lo que dice vsioncode de sí mismo no lleva ninguna marca: es lo
        // menos importante de la pantalla y se distingue de sobra por el gris.
        // Cualquier glifo suyo es un adorno que se repite en cada aviso.
        Role::System => ("   ", Papel::Sistema),
        // La única marca de vsioncode en toda la pantalla, y se gana el sitio:
        // es la tecla que hay que pulsar. Ocupa tres columnas como las demás,
        // para que el texto siga arrancando siempre en la misma.
        Role::Espera => ("\u{21b5}  ", Papel::Espera),
    };
    // Los dos puntos del agente no se repiten en cada línea: la sangría ya
    // dice que el párrafo sigue siendo suyo.
    let sigue = "   ";
    let estilo = tema().estilo(papel);

    // Lo que el agente dice mientras trabaja pesa menos que lo que dice al
    // acabar, y la pantalla tiene que decirlo sola.
    //
    // La regla es la del propio ciclo y no una lista de casos: un turno que
    // trae comando es trabajo en curso —anuncia qué va a mirar y sigue—; el
    // turno que no trae ninguno es donde ha terminado y cuenta lo averiguado,
    // que es lo único que se relee. Atenuar el primero deja el resumen
    // destacando sin añadir ni una marca nueva a la pantalla.
    let base = match m.role {
        Role::System => Style::new().dim(),
        // Ni atenuado ni a plena luz: en su verde, que es lo que lo separa del
        // resto de lo que dice vsioncode.
        Role::Espera => tema().estilo(Papel::Espera),
        Role::Agent if !crate::ai::extract_commands(&m.text).is_empty() => Style::new().dim(),
        _ => Style::new(),
    };

    let mut lector = Lector::default();
    let mut primera = true;
    for cruda in m.text.lines() {
        let canal = |primera: &mut bool| -> [Span<'static>; 1] {
            if *primera {
                *primera = false;
                [Span::styled(marca, estilo)]
            } else {
                [Span::styled(sigue, estilo)]
            }
        };

        match lector.linea(cruda) {
            // El delimitador no se pinta: los ``` eran ruido puro.
            Linea::Delimitador { .. } => {}

            // Un comando se pinta tal cual y con una canaleta: es lo que vas
            // a ejecutar, así que tiene que verse distinto del texto y leerse
            // exactamente como se va a ejecutar.
            Linea::Comando(cmd) => {
                let est = tema().estilo(Papel::Comando);
                for trozo in envolver(&cmd, ancho.saturating_sub(5)) {
                    let mut spans = canal(&mut primera).to_vec();
                    spans.push(Span::styled("▌ ", est));
                    spans.push(Span::styled(trozo, est));
                    lineas.push(Line::from(spans));
                }
            }

            // Una línea de un cambio: fondo hasta el borde del panel, para
            // que el bloque se lea como un bloque y no como frases sueltas
            // con color. Es la misma razón por la que el texto del humano
            // lleva banda entera.
            Linea::Cambio { signo, texto } => {
                let f = match signo {
                    '-' => tema().estilo(Papel::Quitado),
                    '+' => tema().estilo(Papel::Puesto),
                    // El contexto no lleva fondo: si lo llevara, el ojo
                    // tendría que decidir en cada línea si cambió o no.
                    _ => Style::new().dim(),
                };
                // Una línea de código larga se envuelve, y la continuación
                // se sangra hasta donde empieza el código: si volviera al
                // margen, la columna de números —lo único con lo que se va
                // al fichero a mirarlo— quedaría salpicada de trozos de
                // frase y dejaría de ser una columna.
                let hueco = 1 + texto
                    .char_indices()
                    .find(|(_, c)| !c.is_ascii_digit() && *c != ' ')
                    .map_or(0, |(i, _)| texto[..i].chars().count());
                let cabe = ancho.saturating_sub(4 + hueco);
                let sangria = " ".repeat(hueco);
                for (i, fila) in envolver(&texto[hueco.saturating_sub(1)..], cabe)
                    .into_iter()
                    .enumerate()
                {
                    let mut spans = canal(&mut primera).to_vec();
                    let usado: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                    let cabeza = if i == 0 {
                        format!("{signo}{}", &texto[..hueco.saturating_sub(1)])
                    } else {
                        sangria.clone()
                    };
                    let largo = cabeza.chars().count() + fila.chars().count();
                    spans.push(Span::styled(cabeza, f));
                    spans.push(Span::styled(fila, f));
                    let resto = ancho.saturating_sub(usado + largo);
                    if resto > 0 {
                        spans.push(Span::styled(" ".repeat(resto), f));
                    }
                    lineas.push(Line::from(spans));
                }
            }

            Linea::Titulo(t) => {
                for trozo in envolver(&t, ancho.saturating_sub(4)) {
                    let mut spans = canal(&mut primera).to_vec();
                    spans.push(Span::styled(trozo, base.bold()));
                    lineas.push(Line::from(spans));
                }
            }

            Linea::Texto(trozos) => {
                if trozos.is_empty() {
                    // Una línea en blanco dentro de un mensaje conserva su
                    // barra: si se corta, el bloque se parte en dos a la vista.
                    lineas.push(Line::from(canal(&mut primera).to_vec()));
                    continue;
                }
                // Lo que escribe el humano va sobre un fondo tenue que ocupa
                // el panel de punta a punta, y no solo lo que mide el texto:
                // una banda entera se lee como un bloque, mientras que un
                // fondo recortado a cada frase deja la conversación con la
                // silueta irregular de un mensaje de móvil.
                let fondo = (m.role == Role::User).then(|| tema().estilo(Papel::FondoHumano));
                for fila in envolver_trozos(&trozos, ancho.saturating_sub(5), base) {
                    let mut spans = canal(&mut primera).to_vec();
                    match fondo {
                        Some(f) => {
                            let usado: usize =
                                spans.iter().map(|s| s.content.chars().count()).sum();
                            let texto: usize = fila.iter().map(|s| s.content.chars().count()).sum();
                            spans.push(Span::styled(" ", f));
                            spans.extend(fila.into_iter().map(|s| {
                                let contenido = s.content.into_owned();
                                Span::styled(contenido, s.style.patch(f))
                            }));
                            // Hasta el borde. Sin esto, cada línea acaba donde
                            // acaba su frase y el bloque se ve dentado.
                            let resto = ancho.saturating_sub(usado + texto + 1);
                            if resto > 0 {
                                spans.push(Span::styled(" ".repeat(resto), f));
                            }
                        }
                        None => spans.extend(fila),
                    }
                    lineas.push(Line::from(spans));
                }
            }
        }
    }
    lineas.push(Line::from(""));
    lineas
}

/// Líneas ya compuestas de un mensaje, con lo necesario para saber si valen.
pub struct Cacheado {
    ancho: usize,
    largo: usize,
    lineas: Vec<Line<'static>>,
}

/// Guarda las líneas ya envueltas de cada mensaje.
///
/// Sin esto, el panel volvía a envolver **toda** la conversación en cada
/// fotograma, hasta treinta veces por segundo, cuando lo único que cambiaba
/// era el último mensaje que estaba llegando. Con una charla larga eso se
/// nota: el streaming empieza a ir a trompicones justo cuando más fluidez
/// hace falta.
///
/// La validez se decide por el ancho y por la longitud del texto. La longitud
/// basta porque un mensaje solo cambia de una forma: creciendo por el final
/// mientras se recibe. Un hash sería más general y más caro, y aquí no
/// compraría nada.
#[derive(Default)]
pub struct CacheChat {
    entradas: Vec<Cacheado>,
}

impl CacheChat {
    pub fn lineas(&mut self, chat: &[ChatMsg], ancho: usize) -> Vec<Line<'static>> {
        // Si se han quitado mensajes (un `/limpiar`), sobra todo lo guardado.
        if self.entradas.len() > chat.len() {
            self.entradas.clear();
        }

        for (i, m) in chat.iter().enumerate() {
            let vale = self
                .entradas
                .get(i)
                .is_some_and(|c| c.ancho == ancho && c.largo == m.text.len());
            if vale {
                continue;
            }
            let nuevo = Cacheado {
                ancho,
                largo: m.text.len(),
                lineas: componer(m, ancho),
            };
            match self.entradas.get_mut(i) {
                Some(hueco) => *hueco = nuevo,
                None => self.entradas.push(nuevo),
            }
        }

        self.entradas
            .iter()
            .flat_map(|c| c.lineas.iter().cloned())
            .collect()
    }
}

fn draw_chat(f: &mut Frame, app: &App, area: Rect) {
    let ancho = area.width.saturating_sub(1).max(10) as usize;
    let lineas = app.cache_chat.borrow_mut().lineas(&app.chat, ancho);

    // Anclado abajo: lo último dicho es lo que importa.
    let total = lineas.len() as u16;
    let visible = area.height;
    let max_scroll = total.saturating_sub(visible);
    let offset = max_scroll.saturating_sub(app.chat_scroll.min(max_scroll));

    f.render_widget(Paragraph::new(lineas).scroll((offset, 0)), area);
}

fn envolver(texto: &str, ancho: usize) -> Vec<String> {
    if texto.trim().is_empty() {
        return vec![String::new()];
    }
    textwrap::wrap(texto, ancho.max(8))
        .into_iter()
        .map(|c| c.into_owned())
        .collect()
}

/// Ajusta al ancho conservando el estilo de cada trozo.
///
/// Envolver primero y estilar después perdería el marcado; estilar primero y
/// no envolver cortaría las líneas largas. Hay que hacer las dos cosas a la
/// vez, palabra a palabra.
fn envolver_trozos(trozos: &[Trozo], ancho: usize, base: Style) -> Vec<Vec<Span<'static>>> {
    let ancho = ancho.max(8);
    let mut filas: Vec<Vec<Span<'static>>> = Vec::new();
    let mut fila: Vec<Span<'static>> = Vec::new();
    let mut usado = 0usize;

    for trozo in trozos {
        let (texto, estilo) = match trozo {
            Trozo::Normal(t) => (t.as_str(), base),
            Trozo::Fuerte(t) => (t.as_str(), base.bold()),
            // El código en línea suele ser un comando o una ruta: en color se
            // distingue de la prosa de un vistazo, que es justo para lo que
            // el autor puso las comillas.
            Trozo::Codigo(t) => (t.as_str(), base.patch(tema().estilo(Papel::Codigo))),
        };

        for (i, palabra) in texto.split_inclusive(' ').enumerate() {
            let n = palabra.chars().count();
            if usado + n > ancho && usado > 0 {
                filas.push(std::mem::take(&mut fila));
                usado = 0;
            }
            // Una palabra sola más larga que el ancho se parte, en vez de
            // desbordar la línea.
            if n > ancho {
                for pedazo in partir(palabra, ancho) {
                    let l = pedazo.chars().count();
                    if usado + l > ancho && usado > 0 {
                        filas.push(std::mem::take(&mut fila));
                        usado = 0;
                    }
                    fila.push(Span::styled(pedazo, estilo));
                    usado += l;
                }
                continue;
            }
            let _ = i;
            fila.push(Span::styled(palabra.to_string(), estilo));
            usado += n;
        }
    }

    if !fila.is_empty() {
        filas.push(fila);
    }
    if filas.is_empty() {
        filas.push(vec![Span::from("")]);
    }
    filas
}

fn partir(s: &str, ancho: usize) -> Vec<String> {
    let cs: Vec<char> = s.chars().collect();
    cs.chunks(ancho.max(1))
        .map(|c| c.iter().collect())
        .collect()
}

fn draw_proposal(f: &mut Frame, app: &App, area: Rect) {
    let Some(p) = &app.proposal else { return };

    let (color, glifo) = match p.risk {
        Risk::Safe => (tema().estilo(Papel::Exito), "✓"),
        Risk::Caution => (Style::new().yellow(), "!"),
        Risk::Danger => (Style::new().red(), "✗"),
    };

    let pie = match p.risk {
        Risk::Danger => "no lo inserto: cópialo a mano si estás seguro".to_string(),
        _ => "Alt+↵ ponerlo en mi línea · Ctrl+N descartar".to_string(),
    };

    let mut lineas = vec![Line::from(vec![
        Span::styled(format!("{glifo} "), color.bold()),
        Span::from(short(&p.cmd, area.width.saturating_sub(6) as usize)).bold(),
    ])];
    if let Some(r) = &p.reason {
        lineas.push(Line::from(Span::from(format!("  {r}")).dim()));
    }
    lineas.push(Line::from(Span::from(pie).dim()));

    let b = Block::default()
        .borders(Borders::TOP)
        .border_style(color)
        .title(Span::from(format!(" propone · {} ", p.risk.label())).style(color));
    let inner = b.inner(area);
    f.render_widget(b, area);
    f.render_widget(Paragraph::new(lineas), inner);
}

/// Envuelve el texto de la caja de escritura **conservando los espacios**.
///
/// Los envolvedores normales recortan el espacio del final de cada línea, y
/// para pintar un párrafo está bien. Para una caja donde se escribe, no: si al
/// pulsar la barra espaciadora el texto envuelto no cambia, el cursor no se
/// mueve, y al escribir la letra siguiente da un salto. Se nota mucho y parece
/// que la interfaz va a trompicones.
///
/// Un `\n` corta línea de verdad: la caja recibe texto pegado de fuera y ahí
/// los saltos vienen dentro.
///
/// Devuelve las líneas y dónde queda el cursor, que es el final del texto.
fn envolver_entrada(texto: &str, ancho: usize) -> (Vec<String>, (usize, usize)) {
    let ancho = ancho.max(4);
    let mut lineas: Vec<String> = Vec::new();
    let mut actual = String::new();
    let mut usado = 0usize;

    // Un pegado de varias líneas trae saltos dentro, y un salto que viaja
    // como una letra más no mide ninguna columna: las tres líneas acababan
    // amontonadas en una y los saltos pintados como glifos raros. Aquí corta
    // de verdad.
    for (i, parrafo) in texto.split('\n').enumerate() {
        if i > 0 {
            lineas.push(std::mem::take(&mut actual));
            usado = 0;
        }

        // El resto de caracteres de control llegan por la misma vía —el `\t`
        // de un pegado, el `\r` de un CRLF— y tampoco tienen ancho. Se pintan
        // como una columna en blanco: el terminal no sabe dibujarlos, pero el
        // hueco que traía el texto se conserva y el cursor sigue cuadrando.
        let parrafo: std::borrow::Cow<str> = if parrafo.contains(char::is_control) {
            parrafo
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect::<String>()
                .into()
        } else {
            parrafo.into()
        };

        // Se parte por palabras conservando el espacio que las separa, así que
        // «hola » ocupa cinco columnas y no cuatro.
        for trozo in parrafo.split_inclusive(' ') {
            let palabra: String = trozo.trim_end_matches(' ').to_string();
            let espacios = trozo.chars().count() - palabra.chars().count();
            let ancho_palabra = ancho_texto(&palabra);

            // La palabra no cabe en lo que queda: se baja de línea.
            if usado > 0 && usado + ancho_palabra > ancho {
                lineas.push(std::mem::take(&mut actual));
                usado = 0;
            }

            // Y si no cabe ni en una línea entera, se parte.
            if ancho_palabra > ancho {
                for c in palabra.chars() {
                    let w = ancho_char(c);
                    if usado + w > ancho {
                        lineas.push(std::mem::take(&mut actual));
                        usado = 0;
                    }
                    actual.push(c);
                    usado += w;
                }
            } else {
                actual.push_str(&palabra);
                usado += ancho_palabra;
            }

            for _ in 0..espacios {
                if usado >= ancho {
                    lineas.push(std::mem::take(&mut actual));
                    usado = 0;
                }
                actual.push(' ');
                usado += 1;
            }
        }
    }

    lineas.push(actual);

    // El cursor va al final del texto. Si justo llena la línea, pasa al
    // principio de la siguiente, como en cualquier terminal.
    let mut fila = lineas.len() - 1;
    let mut col = ancho_texto(&lineas[fila]);
    if col >= ancho {
        lineas.push(String::new());
        fila += 1;
        col = 0;
    }
    (lineas, (fila, col))
}

/// (fila, columna) donde queda el cursor de caracteres `cursor` cuando el
/// texto se envuelve a `ancho` columnas.
///
/// Envolver el prefijo hasta el cursor da su posición exacta: el algoritmo
/// de wrap es estable respecto al prefijo, y la fila fantasma final de
/// `envolver_entrada` lo baja a la línea siguiente cuando la actual queda
/// llena — igual que hace el cursor «al final» de la caja.
pub fn posicion_cursor(texto: &str, ancho: usize, cursor: usize) -> (usize, usize) {
    let byte = crate::app::byte_de(texto, cursor);
    envolver_entrada(&texto[..byte], ancho).1
}

/// Índice de caracteres al que se mueve el cursor al subir o bajar una línea
/// visual (`delta` -1 o 1). Se conserva la columna salvo que la línea
/// destino sea más corta, como en cualquier editor.
pub fn cursor_vertical(texto: &str, ancho: usize, cursor: usize, delta: i32) -> usize {
    let n = texto.chars().count();
    let posiciones: Vec<(usize, usize)> =
        (0..=n).map(|i| posicion_cursor(texto, ancho, i)).collect();
    let (fila, col) = posiciones[cursor.min(n)];
    let ultima = posiciones[n].0;
    let destino = (fila as i32 + delta).clamp(0, ultima as i32) as usize;
    // Entre los índices que caen en la línea destino, el de columna más
    // cercana; si la línea es más corta que nuestra columna, su final.
    (0..=n)
        .filter(|i| posiciones[*i].0 == destino)
        .min_by_key(|i| (posiciones[*i].1.abs_diff(col), *i))
        .unwrap_or(cursor)
}

fn ancho_char(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

fn ancho_texto(s: &str) -> usize {
    s.chars().map(ancho_char).sum()
}

fn draw_input(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    if area.height == 0 {
        return;
    }
    let b = Block::default()
        .borders(Borders::TOP)
        .border_style(if focused {
            tema().estilo(Papel::Acento)
        } else {
            Style::new().dark_gray()
        });
    let inner = b.inner(area);
    f.render_widget(b, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if app.input.is_empty() && !focused {
        let pista = if app.has_brain() {
            "Alt+A para preguntar · Alt+E para que te explique lo último"
        } else {
            "sin agente conectado"
        };
        f.render_widget(Paragraph::new(Span::from(pista).dim()), inner);
        return;
    }

    // Dictando: se marca con el micro y en cursiva, para que se vea de un
    // vistazo que eso lo ha escrito tu voz y todavía no se ha enviado — que es
    // justo el momento de corregirlo si se equivocó.
    let dictando = app.voz_encendida() && !app.input.is_empty();
    let (marca, estilo) = if dictando {
        ("🎙 ", Style::new().italic())
    } else {
        ("› ", Style::new())
    };

    // La caja tiene tres líneas y el texto puede ser mucho más largo. Sin
    // desplazar, lo que escribes desaparece por abajo y te quedas escribiendo
    // a ciegas — que es exactamente lo que pasaba.
    // Se reservan dos columnas para la marca de la izquierda.
    let ancho = inner.width.saturating_sub(2).max(4) as usize;
    let (envuelto, _) = envolver_entrada(&app.input, ancho);

    let alto = inner.height as usize;
    // El ancho de la caja es lo que ↑/↓ necesitan para saber dónde cortan
    // las líneas; el dibujo es quien lo conoce, así que lo deja anotado.
    app.ancho_entrada.set(ancho);

    // El cursor manda: la ventana de líneas se recorta para que la fila del
    // cursor esté siempre a la vista, anclada al final cuando puede.
    let (fila_cursor, col_cursor) = posicion_cursor(&app.input, ancho, app.cursor);
    let mut desde = envuelto.len().saturating_sub(alto);
    if fila_cursor < desde {
        desde = fila_cursor;
    }
    if fila_cursor >= desde + alto {
        desde = fila_cursor + 1 - alto;
    }

    let lineas: Vec<Line> = envuelto
        .iter()
        .enumerate()
        .skip(desde)
        .map(|(i, l)| {
            let prefijo = if i == 0 { marca } else { "  " };
            Line::from(vec![
                Span::styled(prefijo, tema().estilo(Papel::Acento)),
                Span::styled(l.clone(), estilo),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lineas), inner);

    if focused {
        let y = inner.y + (fila_cursor - desde) as u16;
        let x = inner.x + 2 + col_cursor as u16;
        f.set_cursor_position((
            x.min(inner.x + inner.width.saturating_sub(1)),
            y.min(inner.y + inner.height.saturating_sub(1)),
        ));
    }
}

// -------------------------------------------------------------------- barra

/// Separador entre grupos de la barra.
///
/// Todos los trozos acaban ya en un espacio, así que esto solo añade la
/// barrita y su aire. Va tenue a propósito: tiene que separar sin competir
/// con lo que separa. Antes la barra era una tira seguida —modo, captura,
/// cwd, comando, LIBRE, tokens— pegada con un espacio cada cosa, y a ojo no
/// se veía dónde acababa una y empezaba la siguiente.
fn sep(spans: &mut Vec<Span<'static>>) {
    if !spans.is_empty() {
        spans.push(Span::from("│ ").dim());
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();

    // Modo, en video inverso: se lee de un vistazo aunque no haya color.
    let (modo, papel) = match app.mode {
        Mode::Observar => (" OBSERVAR ", Papel::ModoObservar),
        Mode::Sugerir => (" SUGERIR ", Papel::ModoSugerir),
        Mode::Conversar => (" CONVERSAR ", Papel::ModoConversar),
        // Con marca además del color: cuando el agente puede lanzar comandos
        // por su cuenta, eso no puede pasar desapercibido.
        Mode::Ejecutar => (" ▶ EJECUTAR ", Papel::ModoEjecutar),
    };
    spans.push(Span::styled(modo, tema().estilo(papel).bold()));

    // Estado de la captura: permanente, nunca un aviso que se desvanece. Si
    // vsioncode ve lo que escribes, eso se mira, no se recuerda.
    sep(&mut spans);
    spans.push(if app.capture {
        Span::styled(" \u{25cf} captura ", tema().estilo(Papel::Exito))
    } else {
        Span::styled(" \u{23f8} PAUSADA ", tema().estilo(Papel::Aviso).bold())
    });

    // El directorio va con la captura y no en su propio grupo: los dos dicen
    // de qué terminal estamos hablando.
    spans.push(Span::from(format!("{} ", short_path(app.ledger.cwd()))).dim());

    // Resultado del ultimo comando: glifo mas numero, no solo color, para que
    // se lea igual con daltonismo, en monocromo o con NO_COLOR.
    if app.ledger.last().is_some() {
        sep(&mut spans);
    }
    let fallo = match app.ledger.last() {
        Some(ev) => match ev.exit {
            Some(0) => {
                spans.push(Span::styled(
                    format!("\u{2713} {} ", short(&ev.cmd, 22)),
                    tema().estilo(Papel::Exito),
                ));
                // Cuánto tardó, y solo si tardó. Poner «0s» detrás de cada
                // `ls` llena la barra de un número que nunca dice nada, y
                // entonces tampoco se lee el día que el build tarda tres
                // minutos.
                if ev.duration.as_secs() >= 1 {
                    spans.push(Span::from(format!("{} ", duracion(ev.duration.as_secs()))).dim());
                }
                false
            }
            Some(c) => {
                spans.push(Span::styled(
                    format!("\u{2717} {c} {} ", short(&ev.cmd, 20)),
                    tema().estilo(Papel::Error).bold(),
                ));
                true
            }
            None => {
                spans.push(Span::from(format!("\u{b7} {} ", short(&ev.cmd, 22))).dim());
                false
            }
        },
        None => false,
    };

    // Cuando algo falla se ofrece **una** accion. Ofrecer cinco es no ofrecer
    // ninguna: la barra pasa a ser decoracion y se deja de leer.
    if fallo && app.has_brain() && !app.thinking {
        spans.push(Span::styled(
            " Alt+E explicar ",
            tema().estilo(Papel::Acento).bold(),
        ));
    }

    push_tail(&mut spans, app);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn push_tail(spans: &mut Vec<Span<'static>>, app: &App) {
    // Cuando el agente está lanzando comandos por su cuenta, se dice. No es
    // un detalle: es la diferencia entre una herramienta que trabaja para ti
    // y una que hace cosas en tu máquina sin que lo sepas.
    // Subagentes trabajando: se dice cuántos y en qué. Lanzar procesos que
    // cuestan dinero sin que se vea sería justo lo contrario de lo que hace
    // esta herramienta.
    if !app.subagentes.is_empty() {
        sep(spans);
        // El título solo cuando no hay lista. Con lista, repetir aquí el del
        // primero enseñaría dos veces lo mismo y, peor, sin decir cuál es.
        let titulo = match filas_subagentes(app) {
            0 => app.subagentes.first().map(|s| s.titulo.as_str()),
            _ => None,
        };
        spans.extend(spans_subagentes(
            app.subagentes.len(),
            app.sub_desde,
            titulo,
        ));
    }

    // Modo libre: se ve, porque cambia cuánto puede hacer solo.
    if app.libre {
        sep(spans);
        spans.push(Span::styled(
            " LIBRE ",
            tema().estilo(Papel::Aviso).reversed().bold(),
        ));
    }
    if app.pide_libre && !app.libre {
        sep(spans);
        spans.push(Span::styled(
            " pide libre · Alt+L ",
            tema().estilo(Papel::Aviso).bold(),
        ));
    }

    // El plan, en la barra y no solo en la conversación: la conversación se va
    // hacia arriba en cuanto pasan tres comandos, y «por qué paso va» no es un
    // aviso que ocurrió, es en qué punto está el trabajo ahora mismo. Es el
    // mismo motivo por el que aquí está «te toca a ti».
    let plan = spans_plan(app.plan.as_ref());
    if !plan.is_empty() {
        sep(spans);
        spans.extend(plan);
    }

    if app.auto.activo() {
        sep(spans);
        spans.push(Span::styled(
            format!(" ▶ ejecutando {}/{} ", app.auto.pasos, app.auto.max),
            tema().estilo(Papel::ModoEjecutar).bold(),
        ));
        spans.push(Span::styled(" Esc corta ", tema().estilo(Papel::Tenue)));
    }

    // El estado del micrófono, permanente mientras esté encendido. Igual que
    // con la captura: si vsioncode está oyendo la habitación, eso se mira, no
    // se recuerda.
    if app.oyendo {
        sep(spans);
        spans.push(Span::from(" ● oyendo ").on_red().bold());
    } else if app.voz_encendida() && !app.input.is_empty() {
        // Ha dejado de hablar pero aún no ha terminado el turno: se dice, para
        // que la espera no parezca que se ha colgado.
        sep(spans);
        spans.push(Span::styled(
            " sigue o calla para enviar ",
            tema().estilo(Papel::Acento),
        ));
    }

    // Buscar tarda segundos: decir «pensando» mientras el agente está leyendo
    // una página es mentir sobre en qué se va el tiempo.
    if let Some(q) = &app.buscando
        && app.thinking
    {
        sep(spans);
        spans.push(Span::styled(
            format!(" ⌕ {} ", short(q, 34)),
            tema().estilo(Papel::Acento).bold(),
        ));
    }

    if app.thinking {
        let secs = app
            .thinking_since
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        sep(spans);
        spans.push(
            Span::from(format!(
                "{} pensando {} ",
                giro(app.thinking_since),
                duracion(secs)
            ))
            .magenta(),
        );
    } else if app.esperando_tu_mano() {
        // El estado siguiente al spinner, en su mismo sitio: pensando → te
        // toca. Puesto aquí y no solo en la conversación porque un mensaje
        // sube con el scroll y esto no es un aviso que pasó, es en qué punto
        // está el trabajo ahora mismo.
        sep(spans);
        spans.push(Span::styled(
            " \u{21b5} te toca a ti · Alt+\u{21b5} ",
            tema().estilo(Papel::Espera).bold(),
        ));
    }

    // Lo gastado, siempre a la vista mientras haya algo que contar.
    //
    // No es un aviso: es un instrumento, como el `cwd` o el estado de la
    // captura. La cuota de este proyecto se va por la entrada —el contexto
    // que se reenvía en cada turno—, y ese número solo se veía escribiendo
    // `/coste`, o sea después de que ya se hubiera ido. Aquí se ve subir.
    if !app.uso.vacio() {
        sep(spans);
        spans.push(
            Span::from(format!(
                "\u{2191}{} \u{2193}{} ",
                tokens(app.uso.entrada_total()),
                tokens(app.uso.salida),
            ))
            .dim(),
        );
    }

    // Estos tres traían su propio «·» de cuando eran los únicos que se
    // separaban de algo. Ahora la barrita la pone `sep` para todos, y dos
    // separadores seguidos se leen como un error de pintado.
    if let Some((msg, _)) = &app.notice {
        sep(spans);
        spans.push(Span::from(format!("{msg} ")).yellow());
    }

    if let Some(q) = &app.quota
        && !q.ends_with("allowed")
    {
        sep(spans);
        spans.push(Span::from(format!("cuota {q} ")).yellow());
    }
    sep(spans);
    spans.push(Span::from("Alt+H ayuda").dim());
}

// -------------------------------------------------------------------- ayuda

fn draw_help(f: &mut Frame, area: Rect) {
    let w = 74u16.min(area.width.saturating_sub(4));
    let h = (HELP.len() as u16 + 6).min(area.height.saturating_sub(2));
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);

    f.render_widget(ratatui::widgets::Clear, popup);

    let mut lines = vec![
        Line::from(Span::from("Ctrl sigue siendo del shell. Los atajos van con Alt.").dim()),
        Line::from(""),
    ];
    for b in HELP {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<14}", b.keys),
                tema().estilo(Papel::Acento).bold(),
            ),
            Span::from(b.what),
        ]));
    }

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(tema().estilo(Papel::Acento))
        .title(Span::from(" vsioncode ").bold());
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// -------------------------------------------------------------------- varios

/// Un rato, en la unidad en la que se piensa.
///
/// `pensando 1490s` no se lee: hay que dividir mentalmente para saber si son
/// dos minutos o veinticinco. Y en modo libre un turno de veinte minutos es
/// normal, así que el caso de los minutos no es el raro.
fn duracion(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        _ => format!("{}m {}s", secs / 60, secs % 60),
    }
}

/// Un recuento de tokens en tres o cuatro columnas.
///
/// En la barra no cabe `1.234.567` ni hace falta: lo que se mira ahí es el
/// orden de magnitud —si vas por cien mil o por dos millones—, y para el
/// detalle está `/coste`.
fn tokens(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Cuántas filas se lleva la lista de subagentes.
///
/// Con uno solo no sale ninguna: la barra ya lo dice entero en su línea, y una
/// lista de un elemento es un marco alrededor de nada. Desde dos, el título
/// suelto de la barra dejaba de servir —decía qué hace *uno* de ellos, sin
/// decir cuál— y esa era justo la pregunta.
///
/// Pública porque el ratón necesita el mismo número: las zonas se calculan
/// aparte del dibujado, y si discreparan, un clic caería en el sitio de al
/// lado.
pub fn filas_subagentes(app: &App) -> u16 {
    let total = app.subagentes.len() + app.en_cola().len();
    if app.subagentes.len() < 2 && total < 2 {
        return 0;
    }
    total.min(MAX_SUBS) as u16
}

/// La lista de subagentes, encima de la barra de estado.
///
/// Encima y no dentro del panel del agente: ahí abajo se pelean por el sitio
/// el menú de barra, la lista de delegar y la propuesta de comando —que sale
/// justo mientras los subagentes trabajan—, y lo que se tapa unos a otros no
/// se puede mirar. Aquí tiene sitio propio y aparece pegada a la barra, que es
/// donde ya se mira el estado de todo lo demás.
fn draw_subagentes(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 || area.width < 12 {
        return;
    }
    let cola = app.en_cola();
    let lineas = lineas_subagentes(&app.subagentes, &cola, area.width, area.height);
    f.render_widget(Paragraph::new(lineas), area);
}

/// Lo que dice la lista, aparte de dónde se pinta.
///
/// Separado para poder mirarlo sin levantar una `App`: el alineado de esta
/// lista solo se juzga viéndola, y la `App` arrastra un pseudoterminal.
fn lineas_subagentes(
    vivos: &[crate::app::Subagente],
    cola: &[&str],
    ancho: u16,
    alto: u16,
) -> Vec<Line<'static>> {
    let sitio = alto as usize;
    let total = vivos.len() + cola.len();
    // Si no caben todos, la última fila es para el resumen: enseñar cuatro de
    // siete sin decir que hay siete es peor que enseñar tres y decirlo. Con
    // una sola fila no, que entonces el resumen se comería la lista entera
    // para repetir el recuento que ya está en la barra.
    let cabe = match total > sitio && sitio > 1 {
        true => sitio - 1,
        false => sitio,
    };

    let mut v: Vec<Line<'static>> = Vec::new();
    for s in vivos.iter().take(cabe) {
        v.push(linea_subagente(
            &s.titulo,
            Some(&s.modelo),
            None,
            Some(s.desde),
            ancho,
        ));
    }
    for t in cola.iter().take(cabe.saturating_sub(v.len())) {
        v.push(linea_subagente(t, None, Some("en cola"), None, ancho));
    }
    if total > v.len() {
        v.push(Line::from(
            Span::from(format!("   +{} más ", total - v.len())).dim(),
        ));
    }
    v
}

/// Una fila de la lista: quién, en qué anda y cuánto lleva.
///
/// El tiempo va a la derecha y en columna, no detrás del título: con tres o
/// cuatro a la vez, lo que se busca de un vistazo es cuál se está eternizando,
/// y eso solo se ve si los números están alineados.
fn linea_subagente(
    titulo: &str,
    modelo: Option<&str>,
    etiqueta: Option<&str>,
    desde: Option<std::time::Instant>,
    ancho: u16,
) -> Line<'static> {
    let activo = desde.is_some();
    let ancho = ancho as usize;

    // La columna de la derecha, de lo prescindible a lo que no lo es. El
    // modelo se cae en un panel estrecho —es el mismo para todos casi
    // siempre—; la etiqueta y el tiempo, nunca: son lo que distingue una fila
    // de otra.
    let mut derecha: Vec<String> = Vec::new();
    if let Some(m) = modelo
        && ancho >= 46
    {
        derecha.push(m.to_string());
    }
    if let Some(e) = etiqueta {
        derecha.push(e.to_string());
    }
    if let Some(t) = desde
        && t.elapsed().as_secs() >= 1
    {
        derecha.push(duracion(t.elapsed().as_secs()));
    }
    let derecha = match derecha.is_empty() {
        true => String::new(),
        false => format!("{} ", derecha.join(" · ")),
    };

    // Tres columnas de margen —el aire de la izquierda y la marca— y una de
    // separación que no se negocia: un título que llega a tocar la columna de
    // la derecha se lee como una sola palabra pegada.
    let hueco = ancho.saturating_sub(derecha.chars().count() + 3);
    let titulo = short(titulo, hueco.saturating_sub(1).max(8));
    let relleno = " ".repeat(hueco.saturating_sub(titulo.chars().count()).max(1));

    // El giro solo en los que trabajan. Un dibujo de trabajo sobre algo que
    // espera turno es exactamente el aviso que no toca.
    let marca = match activo {
        true => Span::styled(format!(" {} ", giro(desde)), tema().estilo(Papel::Acento)),
        false => Span::from(" · ").dim(),
    };
    let titulo = match activo {
        true => Span::from(titulo),
        false => Span::from(titulo).dim(),
    };

    Line::from(vec![
        marca,
        titulo,
        Span::from(relleno),
        Span::from(derecha).dim(),
    ])
}

/// Lo que dice la barra del plan, sin dónde se pinta.
///
/// Aparte del dibujo por lo mismo que [`spans_subagentes`]: montar una `App`
/// pide un pseudoterminal y un cerebro vivos, y entonces lo que dice la barra
/// deja de tener prueba y se decide a ojo una vez y nunca más.
fn spans_plan(fase: Option<&crate::plan::Fase>) -> Vec<Span<'static>> {
    match fase {
        None => Vec::new(),
        // Trazar puede llevar varios turnos de lectura, y durante ellos la
        // barra solo diría «pensando». Esto dice pensando en qué.
        Some(crate::plan::Fase::Trazando) => vec![Span::styled(
            " trazando el plan ",
            tema().estilo(Papel::Acento).bold(),
        )],
        // Con la tecla puesta, que es lo único que puede pasar ahora: el
        // trabajo está parado esperando esa decisión.
        Some(crate::plan::Fase::Propuesto(p)) => vec![Span::styled(
            format!(
                " plan: {} · Alt+\u{21b5} ",
                plural(p.pasos.len(), "paso", "pasos")
            ),
            tema().estilo(Papel::Espera).bold(),
        )],
        // Sin ningún paso marcado no se pinta «0/7»: sería decir que va por el
        // paso cero, y por el paso cero no va nadie. Lo que se sabe es cuántos
        // pasos tiene, y eso es lo que se dice.
        Some(crate::plan::Fase::Marcha { plan, paso }) => {
            let t = if *paso == 0 {
                format!(" plan · {} ", plural(plan.pasos.len(), "paso", "pasos"))
            } else {
                format!(" plan {paso}/{} ", plan.pasos.len())
            };
            vec![Span::styled(t, tema().estilo(Papel::ModoEjecutar).bold())]
        }
    }
}

/// Lo que enseña la barra cuando hay subagentes trabajando.
///
/// Aparte para poder pintarlo y mirarlo sin levantar una `App` entera: esto se
/// decide con los ojos —qué destaca, qué se apaga—, y a través de un
/// pseudoterminal cada retoque sería una expedición.
///
/// Sin chapa de color. En esta barra el fondo es el distintivo del modo y de
/// nada más: una segunda mancha al lado se lee como si hubiera dos modos
/// puestos, y el color que llevaba —el morado de conversar— decía además el
/// modo equivocado.
fn spans_subagentes(
    n: usize,
    desde: Option<std::time::Instant>,
    titulo: Option<&str>,
) -> Vec<Span<'static>> {
    let mut v = Vec::new();
    // Que están vivos lo dice el mismo giro que el de «pensando», porque es lo
    // mismo que pasa: hay alguien trabajando. Dos dibujos distintos para el
    // mismo hecho harían creer que son dos cosas distintas.
    v.push(Span::styled(
        format!("{} ", giro(desde)),
        tema().estilo(Papel::Acento),
    ));
    v.push(Span::styled(
        format!("{} ", plural(n, "subagente", "subagentes")),
        tema().estilo(Papel::Acento).bold(),
    ));
    // Cuánto llevan fuera. Mientras trabajan, `thinking` está apagado y no hay
    // ningún otro número moviéndose en la pantalla: sin este no se distingue
    // «tarda» de «se ha colgado». Y solo a partir del segundo, como el de los
    // comandos: un «0s» detrás de cada reparto es un número que no dice nada.
    if let Some(t) = desde
        && t.elapsed().as_secs() >= 1
    {
        v.push(Span::from(format!("{} ", duracion(t.elapsed().as_secs()))).dim());
    }
    // En qué anda el primero, atenuado y detrás: el estado es cuántos hay, el
    // título es el detalle. Antes iba en morado y pegado a la chapa, y se leía
    // como si fuera parte de ella.
    if let Some(t) = titulo {
        v.push(Span::from(format!("\u{b7} {} ", short(t, 28))).dim());
    }
    v
}

/// El giro que dice que algo está corriendo ahora mismo.
///
/// Uno solo para los dos sitios que lo enseñan —el cerebro pensando y los
/// subagentes trabajando fuera—: desde la barra son el mismo hecho, y darles
/// dibujos distintos haría creer que son dos cosas distintas. Va en la barra y
/// no dentro del texto porque un giro inline obliga a recalcular el ajuste de
/// línea en cada fotograma.
fn giro(desde: Option<std::time::Instant>) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let i = (desde.map(|t| t.elapsed().as_millis() / 80).unwrap_or(0) as usize) % FRAMES.len();
    FRAMES[i]
}

fn short(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    format!("{}…", s.chars().take(keep).collect::<String>())
}

/// Acorta la ruta a la de casa con `~` y, si aún es larga, deja las dos
/// últimas partes. `/home/newt/proyectos/x/y/z` → `~/…/y/z`.
fn short_path(p: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.display().to_string())
        .unwrap_or_default();
    let p = if !home.is_empty() && p.starts_with(&home) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    };
    if p.chars().count() <= 26 {
        return p;
    }
    let partes: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    match partes.len() {
        0 | 1 => p,
        n => format!("~/…/{}", partes[n - 2..].join("/")),
    }
}

#[cfg(test)]
mod tests_composicion {
    use super::*;
    use crate::app::{ChatMsg, Role};

    fn msg(role: Role, text: &str) -> ChatMsg {
        ChatMsg {
            role,
            text: text.into(),
        }
    }

    /// Todas las líneas de un mensaje arrancan en la misma columna: es lo que
    /// mantiene el párrafo unido ahora que no hay ni etiqueta ni barra.
    #[test]
    fn el_texto_arranca_siempre_en_la_misma_columna() {
        for rol in [Role::System, Role::Agent] {
            let chat = vec![msg(
                rol,
                "una línea bastante larga que no cabe entera y se parte en varias",
            )];
            let lineas: Vec<String> = previa(&chat, 40)
                .into_iter()
                .filter(|l| !l.trim().is_empty())
                .collect();
            assert!(lineas.len() > 1, "hacen falta varias líneas");
            // Se mira el ancho de la marca, no los espacios: la primera
            // línea del agente empieza por «·· », que ocupa lo mismo pero no
            // es blanco.
            for linea in componer(&chat[0], 40) {
                let Some(marca) = linea.spans.first() else {
                    continue;
                };
                assert_eq!(
                    marca.content.chars().count(),
                    3,
                    "la marca cambia de ancho: {:?}",
                    marca.content
                );
            }
        }
    }

    /// Y la etiqueta solo en la primera. Repetirla en cada línea es el ruido
    /// que se quería quitar.
    #[test]
    fn la_etiqueta_va_solo_al_principio() {
        let chat = vec![msg(
            Role::Agent,
            "una respuesta larga que no cabe y se parte en varias líneas seguidas",
        )];
        let lineas: Vec<String> = previa(&chat, 30)
            .into_iter()
            .filter(|l| !l.is_empty())
            .collect();
        assert!(lineas[0].contains("··"));
        for l in &lineas[1..] {
            assert!(!l.contains("··"), "la etiqueta se repite: {l:?}");
        }
    }

    /// Al humano no lo marca ninguna etiqueta ni ninguna barra, sino el fondo
    /// de su propio texto. Es el cambio que quita más ruido de la pantalla, y
    /// por eso conviene que un despiste no lo devuelva.
    #[test]
    fn lo_que_escribe_el_humano_no_lleva_etiqueta() {
        let chat = vec![msg(Role::User, "el push ha fallado")];
        let t = previa(&chat, 40).join("\n");
        assert!(!t.contains("tú"), "volvió la etiqueta: {t:?}");
        assert!(!t.contains("▏"), "volvió la barra: {t:?}");
        assert!(!t.contains("vs"), "volvió la etiqueta de sistema: {t:?}");
        assert!(t.contains("el push ha fallado"));

        // Lo que sí lleva es fondo, que es lo que lo identifica ahora.
        let lineas = componer(&chat[0], 40);
        let con_fondo = lineas
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.style.bg.is_some());
        assert!(con_fondo, "el texto del humano se quedó sin fondo");
    }

    #[test]
    fn cada_rol_se_distingue() {
        let chat = vec![
            msg(Role::User, "hola"),
            msg(Role::Agent, "qué tal"),
            msg(Role::System, "nota"),
        ];
        let t = previa(&chat, 40).join("\n");
        assert!(t.contains("··"), "el agente pierde su marca");
        // Ni el humano ni vsioncode llevan ninguna: uno se distingue por el
        // fondo y el otro por el gris. Es lo que deja la pantalla callada.
        assert!(!t.contains("tú"));
        assert!(!t.contains("vs"));
    }

    /// El trabajo en curso pesa menos que el resultado, y se ve.
    ///
    /// Un turno que trae comando es el agente diciendo qué va a mirar antes de
    /// seguir; el que no trae ninguno es donde ha terminado y cuenta lo
    /// averiguado. Atenuar el primero es lo que deja el resumen destacando sin
    /// añadir ni una marca más a la pantalla.
    #[test]
    fn lo_que_dice_mientras_trabaja_va_atenuado() {
        use ratatui::style::Modifier;
        // Se mira la palabra concreta: la marca del agente y la canaleta del
        // comando llevan estilo propio y no entran en esto.
        let atenuada = |m: &ChatMsg, palabra: &str| {
            componer(m, 60)
                .iter()
                .flat_map(|l| l.spans.clone())
                .find(|s| s.content.contains(palabra))
                .unwrap_or_else(|| panic!("no se pintó «{palabra}»"))
                .style
                .add_modifier
                .contains(Modifier::DIM)
        };
        let trabajando = msg(
            Role::Agent,
            "Miro las interfaces.\n\n```bash\nip -br a\n```",
        );
        let resumen = msg(Role::Agent, "La red va por wlp1s0.");
        assert!(
            atenuada(&trabajando, "Miro"),
            "el turno con comando no se atenúa"
        );
        assert!(
            !atenuada(&resumen, "red"),
            "el resumen no puede ir atenuado"
        );
    }

    /// Que el trabajo esté parado esperándote no puede leerse como un aviso
    /// más de vsioncode.
    ///
    /// Es lo contrario de lo que dice de sí mismo —que va en gris porque es lo
    /// menos importante—: aquí no puede seguir nadie hasta que pulses. Lleva su
    /// marca, que además es la tecla, y no va atenuado.
    #[test]
    fn la_espera_no_se_confunde_con_un_aviso() {
        use ratatui::style::Modifier;
        let espera = msg(Role::Espera, "Te toca a ti: Alt+↵ lo pone en tu línea.");
        let aviso = msg(Role::System, "Escrito src/app.rs: −3 +7 líneas.");

        let t = previa(std::slice::from_ref(&espera), 60).join("\n");
        assert!(t.contains('\u{21b5}'), "la espera pierde su marca: {t}");

        let atenuada = |m: &ChatMsg, palabra: &str| {
            componer(m, 60)
                .iter()
                .flat_map(|l| l.spans.clone())
                .find(|s| s.content.contains(palabra))
                .unwrap_or_else(|| panic!("no se pintó «{palabra}»"))
                .style
                .add_modifier
                .contains(Modifier::DIM)
        };
        assert!(!atenuada(&espera, "toca"), "la espera no puede ir apagada");
        assert!(atenuada(&aviso, "Escrito"), "lo demás sí va apagado");
    }

    /// Un cambio se pinta como un bloque: lo que se va y lo que entra con
    /// fondos distintos, y el contexto sin ninguno.
    ///
    /// Se comprueba el fondo y no el color de la letra porque es lo que
    /// distingue un diff de un párrafo cualquiera: la forma del bloque.
    #[test]
    fn un_cambio_se_ve_como_un_bloque() {
        let m = msg(
            Role::System,
            "Escrito x.rs: −1 +1 líneas.\n```diff\n 12 antes\n-13 viejo\n+13 nuevo\n```",
        );
        let fondo = |palabra: &str| {
            componer(&m, 60)
                .iter()
                .flat_map(|l| l.spans.clone())
                .find(|s| s.content.contains(palabra))
                .unwrap_or_else(|| panic!("no se pintó «{palabra}»"))
                .style
                .bg
        };
        let (fuera, dentro) = (fondo("viejo"), fondo("nuevo"));
        assert!(fuera.is_some(), "lo que se va no tiene fondo");
        assert!(dentro.is_some(), "lo que entra no tiene fondo");
        assert_ne!(fuera, dentro, "se van y entran con el mismo color");
        assert!(fondo("antes").is_none(), "el contexto no lleva fondo");
        // Las vallas no se pintan, aquí tampoco.
        let t = previa(std::slice::from_ref(&m), 60).join("\n");
        assert!(!t.contains("```"), "las vallas del diff se ven: {t}");
    }

    /// El bloque llega hasta el borde del panel. Si cada línea acabase donde
    /// acaba su código, el diff se vería dentado y dejaría de leerse de un
    /// vistazo.
    #[test]
    fn el_bloque_del_cambio_llega_al_borde() {
        let m = msg(Role::System, "```diff\n+13 x\n```");
        let ancho = 40;
        let fila = componer(&m, ancho)
            .into_iter()
            .find(|l| l.spans.iter().any(|s| s.style.bg.is_some()))
            .expect("no se pintó la línea");
        let largo: usize = fila.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(largo, ancho, "la banda no llega al borde");
    }

    /// Un turno largo se mide en minutos: `1490s` obliga a dividir para saber
    /// si son dos minutos o veinticinco.
    #[test]
    fn un_rato_largo_se_dice_en_minutos() {
        assert_eq!(duracion(0), "0s");
        assert_eq!(duracion(59), "59s");
        assert_eq!(duracion(60), "1m 0s");
        assert_eq!(duracion(79), "1m 19s");
        assert_eq!(duracion(1443), "24m 3s");
    }

    /// En la barra el recuento va en orden de magnitud: un `1.234.567` no
    /// cabe y tampoco dice nada más.
    #[test]
    fn los_tokens_de_la_barra_caben() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(9_999), "9999");
        assert_eq!(tokens(10_000), "10k");
        assert_eq!(tokens(999_999), "999k");
        assert_eq!(tokens(1_240_000), "1.2M");
        assert_eq!(tokens(12_000_000), "12.0M");
    }

    /// Un comando se pinta con su canaleta y sin las vallas del markdown, que
    /// eran ruido puro.
    #[test]
    fn el_comando_se_pinta_sin_las_vallas() {
        let chat = vec![msg(Role::Agent, "Prueba:\n\n```bash\nls -la\n```")];
        let t = previa(&chat, 60).join("\n");
        assert!(t.contains("▌ ls -la"), "{t}");
        assert!(!t.contains("```"), "las vallas no se pintan: {t}");
    }

    /// Un ancho ridículo no puede hacer que se pierda el texto ni que reviente
    /// la resta del prefijo.
    #[test]
    fn aguanta_un_panel_estrechisimo() {
        let chat = vec![msg(Role::Agent, "hola qué tal")];
        let _ = previa(&chat, 8);
        let _ = previa(&chat, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El fallo que se veía: al pulsar la barra espaciadora el cursor no se
    /// movía, y al escribir la letra siguiente daba un salto.
    ///
    /// Pasaba porque los envolvedores de texto recortan el espacio del final
    /// de cada línea. Para pintar un párrafo está bien; para una caja donde se
    /// escribe, hace que escribir un espacio no tenga efecto visible.
    #[test]
    fn el_espacio_mueve_el_cursor() {
        let (_, sin) = envolver_entrada("hola", 20);
        let (_, con) = envolver_entrada("hola ", 20);
        assert_eq!(sin, (0, 4));
        assert_eq!(con, (0, 5), "el espacio tiene que contar");

        let (_, dos) = envolver_entrada("hola  ", 20);
        assert_eq!(dos, (0, 6), "y dos espacios, dos columnas");
    }

    /// La invariante que de verdad importa: **cada tecla mueve el cursor**.
    ///
    /// No se puede exigir que avance exactamente una columna: cuando una
    /// palabra a medias deja de caber, baja entera a la línea siguiente y el
    /// cursor se va con ella. Eso lo hace cualquier editor y está bien. Lo que
    /// no puede pasar nunca es que se quede quieto, que era el fallo.
    #[test]
    fn cada_tecla_mueve_el_cursor() {
        let texto = "busca herramientas de analisis de redes que hagan falta al ordenador";
        for ancho in [16usize, 24, 40] {
            let mut anterior = (0usize, 0usize);
            for i in 1..=texto.chars().count() {
                let trozo: String = texto.chars().take(i).collect();
                let (_, ahora) = envolver_entrada(&trozo, ancho);
                assert_ne!(
                    ahora, anterior,
                    "el cursor se quedó quieto con ancho {ancho} al escribir «{trozo}»"
                );
                anterior = ahora;
            }
        }
    }

    /// Y el caso concreto de la queja, escrito tal cual.
    #[test]
    fn escribir_varios_espacios_seguidos_mueve_el_cursor_cada_vez() {
        let mut anterior = envolver_entrada("hola", 20).1;
        for n in 1..=4 {
            let trozo = format!("hola{}", " ".repeat(n));
            let ahora = envolver_entrada(&trozo, 20).1;
            assert_ne!(ahora, anterior, "el espacio nº {n} no movió el cursor");
            anterior = ahora;
        }
    }

    #[test]
    fn parte_por_palabras_sin_cortarlas() {
        let (lineas, _) = envolver_entrada("uno dos tres cuatro cinco", 12);
        assert!(lineas.len() > 1);
        for l in &lineas {
            assert!(l.chars().count() <= 13, "línea demasiado larga: {l:?}");
        }
        let junto: String = lineas.join("");
        assert_eq!(junto.replace("  ", " ").trim(), "uno dos tres cuatro cinco");
    }

    #[test]
    fn una_palabra_larguisima_se_parte() {
        let (lineas, _) = envolver_entrada("supercalifragilisticoespialidoso", 10);
        assert!(lineas.len() >= 4);
        for l in &lineas {
            assert!(l.chars().count() <= 10, "no se partió: {l:?}");
        }
    }

    /// El cursor de caracteres coincide con lo que envolver el prefijo dice:
    /// es la propiedad que usa `posicion_cursor`, comprobada contra la
    /// posición «al final» que devuelve `envolver_entrada` para cada trozo.
    #[test]
    fn la_posicion_del_cursor_cuadra_con_el_wrap_del_prefijo() {
        let texto = "dime qué ves en hóla mundo y sigue";
        for ancho in [16usize, 24, 40] {
            for i in 0..=texto.chars().count() {
                assert_eq!(
                    posicion_cursor(texto, ancho, i),
                    envolver_entrada(&texto.chars().take(i).collect::<String>(), ancho).1,
                    "cursor {i} con ancho {ancho} no cuadra"
                );
            }
        }
    }

    /// ↑ conserva la columna y baja al principio si la línea es más corta;
    /// ↓ al final del texto se queda quieto en vez de salirse.
    #[test]
    fn el_movimiento_vertical_respeta_la_columna_y_no_se_sale() {
        let texto = "uno dos tres cuatro cinco seis";
        let ancho = 12usize;
        // Bajar desde la primera línea: la columna no puede pasarse del final
        // de la línea destino, y bajar más allá de la última no mueve.
        let abajo = cursor_vertical(texto, ancho, 3, 1);
        assert!(posicion_cursor(texto, ancho, abajo).0 == 1);
        let ultima_fila = posicion_cursor(texto, ancho, texto.chars().count()).0;
        let fondo = cursor_vertical(texto, ancho, texto.chars().count(), 1);
        assert_eq!(fondo, texto.chars().count());
        assert_eq!(posicion_cursor(texto, ancho, fondo).0, ultima_fila);
        // Y desde esa fila de abajo, subir vuelve a la línea anterior sin
        // perder de vista en qué columna estábamos.
        let arriba = cursor_vertical(texto, ancho, fondo, -1);
        assert!(posicion_cursor(texto, ancho, arriba).0 < ultima_fila);
        assert!(posicion_cursor(texto, ancho, arriba).1 <= 12);
        // Subir desde el principio tampoco se sale.
        assert_eq!(cursor_vertical(texto, ancho, 0, -1), 0);
    }

    #[test]
    fn el_cursor_baja_de_linea_al_llenarla() {
        let (lineas, (fila, col)) = envolver_entrada("abcdefghij", 10);
        assert_eq!((fila, col), (1, 0), "llenar la línea pasa a la siguiente");
        assert_eq!(lineas.len(), 2);
    }

    #[test]
    fn cuenta_bien_los_acentos() {
        // Contar bytes en vez de caracteres pondría el cursor donde no es.
        let (_, c) = envolver_entrada("año", 20);
        assert_eq!(c, (0, 3));
        let (_, c) = envolver_entrada("ñññ ", 20);
        assert_eq!(c, (0, 4));
    }

    #[test]
    fn con_la_caja_vacia_el_cursor_va_al_principio() {
        let (lineas, c) = envolver_entrada("", 20);
        assert_eq!(c, (0, 0));
        assert_eq!(lineas.len(), 1);
    }

    fn msg(t: &str) -> ChatMsg {
        ChatMsg {
            role: Role::Agent,
            text: t.into(),
        }
    }

    /// Lo que motivó el caché: solo se recompone lo que ha cambiado.
    ///
    /// Antes se reenvolvía toda la conversación en cada fotograma, hasta
    /// treinta veces por segundo, cuando lo único que crecía era el último
    /// mensaje. Con charla larga eso se nota como trompicones justo mientras
    /// llega la respuesta.
    #[test]
    fn el_cache_no_rehace_lo_que_no_cambio() {
        let mut c = CacheChat::default();
        let chat = vec![msg("primero"), msg("segundo")];
        let a = c.lineas(&chat, 40);

        // Sin tocar nada, el resultado es idéntico.
        assert_eq!(a.len(), c.lineas(&chat, 40).len());

        // Crece el último, como al recibir por trozos.
        let chat2 = vec![msg("primero"), msg("segundo y algo más largo")];
        let b = c.lineas(&chat2, 40);
        assert!(b.len() >= a.len());
        assert_eq!(c.entradas.len(), 2);
    }

    #[test]
    fn al_cambiar_el_ancho_se_recompone() {
        let mut c = CacheChat::default();
        let chat = vec![msg("una frase razonablemente larga para que envuelva")];
        let ancho = c.lineas(&chat, 80).len();
        let estrecho = c.lineas(&chat, 20).len();
        assert!(estrecho > ancho, "más estrecho debe dar más líneas");
    }

    /// `/limpiar` vacía la conversación: el caché no puede seguir enseñando
    /// mensajes que ya no existen.
    #[test]
    fn al_vaciar_la_conversacion_se_olvida_todo() {
        let mut c = CacheChat::default();
        c.lineas(&[msg("uno"), msg("dos")], 40);
        assert!(c.lineas(&[], 40).is_empty());
        assert!(c.entradas.is_empty());
    }

    #[test]
    fn un_mensaje_del_agente_vacio_no_ocupa_sitio() {
        let mut c = CacheChat::default();
        assert!(c.lineas(&[msg("")], 40).is_empty());
    }

    #[test]
    fn el_reparto_ancho_es_lado_a_lado() {
        let r = split(Rect::new(0, 0, 160, 40), Vista::Ambos, 0);
        let (sh, ag) = (r.shell.unwrap(), r.agente.unwrap());
        // Mismo alto: están uno al lado del otro.
        assert_eq!(sh.height, ag.height);
        assert!(sh.width > ag.width, "el shell se lleva la parte grande");
    }

    #[test]
    fn el_reparto_estrecho_es_apilado() {
        let r = split(Rect::new(0, 0, 90, 40), Vista::Ambos, 0);
        let (sh, ag) = (r.shell.unwrap(), r.agente.unwrap());
        assert_eq!(sh.width, ag.width, "apilados ocupan el mismo ancho");
        assert!(sh.height > ag.height, "el shell se lleva la parte grande");
    }

    /// El invariante que hace que esto se pueda enseñar: la lista sale del
    /// panel del agente y **nunca** del shell. El pseudoterminal solo se
    /// redimensiona al cambiar la ventana, así que quitarle filas aquí
    /// pintaría un terminal de un tamaño en un hueco de otro.
    #[test]
    fn la_lista_no_le_quita_filas_al_shell() {
        let sin = split(Rect::new(0, 0, 160, 40), Vista::Ambos, 0);
        let con = split(Rect::new(0, 0, 160, 40), Vista::Ambos, 3);
        assert_eq!(sin.shell, con.shell, "el shell se ha movido");
        assert_eq!(con.lista.unwrap().height, 3);
        assert_eq!(
            con.agente.unwrap().height + 3,
            sin.agente.unwrap().height,
            "las filas salen del panel del agente"
        );
    }

    /// Antes que dejar al agente sin conversación, no hay lista: un panel con
    /// borde, caja y nada en medio no es un panel.
    #[test]
    fn en_un_panel_pequeño_la_lista_no_sale() {
        let r = split(Rect::new(0, 0, 90, 20), Vista::Ambos, 4);
        assert!(r.lista.is_none(), "{:?}", r.lista);
        assert_eq!(
            r.agente,
            split(Rect::new(0, 0, 90, 20), Vista::Ambos, 0).agente
        );
    }

    #[test]
    fn sin_panel_el_shell_se_lo_queda_todo() {
        let r = split(Rect::new(0, 0, 160, 40), Vista::SoloShell, 0);
        assert!(r.agente.is_none());
        assert_eq!(r.shell.unwrap().height, 40 - STATUS_H);
        assert_eq!(r.status.height, STATUS_H);
    }

    #[test]
    fn el_pty_recibe_el_hueco_interior_no_la_ventana() {
        let (rows, cols) = shell_dims(160, 40, Vista::Ambos);
        // Siempre menor que la ventana: se han restado bordes y barra.
        assert!(cols < 160 && rows < 40);
        assert!(
            cols > 60 && rows > 30,
            "pero sin quedarse en nada: {cols}×{rows}"
        );
    }

    /// La vista de solo agente ocupa la pantalla entera.
    ///
    /// Estuvo centrada a 96 columnas y se cambió al verla: un recuadro en
    /// mitad de la pantalla con bandas negras a los lados parece un fallo de
    /// dibujado, no una decisión de tipografía.
    #[test]
    fn solo_agente_ocupa_todo_el_ancho() {
        let r = split(Rect::new(0, 0, 160, 40), Vista::SoloAgente, 0);
        assert!(r.shell.is_none(), "el shell no se pinta");
        let (ag, st) = (r.agente.unwrap(), r.status);
        assert_eq!(ag.width, 160, "sin márgenes a los lados");
        assert_eq!(ag.x, 0);
        assert_eq!(
            ag.height + st.height,
            40,
            "y sin dejar hueco arriba o abajo"
        );
    }

    #[test]
    fn las_tres_vistas_se_recorren_en_circulo() {
        let mut v = Vista::Ambos;
        for _ in 0..3 {
            v = v.siguiente();
        }
        assert_eq!(v, Vista::Ambos);
    }

    #[test]
    fn colapsar_el_panel_le_da_mas_sitio_al_shell() {
        let (_, con) = shell_dims(160, 40, Vista::Ambos);
        let (_, sin) = shell_dims(160, 40, Vista::SoloShell);
        assert!(sin > con);
    }

    #[test]
    fn las_rutas_largas_se_acortan_sin_perder_el_final() {
        assert_eq!(
            short_path("/usr/local/share/cosas/muy/largas/de/verdad"),
            "~/…/de/verdad"
        );
        assert_eq!(short_path("/tmp"), "/tmp");
    }

    /// `/modelo` en una terminal pequeña: la ventana tiene que caber donde
    /// se dibuja. El mínimo legible restaba más de lo que había y el centrado
    /// daba la vuelta en `u16`: la ventana se iba a la columna ~32763 y quien
    /// pulsaba `/modelo` no veía nada.
    #[test]
    fn la_ventana_de_modelos_cabe_en_el_area() {
        for (w, h) in [(80u16, 24u16), (20, 10), (10, 4), (6, 3)] {
            let area = Rect::new(0, 0, w, h);
            let z = zona_selector(area);
            assert!(
                z.x >= area.x && z.y >= area.y,
                "se sale por arriba o por la izquierda en {w}x{h}: {z:?}"
            );
            assert!(
                z.right() <= area.right() && z.bottom() <= area.bottom(),
                "se sale por abajo o por la derecha en {w}x{h}: {z:?}"
            );
        }
    }

    /// Un pegado de varias líneas se pintaba todo en una: el salto viajaba
    /// dentro de una «palabra» y no medía ninguna columna.
    #[test]
    fn un_salto_de_linea_parte_la_linea() {
        let (lineas, cursor) = envolver_entrada("uno\ndos", 20);
        assert_eq!(lineas.len(), 2, "dos líneas, no una: {lineas:?}");
        assert_eq!(lineas[0], "uno");
        assert_eq!(lineas[1], "dos");
        assert_eq!(cursor, (1, 3));
    }

    /// Y el cursor sigue cuadrando con lo que se pinta cuando hay saltos.
    #[test]
    fn el_cursor_cuadra_en_un_texto_con_saltos() {
        let texto = "uno\ndos tres";
        assert_eq!(posicion_cursor(texto, 20, 2), (0, 2));
        assert_eq!(posicion_cursor(texto, 20, 4), (1, 0), "justo tras el salto");
        assert_eq!(posicion_cursor(texto, 20, texto.chars().count()), (1, 8));
        // Y el salto de una línea que ya iba llena no se pierde por el camino.
        let (lineas, _) = envolver_entrada("abcdefghij\nx", 10);
        assert_eq!(lineas, vec!["abcdefghij", "x"]);
    }

    /// Un tabulador o el `\r` de un CRLF entran por la misma vía y tampoco
    /// miden nada: valen una columna en blanco para que el cursor cuadre.
    #[test]
    fn los_otros_caracteres_de_control_ocupan_una_columna() {
        let (lineas, cursor) = envolver_entrada("a\tb", 20);
        assert_eq!(lineas, vec!["a b"]);
        assert_eq!(cursor, (0, 3));
        let (lineas, _) = envolver_entrada("uno\r\ndos", 20);
        assert_eq!(lineas.len(), 2, "el CRLF corta una vez, no dos");
    }

    #[test]
    fn acortar_respeta_los_caracteres_no_ascii() {
        // Cortar por bytes en vez de por caracteres rompería la ñ por la mitad.
        let s = short("añañañañañaña", 5);
        assert_eq!(s.chars().count(), 5);
        assert!(s.ends_with('…'));
    }
}

#[cfg(test)]
mod tests_subagentes {
    use super::*;
    use ratatui::style::Modifier;
    use std::time::{Duration, Instant};

    fn texto(v: &[Span<'static>]) -> String {
        v.iter().map(|s| s.content.as_ref()).collect()
    }

    fn sub(t: &str, m: &str, secs: u64) -> crate::app::Subagente {
        crate::app::Subagente {
            titulo: t.into(),
            modelo: m.into(),
            desde: Instant::now() - Duration::from_secs(secs),
        }
    }

    /// Con dos o más, cada uno con su línea: el título suelto de la barra
    /// decía qué hace uno de ellos sin decir cuál, que es la pregunta.
    #[test]
    fn cada_uno_dice_en_que_anda() {
        let vivos = vec![
            sub("leer src/voz.rs", "haiku", 12),
            sub("leer src/pty.rs", "haiku", 3),
        ];
        let v = lineas_subagentes(&vivos, &[], 58, 4);
        assert_eq!(v.len(), 2);
        assert!(texto(&v[0].spans).contains("leer src/voz.rs"));
        assert!(texto(&v[1].spans).contains("leer src/pty.rs"));
        // Y cada uno con su reloj, que es lo que enseña cuál se eterniza.
        assert!(texto(&v[0].spans).contains("12s"));
        assert!(texto(&v[1].spans).contains("3s"));
    }

    /// Las columnas cuadran aunque el título llegue al borde: si el tiempo
    /// baila de fila en fila, la lista deja de poder leerse en vertical.
    #[test]
    fn el_tiempo_va_en_columna() {
        let vivos = vec![
            sub(
                "un título larguísimo que no cabe de ninguna manera aquí",
                "haiku",
                12,
            ),
            sub("corto", "haiku", 12),
        ];
        let v = lineas_subagentes(&vivos, &[], 58, 4);
        let anchos: Vec<usize> = v.iter().map(|l| texto(&l.spans).chars().count()).collect();
        assert_eq!(
            anchos[0], anchos[1],
            "las filas no miden lo mismo: {anchos:?}"
        );
        assert!(anchos[0] <= 58, "se sale del panel: {anchos:?}");
        // Y sin quedar pegado al título.
        assert!(
            !texto(&v[0].spans).contains("…haiku"),
            "{}",
            texto(&v[0].spans)
        );
    }

    /// Lo que espera turno se ve, y se ve que espera. Pedir tres y ver dos
    /// parece que uno se ha perdido: el mensaje que lo explicaba sube con la
    /// conversación y a los diez segundos ya no está.
    #[test]
    fn la_cola_se_ve_y_no_finge_que_trabaja() {
        let vivos = vec![sub("uno", "haiku", 5)];
        let v = lineas_subagentes(&vivos, &["dos"], 58, 4);
        assert!(texto(&v[1].spans).contains("en cola"));
        assert!(!texto(&v[1].spans).contains("⠋"), "no gira lo que no anda");
    }

    /// Enseñar cuatro de siete sin decir que hay siete es peor que enseñar
    /// tres y decirlo.
    #[test]
    fn lo_que_no_cabe_se_cuenta() {
        let vivos: Vec<_> = (0..5).map(|i| sub(&format!("t{i}"), "haiku", 2)).collect();
        let v = lineas_subagentes(&vivos, &[], 58, 3);
        assert_eq!(v.len(), 3);
        assert!(
            texto(&v[2].spans).contains("+3 más"),
            "{}",
            texto(&v[2].spans)
        );
    }

    /// «1 subagentes» estuvo en la barra desde el primer día. Un fallo de
    /// concordancia en lo único que se lee mil veces al día resta más
    /// credibilidad que lo que suma el dato.
    #[test]
    fn uno_es_un_subagente() {
        assert!(
            texto(&spans_subagentes(1, None, None)).contains("1 subagente "),
            "{}",
            texto(&spans_subagentes(1, None, None))
        );
        assert!(texto(&spans_subagentes(3, None, None)).contains("3 subagentes"));
    }

    /// El fondo de esta barra es el distintivo del modo. Cualquier otra chapa
    /// de color al lado se lee como un segundo modo puesto.
    #[test]
    fn no_lleva_chapa_de_fondo() {
        for s in spans_subagentes(2, Some(Instant::now()), Some("mirar src/voz.rs")) {
            assert!(s.style.bg.is_none(), "«{}» lleva fondo", s.content);
        }
    }

    /// El número es el estado; el título, el detalle. Si el detalle grita
    /// igual que el estado, hay que leer los dos para saber cuál es cuál.
    #[test]
    fn el_titulo_va_detras_y_apagado() {
        let v = spans_subagentes(1, None, Some("mirar qué hace src/voz.rs"));
        let ultimo = v.last().unwrap();
        assert!(ultimo.content.contains("mirar qué hace"), "{ultimo:?}");
        assert!(
            ultimo.style.add_modifier.contains(Modifier::DIM),
            "{ultimo:?}"
        );
    }

    /// Cuánto llevan, que mientras trabajan es el único número que se mueve
    /// en la pantalla. Y no antes del segundo: un «0s» en cada reparto es un
    /// número que nunca dice nada, y entonces tampoco se lee el que sí.
    #[test]
    fn el_tiempo_sale_cuando_hay_algo_que_contar() {
        let ya = Instant::now() - Duration::from_secs(75);
        assert!(texto(&spans_subagentes(1, Some(ya), None)).contains("1m 15s"));
        assert!(!texto(&spans_subagentes(1, Some(Instant::now()), None)).contains("0s"));
    }
}

#[cfg(test)]
mod tests_plan {
    use super::*;
    use crate::plan::{Fase, Plan};

    fn texto(v: &[Span<'static>]) -> String {
        v.iter().map(|s| s.content.as_ref()).collect()
    }

    fn plan(n: usize) -> Plan {
        Plan {
            objetivo: "arreglar la barra".into(),
            pasos: (1..=n).map(|i| format!("paso {i}")).collect(),
        }
    }

    /// Sin plan la barra no dice nada del plan. Un hueco reservado «por si
    /// acaso» es sitio quitado a lo que sí está pasando.
    #[test]
    fn sin_plan_no_ocupa_barra() {
        assert!(spans_plan(None).is_empty());
    }

    /// Las tres fases se distinguen de un vistazo, que es lo único que se le
    /// pide a la barra.
    #[test]
    fn cada_fase_dice_lo_suyo() {
        assert!(texto(&spans_plan(Some(&Fase::Trazando))).contains("trazando"));

        let propuesto = texto(&spans_plan(Some(&Fase::Propuesto(plan(7)))));
        assert!(propuesto.contains("7 pasos"), "{propuesto}");
        assert!(propuesto.contains("Alt+"), "falta la tecla: {propuesto}");

        let marcha = texto(&spans_plan(Some(&Fase::Marcha {
            plan: plan(7),
            paso: 3,
        })));
        assert_eq!(marcha.trim(), "plan 3/7");
    }

    /// «plan 0/7» diría que va por el paso cero, y por el paso cero no va
    /// nadie: es que todavía no ha marcado ninguno. Lo que se sabe es cuántos
    /// pasos hay.
    #[test]
    fn sin_paso_marcado_no_se_inventa_el_cero() {
        let t = texto(&spans_plan(Some(&Fase::Marcha {
            plan: plan(7),
            paso: 0,
        })));
        assert!(!t.contains("0/"), "{t}");
        assert!(t.contains("7 pasos"), "{t}");
    }

    /// «1 pasos» en lo que se lee mil veces al día resta más credibilidad que
    /// lo que suma el dato. Es el mismo fallo que tuvo «1 subagentes».
    #[test]
    fn uno_es_un_paso() {
        let t = texto(&spans_plan(Some(&Fase::Propuesto(plan(1)))));
        assert!(t.contains("1 paso ·"), "{t}");
    }

    /// Un plan esperando y uno en marcha no pueden pintarse igual: uno pide
    /// una tecla y el otro solo informa.
    #[test]
    fn esperar_y_andar_no_se_pintan_igual() {
        let esperando = spans_plan(Some(&Fase::Propuesto(plan(4))))[0].style;
        let andando = spans_plan(Some(&Fase::Marcha {
            plan: plan(4),
            paso: 2,
        }))[0]
            .style;
        assert_ne!(esperando.fg, andando.fg);
    }
}

#[cfg(test)]
mod tests_barra {
    use super::*;
    use crate::app::Delegar;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn barra(tarea: &str, lista: Option<usize>) -> Delegar {
        Delegar {
            tarea: tarea.into(),
            cursor: tarea.chars().count(),
            agente: "qwen3.6-27b".into(),
            agentes: [
                ("haiku", "claude", "claude · el más rápido"),
                ("sonnet", "claude", "claude · equilibrado"),
                ("qwen3.6-27b", "groq", "groq · el más rápido de todos"),
            ]
            .into_iter()
            .map(|(n, de, nota)| crate::agente::Posible {
                nombre: n.into(),
                proveedor: de.into(),
                detalle: nota.into(),
                motor: crate::agente::Motor::Daemon(n.into()),
            })
            .collect(),
            lista,
        }
    }

    /// Lo que se ve, visto. `draw_delegar` y `draw_agentes` no necesitan una
    /// `App` —solo la barra y un rectángulo—, así que aquí sí se puede pintar
    /// en un búfer y leer el resultado, que es lo más cerca que se llega de
    /// mirar la pantalla sin abrir un terminal.
    fn pintado(d: &Delegar, alto: u16, con_lista: bool) -> String {
        let mut t = Terminal::new(TestBackend::new(60, alto)).unwrap();
        t.draw(|f| {
            let area = f.area();
            if con_lista {
                let [arriba, abajo] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(4)]).areas(area);
                draw_agentes(f, d, d.lista.unwrap_or(0), arriba);
                draw_delegar(f, d, abajo, true);
            } else {
                draw_delegar(f, d, area, true);
            }
        })
        .unwrap();
        let b = t.backend().buffer().clone();
        (0..b.area.height)
            .map(|y| {
                (0..b.area.width)
                    .map(|x| b[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Con quién va se ve **siempre**, y sin buscarlo: es el dato que cambia
    /// lo que pasa al pulsar Enter.
    #[test]
    fn la_barra_dice_con_quien_va_y_como_se_usa() {
        let v = pintado(&barra("", None), 4, false);
        assert!(v.contains("delegar"), "{v}");
        assert!(v.contains("qwen3.6-27b"), "{v}");
        assert!(
            v.contains("(groq)"),
            "de quién es, que es media respuesta: {v}"
        );
        assert!(v.contains("↵ enviar"), "{v}");
        // Con la caja vacía, la pista de qué se espera aquí.
        assert!(v.contains("qué le encargas"), "{v}");
    }

    #[test]
    fn la_barra_enseña_lo_escrito() {
        let v = pintado(&barra("mira qué hace src/voz.rs", None), 4, false);
        assert!(v.contains("» mira qué hace src/voz.rs"), "{v}");
        assert!(!v.contains("qué le encargas"), "la pista se va: {v}");
    }

    /// La lista se despliega encima, con el señalado marcado y su nota detrás.
    #[test]
    fn la_lista_de_agentes_se_lee_como_el_menu() {
        let v = pintado(&barra("", Some(2)), 10, true);
        assert!(v.contains("haiku"), "{v}");
        assert!(v.contains("groq · el más rápido de todos"), "{v}");
        assert!(v.contains("↑↓ elegir"), "{v}");
    }
}
