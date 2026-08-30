//! El selector de modelos: la ventana de `/modelo`.
//!
//! `Fila` y `Selector` viven aquí y no en `app.rs` porque son un tipo con sus
//! propias reglas —el recorrido que se salta las cabeceras, el modelo
//! señalado— y no vocabulario del estado: nadie más que esta ventana los
//! construye. Se reexportan desde `app` porque la vista los dibuja.
//!
//! Submódulo de `app` para seguir viendo lo privado de [`App`] sin abrir sus
//! campos al resto del programa.

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, Role};

/// Una fila del selector de modelos.
///
/// Las cabeceras van en la misma lista que los modelos, y no al margen, porque
/// así el recorrido con las flechas y el dibujado leen exactamente lo mismo.
/// Al moverse se saltan solas.
#[derive(Debug, Clone)]
pub enum Fila {
    /// Un servidor: `groq`, `nvidia`…
    Servidor { id: String, detalle: String },
    /// Uno de sus modelos.
    Modelo {
        proveedor: String,
        /// El id completo, el que espera la API.
        id: String,
        /// Cómo se enseña: sin el prefijo del fabricante.
        corto: String,
        nota: String,
        activo: bool,
    },
}

/// La ventana para elegir modelo.
///
/// Existe porque la lista dejó de caber en una línea de texto: con cinco
/// proveedores y cuatro o cinco modelos cada uno, volcarlo en la conversación
/// obliga a leer veinte líneas para elegir una. Agrupado por servidor y con el
/// tiempo de respuesta al lado, elegir es mirar.
#[derive(Debug, Clone)]
pub struct Selector {
    pub filas: Vec<Fila>,
    pub sel: usize,
}

impl Selector {
    /// Mueve la selección saltándose las cabeceras.
    ///
    /// `paso` es +1 o -1. Da la vuelta al llegar al final, que con la lista
    /// larga ahorra recorrerla entera para llegar al primero.
    pub fn mover(&mut self, paso: isize) {
        let n = self.filas.len();
        if n == 0 {
            return;
        }
        for _ in 0..n {
            let i = (self.sel as isize + paso).rem_euclid(n as isize) as usize;
            self.sel = i;
            if matches!(self.filas[i], Fila::Modelo { .. }) {
                return;
            }
        }
    }

    /// El modelo señalado, si lo que hay bajo el cursor es un modelo.
    pub fn elegido(&self) -> Option<(&str, &str)> {
        match self.filas.get(self.sel)? {
            Fila::Modelo { proveedor, id, .. } => Some((proveedor, id)),
            Fila::Servidor { .. } => None,
        }
    }
}

impl App {
    /// Arma la ventana de modelos: un servidor, sus modelos, el siguiente.
    ///
    /// El orden es el del fichero, a propósito: es el que ha escrito el
    /// usuario, y reordenarlo por cualquier criterio nuestro le quitaría el
    /// único sitio donde puede decidir qué ve primero.
    pub(super) fn armar_selector(&self) -> Selector {
        let mut filas = Vec::new();

        for p in &self.catalogo.proveedor {
            // Lo declarado manda, y los alias de serie son solo el respaldo de
            // `claude`. Antes eran siempre los tres del binario: escribir tus
            // modelos de claude en el fichero no cambiaba esta ventana.
            let ofrecidos = p.a_elegir();
            if p.es_daemon() {
                filas.push(Fila::Servidor {
                    id: p.id.clone(),
                    detalle: "tu suscripción".into(),
                });
                for m in &ofrecidos {
                    // Por igualdad o por contención: `cfg.model` guarda tanto
                    // el alias corto que escribe `/modelo` como el id entero
                    // con el que se arranca al probar.
                    let activo = self.cfg.proveedor.is_none()
                        && self
                            .cfg
                            .model
                            .as_deref()
                            .is_some_and(|a| a == m.id || a.contains(m.corto()));
                    filas.push(Fila::Modelo {
                        proveedor: p.id.clone(),
                        id: m.id.clone(),
                        corto: m.corto().to_string(),
                        nota: m.nota.clone(),
                        activo,
                    });
                }
                continue;
            }

            if ofrecidos.is_empty() {
                continue;
            }

            // Que falte la clave se dice en la cabecera del servidor y no en
            // cada modelo: es del servidor, y repetirlo cinco veces sería
            // ruido en vez de aviso.
            let detalle = if p.clave(&self.entorno).is_empty() && !p.base_url.contains("localhost")
            {
                "le falta la clave".to_string()
            } else {
                String::new()
            };
            filas.push(Fila::Servidor {
                id: p.id.clone(),
                detalle,
            });

            for m in &ofrecidos {
                let activo = self
                    .cfg
                    .proveedor
                    .as_ref()
                    .is_some_and(|a| a.id == p.id && a.modelo == m.id);
                filas.push(Fila::Modelo {
                    proveedor: p.id.clone(),
                    id: m.id.clone(),
                    corto: m.corto().to_string(),
                    nota: m.nota.clone(),
                    activo,
                });
            }
        }

        // Se abre sobre el que está en uso: es la referencia desde la que se
        // decide, y empezar siempre arriba obliga a buscarlo cada vez.
        let sel = filas
            .iter()
            .position(|f| matches!(f, Fila::Modelo { activo: true, .. }))
            .or_else(|| filas.iter().position(|f| matches!(f, Fila::Modelo { .. })))
            .unwrap_or(0);

        Selector { filas, sel }
    }
    /// Cambia a un modelo concreto de un proveedor concreto.
    ///
    /// Lo usan los dos caminos por los que se elige un modelo: esta ventana y
    /// `/modelo <nombre>`. Que sean uno solo es lo que evita que elegir por
    /// escrito acabe haciendo algo distinto de elegir señalando.
    pub(super) fn cambiar_a_modelo(&mut self, proveedor: &str, modelo: &str) {
        let Some(p) = self
            .catalogo
            .proveedor
            .iter()
            .find(|p| p.id == proveedor)
            .cloned()
        else {
            return;
        };

        if p.es_daemon() {
            self.cfg.proveedor = None;
            self.cfg.model = Some(modelo.to_string());
            self.relanzar_cerebro(&format!("Ahora pregunto a «{proveedor}» con {modelo}."));
            return;
        }

        let Some(m) = p.ofrecidos().into_iter().find(|m| m.id == modelo) else {
            return;
        };
        let prov = p.con_modelo(&self.entorno, &m);
        if prov.api_key.is_empty() && !prov.base_url.contains("localhost") {
            self.push(
                Role::System,
                format!(
                    "«{proveedor}» no tiene clave. La busco en {}; declárala ahí o \
                     expórtala.",
                    self.catalogo.env_file.as_deref().unwrap_or("el entorno")
                ),
            );
            return;
        }
        let aviso = format!("Ahora pregunto a «{proveedor}» con {modelo}.");
        self.cfg.proveedor = Some(prov);
        self.relanzar_cerebro(&aviso);
    }
    /// Atiende una tecla mientras la ventana de modelos está abierta.
    ///
    /// Devuelve `true` si se la queda: con la ventana delante, las flechas son
    /// suyas y no del shell.
    pub(super) fn selector_key(&mut self, k: KeyEvent) -> bool {
        let Some(sel) = self.selector.as_mut() else {
            return false;
        };
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => sel.mover(-1),
            KeyCode::Down | KeyCode::Char('j') => sel.mover(1),
            KeyCode::Esc | KeyCode::Char('q') => self.selector = None,
            KeyCode::Enter => {
                let elegido = sel.elegido().map(|(p, m)| (p.to_string(), m.to_string()));
                self.selector = None;
                if let Some((p, m)) = elegido {
                    self.cambiar_a_modelo(&p, &m);
                }
            }
            _ => return false,
        }
        self.shell.mark_dirty();
        true
    }
}
