//! La puesta en marcha: el modo modal que pide claves y escribe el TOML.
//!
//! Submódulo de `app` para seguir viendo lo privado de [`App`] sin abrir sus
//! campos al resto del programa.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Role};
use crate::ui;

impl App {
    /// Las teclas mientras está abierta la puesta en marcha.
    pub(super) fn bienvenida_key(&mut self, k: KeyEvent) {
        use crate::bienvenida::Paso;

        let Some(b) = self.bienvenida.as_mut() else {
            return;
        };
        self.shell.mark_dirty();

        // Ctrl+V pega la clave del portapapeles del escritorio. Es lo que hace
        // todo el mundo con una clave de API —nadie la teclea— y sin esto la
        // primera pantalla obligaría justo a lo que no se hace nunca.
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('v') {
            if matches!(b.paso, Paso::Clave { .. }) {
                use crate::adjunto::portapapeles::{Contenido, leer};
                match leer() {
                    Contenido::Texto(t) => {
                        b.entrada.push_str(t.trim());
                        b.error = None;
                    }
                    _ => b.error = Some("En el portapapeles no hay texto que pegar.".into()),
                }
            }
            return;
        }

        match b.paso.clone() {
            Paso::Portada => match k.code {
                KeyCode::Enter | KeyCode::Char(' ') => b.paso = Paso::Eleccion,
                KeyCode::Esc => self.cerrar_bienvenida(false),
                _ => {}
            },

            Paso::Eleccion => match k.code {
                KeyCode::Up => b.mover(-1),
                KeyCode::Down | KeyCode::Tab => b.mover(1),
                KeyCode::Char('o') | KeyCode::Char('O') => self.abrir_pagina(),
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    if !b.marcar_por_defecto() {
                        b.error = Some(
                            "Configúralo primero: solo puede arrancar con uno ya listo.".into(),
                        );
                    } else {
                        b.error = None;
                    }
                }
                KeyCode::Esc => self.terminar_bienvenida(),
                KeyCode::Enter => {
                    let i = b.sel;
                    if b.es_fila_final(i) {
                        self.terminar_bienvenida();
                    } else {
                        self.elegir_servicio(i);
                    }
                }
                _ => {}
            },

            Paso::Clave { i } => match k.code {
                KeyCode::Esc => {
                    b.entrada.clear();
                    b.error = None;
                    b.paso = Paso::Eleccion;
                }
                KeyCode::Backspace => {
                    b.entrada.pop();
                    b.error = None;
                }
                KeyCode::Char(c) => {
                    b.entrada.push(c);
                    b.error = None;
                }
                KeyCode::Enter => self.guardar_servicio(i),
                _ => {}
            },

            Paso::Hecho => match k.code {
                KeyCode::Enter | KeyCode::Esc => self.cerrar_bienvenida(true),
                _ => {}
            },
        }
    }

    /// El ratón dentro de la ventana de configuración inicial.
    pub(super) fn bienvenida_mouse(&mut self, m: crossterm::event::MouseEvent) {
        use crate::bienvenida::{Clic, Paso, clic_en};
        use crossterm::event::{MouseButton, MouseEventKind};

        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }
        let Some(b) = self.bienvenida.as_mut() else {
            return;
        };
        if b.paso != Paso::Eleccion {
            return;
        }

        // El recuadro se calcula con la misma función que lo dibuja: uno
        // parecido dejaría el clic una fila por encima del enlace.
        let zona = ui::zona_bienvenida(ratatui::layout::Rect::new(0, 0, self.ancho, self.alto));
        let dentro = m.column > zona.x
            && m.column < zona.x + zona.width - 1
            && m.row > zona.y
            && m.row < zona.y + zona.height - 1;
        if !dentro {
            return;
        }

        let fila = (m.row - zona.y - 1) as usize;
        match clic_en(b.sel, fila) {
            Clic::Elegir(i) => {
                b.sel = i;
                b.error = None;
            }
            Clic::Abrir => self.abrir_pagina(),
            Clic::Nada => {}
        }
        self.shell.mark_dirty();
    }

    /// Abre en el navegador la página del servicio señalado.
    fn abrir_pagina(&mut self) {
        use crate::bienvenida::{SERVICIOS, abrir_url};

        let Some(b) = self.bienvenida.as_mut() else {
            return;
        };
        let Some(s) = SERVICIOS.get(b.sel) else {
            return;
        };
        if s.url.is_empty() {
            return;
        }
        b.error = match abrir_url(s.url) {
            Ok(()) => None,
            // Sin escritorio no hay navegador que abrir —por SSH es lo
            // normal—, y ahí lo útil es la dirección escrita, que ya está en
            // pantalla.
            Err(_) => Some(format!(
                "No pude abrir el navegador. La dirección es {}",
                s.url
            )),
        };
    }

    /// Se ha elegido un servicio de la lista.
    fn elegir_servicio(&mut self, i: usize) {
        use crate::bienvenida::{Bienvenida, Paso, SERVICIOS};

        let Some(b) = self.bienvenida.as_mut() else {
            return;
        };
        let s = &SERVICIOS[i];
        b.error = None;

        // El proceso local no lleva clave: o está instalado o no está.
        if s.es_daemon() {
            if !b.hay_claude {
                b.error = Some(format!(
                    "No encuentro «claude» en el PATH. Instálalo desde {} y vuelve.",
                    s.url
                ));
                return;
            }
            if !b.listos.contains(&i) {
                b.listos.push(i);
            }
            b.por_defecto.get_or_insert(i);
            b.resumen
                .push("Claude Code, con tu suscripción de Anthropic.".into());
            return;
        }

        // Ollama no tiene clave que pedir, y la que ya está exportada tampoco
        // hay que volver a teclearla. Pero si el servicio ya estaba
        // configurado, Enter significa «cámbiame la clave»: es la única forma
        // de rotar una clave caducada sin editar ficheros a mano.
        if s.variable.is_empty() || (Bienvenida::ya_en_entorno(i) && !b.esta_listo(i)) {
            self.registrar_servicio(i, None);
            return;
        }

        b.paso = Paso::Clave { i };
        b.entrada.clear();
    }

    /// Comprueba y guarda la clave tecleada.
    fn guardar_servicio(&mut self, i: usize) {
        use crate::bienvenida::clave_plausible;

        let Some(b) = self.bienvenida.as_mut() else {
            return;
        };
        let clave = b.entrada.trim().to_string();
        if let Err(e) = clave_plausible(&clave) {
            b.error = Some(e);
            return;
        }
        self.registrar_servicio(i, Some(clave));
    }

    /// Deja constancia de un servicio configurado, con su clave si la tiene.
    fn registrar_servicio(&mut self, i: usize, clave: Option<String>) {
        use crate::bienvenida::{Paso, SERVICIOS, guardar_clave};

        let Some(b) = self.bienvenida.as_mut() else {
            return;
        };
        let s = &SERVICIOS[i];

        if let Some(c) = clave {
            match guardar_clave(s.variable, &c) {
                Ok(ruta) => b.resumen.push(format!(
                    "{}: clave guardada en {}",
                    s.nombre,
                    crate::bienvenida::corta(&ruta)
                )),
                Err(e) => {
                    b.error = Some(format!("No pude escribir el fichero de claves: {e}"));
                    return;
                }
            }
        } else if s.variable.is_empty() {
            b.resumen
                .push(format!("{}: sin clave, va por la red local.", s.nombre));
        } else {
            b.resumen.push(format!(
                "{}: uso la clave que ya tienes en {}.",
                s.nombre, s.variable
            ));
        }

        if !b.listos.contains(&i) {
            b.listos.push(i);
        }
        // El primero que se configura manda, salvo que luego se configure otro
        // y este fuera el proceso local: quien se molesta en dar una clave
        // quiere usar ese servicio.
        b.por_defecto.get_or_insert(i);
        b.entrada.clear();
        b.error = None;
        b.paso = Paso::Eleccion;
    }

    /// Escribe la configuración y arranca con ella.
    fn terminar_bienvenida(&mut self) {
        use crate::bienvenida::{Paso, guardar_toml};

        let Some(b) = self.bienvenida.as_mut() else {
            return;
        };

        // Sin nada elegido no se escribe un fichero vacío: se deja el disco
        // como estaba, y la próxima vez vuelve a ofrecerse la puesta en marcha.
        if b.listos.is_empty() {
            self.cerrar_bienvenida(false);
            return;
        }

        let listos = b.listos.clone();
        let por_defecto = b.por_defecto;
        match guardar_toml(&listos, por_defecto) {
            Ok(ruta) => {
                b.resumen.push(format!(
                    "Configuración escrita en {}",
                    crate::bienvenida::corta(&ruta)
                ));
                if let Some(i) = por_defecto {
                    b.resumen.push(format!(
                        "Al arrancar preguntaré a «{}». Se cambia con /modelo.",
                        crate::bienvenida::SERVICIOS[i].id
                    ));
                }
                b.paso = Paso::Hecho;
            }
            Err(e) => {
                b.error = Some(format!("No pude escribir la configuración: {e}"));
            }
        }
    }

    /// Cierra la ventana y, si hay configuración nueva, arranca con ella.
    ///
    /// Releer el fichero recién escrito en vez de construir el estado en
    /// memoria es a propósito: así lo que se pone en marcha es exactamente lo
    /// que hay en el disco, y un error en lo que se escribe se ve ahora y no
    /// en el siguiente arranque.
    fn cerrar_bienvenida(&mut self, aplicar: bool) {
        self.bienvenida = None;
        self.shell.mark_dirty();

        if !aplicar {
            self.push(
                Role::System,
                "Sin configurar. Cuando quieras ponerlo en marcha, /configurar.".into(),
            );
            return;
        }

        let (catalogo, entorno) = crate::proveedores::cargar();
        self.catalogo = catalogo;
        self.entorno = entorno;

        // `aplicar_por_defecto` deja `cfg` apuntando al proveedor elegido; el
        // cerebro se relanza con eso.
        let aviso =
            crate::proveedores::aplicar_por_defecto(&mut self.cfg, &self.catalogo, &self.entorno);
        match aviso {
            Some(a) => self.relanzar_cerebro(&a),
            None => self.relanzar_cerebro("Listo. Pregúntame lo que quieras."),
        }
    }
}
