//! La entrada: teclado, ratón y lo que se pega.
//!
//! Es el módulo que más se toca a diario, y por eso sale el último: mientras
//! los demás cortes estaban en marcha, este habría chocado con todos.
//!
//! Lo que llega por aquí no se interpreta dos veces. `keymap` decide qué
//! significa cada tecla —una sola tabla, y con el foco como parte de la
//! pregunta— y aquí solo se obedece: si una tecla no es nuestra, baja al
//! shell tal cual. Es la mitad visible del invariante de que escribir un
//! comando no lo ejecuta.
//!
//! Submódulo de `app` para seguir viendo lo privado de [`App`] sin abrir sus
//! campos al resto del programa.

use color_eyre::eyre::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{App, Focus, Mode, Role, Vista, byte_de};
use crate::keymap::{Action, resolve};
use crate::pty::{encode_key, encode_paste};
use crate::raton::{self, Accion, Zona};
use crate::ui;

/// Cuántas imágenes se adjuntan de una carpeta sin que nadie las nombre.
///
/// Cada una cuesta dinero y sitio en la ventana. Cuatro alcanzan para enseñar
/// una secuencia —lo típico: unas cuantas capturas seguidas— y no tantas como
/// para que soltar una carpeta grande salga caro sin querer.
const MAX_DE_CARPETA: usize = 4;

/// Expande `~` y quita las comillas de una ruta escrita a mano.
///
/// Escrita a mano, no arrastrada: por ahí llega `~/Imágenes` y llegan comillas
/// puestas de más. El pegado tiene su propio camino en `adjunto::rutas`, que
/// resuelve otras cosas —URIs, escapes— y no esta.
fn expandir(s: &str) -> std::path::PathBuf {
    let s = s.trim().trim_matches(['"', '\'']);
    if let Some(h) = dirs::home_dir() {
        if s == "~" {
            return h;
        }
        if let Some(resto) = s.strip_prefix("~/") {
            return h.join(resto);
        }
    }
    std::path::PathBuf::from(s)
}

/// Las imágenes de una carpeta, de la más reciente a la más vieja.
///
/// Por fecha y no por nombre porque el caso real es una carpeta de capturas:
/// lo que se quiere enseñar es lo último que ocurrió, y el nombre de fichero
/// que pone el escritorio no siempre ordena como parece.
fn imagenes_de(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<(std::time::SystemTime, std::path::PathBuf)> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| matches!(crate::adjunto::clasificar(p), crate::adjunto::Clase::Imagen))
        .filter_map(|p| {
            let t = p.metadata().ok()?.modified().ok()?;
            Some((t, p))
        })
        .collect();
    v.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    v.into_iter().map(|(_, p)| p).collect()
}

impl App {
    pub(super) fn on_terminal_event(&mut self, ev: Event) -> Result<()> {
        match ev {
            Event::Key(k) if k.kind == KeyEventKind::Press => self.on_key(k)?,
            Event::Paste(text) => {
                if self.focus == Focus::Shell {
                    let br = self
                        .shell
                        .parser
                        .read()
                        .map(|p| p.screen().bracketed_paste())
                        .unwrap_or(false);
                    self.shell.write_bytes(&encode_paste(&text, br))?;
                } else if let Some(d) = self.delegar.as_mut() {
                    // Con la barra abierta, lo pegado es parte del encargo. Sin
                    // esto se iría a la caja de detrás y aparecería al cerrarla,
                    // que es la peor forma de perder un texto: reaparece donde
                    // no lo esperas.
                    let byte = super::byte_de(&d.tarea, d.cursor);
                    d.tarea.insert_str(byte, &text);
                    d.cursor += text.chars().count();
                    self.shell.mark_dirty();
                } else {
                    self.pegar_en_agente(&text);
                }
            }
            Event::Mouse(m) => self.on_mouse(m)?,
            Event::Resize(cols, rows) => {
                self.ancho = cols;
                self.alto = rows;
                let (r, c) = ui::shell_dims(cols, rows, self.vista);
                self.shell.resize(r, c)?;
            }
            _ => {}
        }
        self.shell.mark_dirty();
        Ok(())
    }
    /// Trae lo que haya en el portapapeles: una imagen, o texto.
    ///
    /// Hace falta porque una terminal **no transmite imágenes**: por el pegado
    /// solo viaja texto, así que Ctrl+V con una captura copiada no llegaría
    /// nunca aquí como imagen por mucho que se escuche el evento de pegado.
    fn pegar_del_portapapeles(&mut self) {
        use crate::adjunto::portapapeles::{Contenido, como_instalarlo, leer};
        match leer() {
            Contenido::Imagen(ruta) => match crate::adjunto::leer(&ruta) {
                Ok(a) => {
                    let aviso = if self.ve_imagenes() {
                        format!("imagen pegada ({})", a.tamano())
                    } else {
                        format!(
                            "imagen pegada ({}) — ojo, «{}» no mira imágenes; cambia con \
                             /modelo o dime qué hay en ella.",
                            a.tamano(),
                            self.model
                        )
                    };
                    self.push(Role::System, aviso);
                    self.adjuntos.push(a);
                }
                Err(e) => self.push(Role::System, format!("No pude leer lo pegado: {e}")),
            },
            Contenido::Texto(t) => self.pegar_en_agente(&t),
            Contenido::Nada => self.notify("El portapapeles está vacío."),
            Contenido::SinHerramienta => self.push(Role::System, como_instalarlo().into()),
        }
        self.shell.mark_dirty();
    }
    /// Un pegado en el panel del agente: o son ficheros, o es texto.
    ///
    /// Arrastrar un fichero a una terminal no pega el fichero, pega su ruta.
    /// Reconocerla es lo que convierte ese gesto en lo que la persona quería
    /// decir al soltarlo.
    fn pegar_en_agente(&mut self, text: &str) {
        let rutas = crate::adjunto::rutas(text);
        if rutas.is_empty() {
            self.input.push_str(text.trim());
            self.cursor = self.input.chars().count();
            return;
        }

        for r in rutas {
            self.adjuntar_ruta(&r);
        }
        self.shell.mark_dirty();
    }

    /// Lee lo que haya en `r` y lo deja en la bandeja de adjuntos.
    ///
    /// Lo comparten el pegado y `/adjuntar`: son el mismo gesto por dos puertas
    /// distintas —una arrastra el fichero, la otra escribe su ruta— y el aviso
    /// tiene que ser el mismo por las dos. Sobre todo el de «este modelo no
    /// mira imágenes», que es el que evita mandar una captura a un modelo que
    /// solo va a ver que existe.
    pub(super) fn adjuntar_ruta(&mut self, r: &std::path::Path) {
        match crate::adjunto::leer(r) {
            Ok(a) => {
                let aviso = match a.clase {
                    crate::adjunto::Clase::Imagen if !self.ve_imagenes() => format!(
                        "adjunto: {} ({}) — ojo, «{}» no mira imágenes; elige otro \
                         modelo con /modelo o dime qué hay en ella.",
                        a.nombre(),
                        a.tamano(),
                        self.model
                    ),
                    crate::adjunto::Clase::Opaco => format!(
                        "adjunto: {} ({}) — no es texto, así que solo le paso el \
                         nombre.",
                        a.nombre(),
                        a.tamano()
                    ),
                    _ => format!("adjunto: {} ({})", a.nombre(), a.tamano()),
                };
                self.push(Role::System, aviso);
                self.adjuntos.push(a);
            }
            Err(e) => self.push(Role::System, format!("No pude leer {}: {e}", r.display())),
        }
    }

    /// `/adjuntar <ruta>`: un fichero, una imagen o una carpeta de imágenes.
    ///
    /// Hasta ahora la única forma de enseñarle algo al agente era pegarlo o
    /// arrastrarlo, y las dos dependen de un escritorio gráfico y del
    /// portapapeles. Escribir la ruta funciona siempre —también por SSH— y es
    /// la única vía razonable cuando lo que quieres enseñarle son las cinco
    /// capturas de una carpeta.
    pub(super) fn adjuntar_desde(&mut self, ruta: &str) {
        let p = expandir(ruta);

        if p.is_dir() {
            let mut imgs = imagenes_de(&p);
            if imgs.is_empty() {
                self.push(
                    Role::System,
                    format!("En {} no hay ninguna imagen.", p.display()),
                );
                return;
            }
            let total = imgs.len();
            imgs.truncate(MAX_DE_CARPETA);
            if total > imgs.len() {
                // Se dice el recorte en vez de adjuntar veinte imágenes en
                // silencio: cada una cuesta dinero y ventana de contexto.
                self.push(
                    Role::System,
                    format!(
                        "Hay {total} imágenes; adjunto las {} más recientes. Para otra, \
                         nómbrala: /adjuntar <ruta del fichero>.",
                        imgs.len()
                    ),
                );
            }
            for i in &imgs {
                self.adjuntar_ruta(i);
            }
            self.shell.mark_dirty();
            return;
        }

        if !p.exists() {
            self.push(Role::System, format!("No existe: {}", p.display()));
            return;
        }
        self.adjuntar_ruta(&p);
        self.shell.mark_dirty();
    }
    fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Result<()> {
        // Con la configuración inicial delante, el ratón es suyo: sirve para
        // señalar un servicio y para abrir su página, que es lo que cualquiera
        // intenta hacer al ver una dirección escrita.
        if self.bienvenida.is_some() {
            self.bienvenida_mouse(m);
            return Ok(());
        }

        let (w, h) = (self.ancho, self.alto);
        let z = ui::zonas(w, h, self.vista, ui::filas_subagentes(self));

        // Si el programa de dentro pidió el ratón, es suyo.
        let suyo = self
            .shell
            .parser
            .read()
            .map(|p| p.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None)
            .unwrap_or(false);

        match raton::resolver(m, z.shell, z.agente, z.caja, suyo) {
            Accion::Foco(Zona::Shell) => self.focus = Focus::Shell,
            Accion::Foco(Zona::Agente) | Accion::Foco(Zona::Caja) => {
                self.vista = Vista::Ambos;
                self.focus = Focus::Agent;
            }
            Accion::Foco(Zona::Barra) => {}
            Accion::DesplazarShell(n) => self.desplazar_shell(n),
            Accion::DesplazarChat(n) => {
                self.chat_scroll = (self.chat_scroll as i32 + n).max(0) as u16;
            }
            Accion::AlPrograma(bytes) => self.shell.write_bytes(&bytes)?,
            Accion::Nada => return Ok(()),
        }
        self.shell.mark_dirty();
        Ok(())
    }
    /// Mueve el scrollback del emulador. Positivo va hacia lo viejo.
    fn desplazar_shell(&mut self, n: i32) {
        if let Ok(mut p) = self.shell.parser.write() {
            let actual = p.screen().scrollback() as i32;
            let nuevo = (actual + n).max(0) as usize;
            p.screen_mut().set_scrollback(nuevo);
        }
    }
    fn on_key(&mut self, k: KeyEvent) -> Result<()> {
        // La puesta en marcha se queda con todas las teclas, incluidos los
        // atajos: mientras no haya a quién preguntar, «explicar lo último» o
        // «modo libre» no significan nada, y una tecla que no hace lo que dice
        // es peor que una tecla que no está.
        if self.bienvenida.is_some() {
            self.bienvenida_key(k);
            return Ok(());
        }

        // La barra de delegar es la capa de más arriba mientras está abierta:
        // lo que se escribe ahí es un encargo para otro, y si esas teclas
        // cayeran a la caja normal acabarían mandadas al agente principal.
        if self.delegar.is_some() && self.delegar_key(k) {
            return Ok(());
        }

        // Con la ventana de modelos delante, las teclas son suyas y de nadie
        // más: va antes incluso que los atajos, porque mientras eliges no
        // tiene sentido que Esc signifique otra cosa.
        if self.selector.is_some() && self.selector_key(k) {
            return Ok(());
        }

        // Esc cierra lo que haya abierto por encima de la conversación.
        //
        // Va antes que los atajos por el mismo motivo que el selector: en la
        // caja del agente Esc es «abortar el turno», y con la ayuda tapando
        // la pantalla eso no es lo que nadie espera. Se cierra lo de arriba
        // del todo y solo eso, para que dos capas abiertas se deshagan en el
        // orden en que se pusieron.
        if k.code == KeyCode::Esc {
            if self.show_help {
                self.show_help = false;
                self.shell.mark_dirty();
                return Ok(());
            }
            if self.focus == Focus::Agent && !self.menu().is_empty() {
                self.menu_oculto = true;
                self.menu_sel = 0;
                self.shell.mark_dirty();
                return Ok(());
            }
        }

        // Las teclas de vsioncode se miran primero, pero solo las suyas: todo
        // lo demás cae al shell sin tocar. Es la única forma de que readline,
        // vim y emacs sigan funcionando con sus atajos de siempre.
        if let Some(action) = resolve(k, self.focus) {
            return self.act(action);
        }

        match self.focus {
            Focus::Shell => {
                let app_cursor = self
                    .shell
                    .parser
                    .read()
                    .map(|p| p.screen().application_cursor())
                    .unwrap_or(false);
                if let Some(bytes) = encode_key(k, app_cursor) {
                    // Escribir devuelve la vista al final, como en cualquier
                    // terminal: si estabas mirando atrás y teclas, quieres ver
                    // lo que escribes.
                    self.desplazar_shell(i32::MIN / 2);
                    self.shell.write_bytes(&bytes)?;
                }
            }
            Focus::Agent => {
                // Con el menú abierto, las flechas y el Tab son suyos. Sin él,
                // esas mismas teclas no hacen nada especial en la caja.
                if !self.menu().is_empty() && self.menu_key(k) {
                    return Ok(());
                }
                // ← con la caja vacía abre la barra de delegar. Solo vacía: si
                // hay texto escrito, la flecha es del cursor, y robársela para
                // un atajo rompería lo único que una caja de texto promete.
                if k.code == KeyCode::Left && k.modifiers.is_empty() && self.input.is_empty() {
                    self.abrir_delegar();
                    return Ok(());
                }
                self.edit_input(k);
                // Al cambiar el texto cambian las opciones; volver al principio
                // evita quedarse señalando una fila que ya no existe.
                self.menu_sel = 0;
                self.menu_oculto = false;
            }
        }
        Ok(())
    }
    /// Offset en bytes del cursor dentro de `input`.
    ///
    /// El cursor cuenta caracteres (lo que ve el usuario al moverse con las
    /// flechas), pero `insert`/`remove` y el troceado de `str` operan en
    /// bytes: con acentos y otros caracteres multibyte, usar el índice tal
    /// cual cae en mitad de un carácter y pánico.
    pub fn byte_del_cursor(&self) -> usize {
        byte_de(&self.input, self.cursor)
    }
    fn edit_input(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                let byte = self.byte_del_cursor();
                self.input.insert(byte, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    let byte = self.byte_del_cursor();
                    self.input.remove(byte);
                }
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.input.chars().count());
            }
            KeyCode::Home => {
                self.cursor = 0;
            }
            KeyCode::End => {
                self.cursor = self.input.chars().count();
            }
            KeyCode::Up => {
                let ancho = self.ancho_entrada.get();
                if ancho > 0 {
                    self.cursor = crate::ui::cursor_vertical(&self.input, ancho, self.cursor, -1);
                }
            }
            KeyCode::Down => {
                let ancho = self.ancho_entrada.get();
                if ancho > 0 {
                    self.cursor = crate::ui::cursor_vertical(&self.input, ancho, self.cursor, 1);
                }
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('w') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                let cut = corte_de_palabra(&self.input);
                self.input.truncate(cut);
                self.cursor = self.cursor.min(self.input.chars().count());
            }
            _ => {}
        }
        self.cursor = self.cursor.min(self.input.chars().count());
    }
    fn act(&mut self, a: Action) -> Result<()> {
        match a {
            Action::Quit => self.quit = true,
            Action::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Shell => {
                        if self.vista == Vista::SoloShell {
                            self.vista = Vista::Ambos;
                        }
                        Focus::Agent
                    }
                    Focus::Agent => {
                        if self.vista == Vista::SoloAgente {
                            self.vista = Vista::Ambos;
                        }
                        Focus::Shell
                    }
                };
                self.refit();
            }
            Action::ToggleAgentPane => {
                self.vista = self.vista.siguiente();
                // El foco sigue a lo que se ve: no tiene sentido estar
                // escribiéndole al agente con el agente fuera de pantalla.
                self.focus = match self.vista {
                    Vista::SoloShell => Focus::Shell,
                    Vista::SoloAgente => Focus::Agent,
                    Vista::Ambos => self.focus,
                };
                self.notify(&format!("Vista: {}.", self.vista.label()));
                self.refit();
            }
            Action::ExplainLast => self.explain_last(),
            Action::ToggleCapture => {
                self.capture = !self.capture;
                self.notify(if self.capture {
                    "Captura reanudada: vuelvo a ver lo que ejecutas."
                } else {
                    "Captura EN PAUSA: no registro nada hasta que la reanudes."
                });
            }
            Action::AcceptProposal => self.accept_proposal()?,
            Action::DismissProposal => {
                // El plan primero y solo el plan: descartar de una tecla un
                // plan y además el comando que hubiera esperando sería tirar
                // dos decisiones distintas con una pulsación.
                if self.descartar_plan() {
                    return Ok(());
                }
                if self.edicion.take().is_some() {
                    self.notify("Cambio descartado. El fichero no se ha tocado.");
                }
                self.proposal = None;
            }
            Action::Submit => {
                // Con el menú abierto, Enter elige la fila señalada en vez de
                // enviar lo que hay escrito. Si no, no habría forma de usar la
                // lista: el keymap ve la tecla antes que la caja de escritura.
                let opciones = self.menu();
                if opciones.is_empty() {
                    self.submit_input();
                } else {
                    self.aceptar_menu(&opciones, true);
                }
            }
            Action::Abortar => {
                // Una sola tecla para «para lo que estés haciendo»: corta la
                // ejecución si la hay, descarta la propuesta si la hay, y si
                // no hay nada que parar, devuelve el foco al shell.
                //
                // Interrumpir un turno va lo primero porque es de lo que no se
                // podía salir: si el cerebro se queda esperando una respuesta
                // que no va a llegar, antes solo cabía cerrar vsioncode. Pasó
                // de verdad, y de la peor manera posible: el propio agente
                // propuso reiniciar la interfaz de red, el comando se ejecutó,
                // y al caer la red se llevó por delante la conexión del daemon
                // con la API. Se cortó su propia línea y se quedó «pensando»
                // para siempre.
                if self.thinking {
                    self.interrumpir_turno();
                } else if self.auto.activo() || self.auto.pasos > 0 {
                    self.abortar_auto();
                } else if self.descartar_plan() {
                    // Lo dice él, en la conversación: un plan descartado no
                    // cabe en un aviso de barra que se va a los cuatro
                    // segundos.
                } else if self.edicion.take().is_some() {
                    self.notify("Cambio descartado. El fichero no se ha tocado.");
                } else if self.proposal.take().is_some() {
                    self.notify("Propuesta descartada.");
                } else {
                    self.focus = Focus::Shell;
                }
            }
            Action::Pegar => self.pegar_del_portapapeles(),
            Action::CycleMode => {
                let m = self.mode.next();
                self.cambiar_modo(m);
                self.notify(&match m {
                    Mode::Observar => "Modo observar: no llamo al modelo. Coste cero.".to_string(),
                    Mode::Sugerir => {
                        "Modo sugerir: cuando algo falle te lo ofrezco en la barra.".to_string()
                    }
                    Mode::Conversar => "Modo conversar: pregúntame lo que quieras.".to_string(),
                    Mode::Ejecutar => {
                        "Modo ejecutar: trabajo yo en la terminal de al lado. Esc me para."
                            .to_string()
                    }
                });
            }
            Action::Voz => self.alternar_voz(),
            Action::Libre => self.alternar_libre(),
            Action::Pantalla => self.mirar_pantalla(),
            Action::Help => {
                self.show_help = !self.show_help;
                self.shell.mark_dirty();
            }
            Action::ScrollChatUp => self.chat_scroll = self.chat_scroll.saturating_add(3),
            Action::ScrollChatDown => self.chat_scroll = self.chat_scroll.saturating_sub(3),
        }
        Ok(())
    }
}

/// Dónde empieza la última palabra: lo que se lleva `Ctrl+W`.
///
/// El corte va **tras** el espacio que la separa, y un espacio puede ocupar
/// más de un byte. `rfind(char::is_whitespace).map(|i| i + 1)` daba un índice
/// en mitad del carácter y `truncate` reventaba el programa entero — y no era
/// un caso rebuscado: U+00A0 llega solo con pegar de una página web o de un
/// PDF. Peor aún, el pánico se llevaba por delante el terminal, porque el
/// gancho no devolvía el ratón.
fn corte_de_palabra(texto: &str) -> usize {
    texto
        .trim_end()
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{corte_de_palabra, expandir, imagenes_de};

    /// Un espacio no siempre ocupa un byte, y el corte cae justo detrás.
    #[test]
    fn ctrl_w_no_parte_un_caracter_por_la_mitad() {
        // Los que reventaban: espacio duro, fino y ideográfico.
        assert_eq!(corte_de_palabra("a\u{a0}b"), 3);
        assert_eq!(corte_de_palabra("a\u{2009}b"), 4);
        assert_eq!(corte_de_palabra("a\u{3000}b"), 4);

        // Y lo de siempre, que tiene que seguir igual.
        assert_eq!(corte_de_palabra("hola mundo"), 5);
        assert_eq!(corte_de_palabra("una sola"), 4);
        assert_eq!(corte_de_palabra("sola"), 0);
        assert_eq!(corte_de_palabra(""), 0);

        // Los espacios de la derecha no cuentan: se borra la palabra, no el
        // hueco que dejaste detrás.
        assert_eq!(corte_de_palabra("hola mundo   "), 5);

        // Sea cual sea la entrada, el corte tiene que ser una frontera válida.
        for t in ["a\u{a0}b", "ñ\u{2009}ñ", "  \u{3000}  ", "日本 語"] {
            assert!(t.is_char_boundary(corte_de_palabra(t)), "«{t}» corta mal");
        }
    }

    /// Una ruta escrita a mano trae `~` y a veces comillas.
    ///
    /// Es la diferencia entre que `/adjuntar ~/Imágenes/x.png` funcione o diga
    /// que no existe un fichero que sí está.
    #[test]
    fn la_ruta_escrita_a_mano_se_entiende() {
        let casa = dirs::home_dir().expect("hay HOME");
        assert_eq!(expandir("~"), casa);
        assert_eq!(expandir("~/Imágenes/x.png"), casa.join("Imágenes/x.png"));
        assert_eq!(
            expandir("  \"/tmp/una captura.png\"  "),
            std::path::PathBuf::from("/tmp/una captura.png"),
            "espacios y comillas de sobra no cuentan"
        );
        // Una tilde en medio no es la casa de nadie.
        assert_eq!(
            expandir("/tmp/a~b.png"),
            std::path::PathBuf::from("/tmp/a~b.png")
        );
    }

    /// De una carpeta salen solo las imágenes, y la más reciente primero.
    #[test]
    fn de_una_carpeta_salen_las_imagenes_por_fecha() {
        let dir = std::env::temp_dir().join(format!("vsion-imgs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crear el temporal");

        for n in ["vieja.png", "notas.txt", "nueva.jpg"] {
            std::fs::write(dir.join(n), b"x").expect("escribir");
            // El sistema de ficheros no siempre distingue dos escrituras
            // seguidas: sin esta pausa el orden sería cosa del azar.
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let v = imagenes_de(&dir);
        assert_eq!(v.len(), 2, "el .txt no es una imagen: {v:?}");
        assert!(v[0].ends_with("nueva.jpg"), "la más reciente va primera");
        assert!(v[1].ends_with("vieja.png"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
