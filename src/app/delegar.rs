//! La barra de delegar: darle una tarea a un subagente sin escribir un comando.
//!
//! `/agente <tarea>` ya servía para esto, pero pedía escribir el comando, y
//! elegir con quién iba obligaba a acordarse del nombre o a bajar dieciocho
//! filas del menú. Delegar es de las cosas que más se usan en cuanto se sabe
//! que están, y lo que más se usa es lo que menos debe costar teclear.
//!
//! Se abre con **←** desde la caja del agente —solo con la caja vacía, que si
//! hay texto la flecha es del cursor y robársela sería romper lo que ya se
//! sabe— o escribiendo `/agente`. Dentro: la tarea se escribe, **↑** con la
//! caja vacía despliega los agentes que hay, Enter envía y Esc se va sin
//! hacer nada.
//!
//! Submódulo de `app` para seguir viendo lo privado de [`App`] sin abrir sus
//! campos al resto del programa.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Role, byte_de};
use crate::agente::Posible;

/// La barra abierta, con lo que se lleva escrito.
#[derive(Debug, Clone)]
pub struct Delegar {
    /// El encargo, según se escribe.
    pub tarea: String,
    /// Dónde está el cursor, en caracteres.
    pub cursor: usize,
    /// Con quién va. Sale del mismo sitio que `/agente`: lo que hay de verdad.
    pub agente: String,
    /// Los agentes que había al abrir, con su motor ya resuelto.
    ///
    /// Congelados a propósito. Recalcularlos en cada tecla dejaría la fila
    /// señalada apuntando a otra cosa si el fichero cambia mientras eliges, y
    /// la lista de esta máquina no cambia entre dos pulsaciones.
    pub agentes: Vec<Posible>,
    /// La lista desplegada, con la fila señalada. `None` mientras está cerrada.
    pub lista: Option<usize>,
}

impl Delegar {
    /// Mueve la selección de la lista, dando la vuelta por los extremos.
    fn mover(&mut self, paso: isize) {
        let n = self.agentes.len();
        if n == 0 {
            return;
        }
        let i = self.lista.unwrap_or(0) as isize;
        self.lista = Some((i + paso).rem_euclid(n as isize) as usize);
    }

    /// Deja la lista abierta por donde está el agente de ahora.
    ///
    /// Empezar siempre por el primero obligaría a buscar con la vista dónde
    /// está el que se está usando, que es justo desde donde se decide cambiar.
    fn abrir_lista(&mut self) {
        let i = self
            .agentes
            .iter()
            .position(|p| p.nombre == self.agente)
            .unwrap_or(0);
        self.lista = Some(i);
    }

    /// De quién es el agente de ahora: `claude`, `groq`, `gemini`…
    ///
    /// Se enseña junto al nombre porque es la mitad de la respuesta a «¿quién
    /// va a hacer esto?»: `haiku` sale de tu suscripción y `qwen3.6-27b` de tu
    /// clave de Groq, y eso cambia lo que cuesta y lo que tarda.
    pub fn proveedor(&self) -> &str {
        self.agentes
            .iter()
            .find(|p| p.nombre == self.agente)
            .map(|p| p.proveedor.as_str())
            .unwrap_or_default()
    }
}

impl App {
    /// Abre la barra, si hay alguien a quien encargarle algo.
    ///
    /// Con la lista vacía no se abre: una barra donde no se puede elegir
    /// nada y que al enviar no haría nada es peor que decir por qué no.
    pub(super) fn abrir_delegar(&mut self) {
        let agentes = self.subagentes_posibles();
        if agentes.is_empty() {
            self.push(
                Role::System,
                "No hay ningún modelo configurado con el que lanzar un subagente. \
                 Añade un proveedor con /configurar."
                    .into(),
            );
            return;
        }
        self.delegar = Some(Delegar {
            tarea: String::new(),
            cursor: 0,
            agente: self.techo_subagente(),
            agentes,
            lista: None,
        });
        self.shell.mark_dirty();
    }

    /// Atiende una tecla mientras la barra está abierta.
    ///
    /// Devuelve `true` si se la queda. Lo que no es suyo sigue su camino, así
    /// que Alt+Q sigue saliendo y Alt+H sigue abriendo la ayuda: la barra es
    /// una capa encima de la caja, no un modo del que haya que escapar.
    pub(super) fn delegar_key(&mut self, k: KeyEvent) -> bool {
        // Solo con el foco en el agente. Con el foco en el shell, lo que se
        // teclea es para el shell: quedarse esas teclas dejaría la barra
        // comiéndose lo que el usuario escribe en su terminal, que es el peor
        // fallo posible aquí. La barra sigue abierta y con lo escrito dentro;
        // vuelve sola al volver el foco.
        if self.focus != super::Focus::Agent {
            return false;
        }
        let Some(d) = self.delegar.as_mut() else {
            return false;
        };
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

        // Con la lista desplegada las flechas son suyas. Escribir la cierra y
        // se queda la letra: quien empieza a teclear ya ha elegido.
        if d.lista.is_some() {
            match k.code {
                KeyCode::Up => {
                    d.mover(-1);
                    self.shell.mark_dirty();
                    return true;
                }
                KeyCode::Down => {
                    d.mover(1);
                    self.shell.mark_dirty();
                    return true;
                }
                KeyCode::Esc => {
                    d.lista = None;
                    self.shell.mark_dirty();
                    return true;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    let elegido = d
                        .lista
                        .and_then(|i| d.agentes.get(i))
                        .map(|p| p.nombre.clone());
                    d.lista = None;
                    if let Some(n) = elegido {
                        self.fijar_agente(n);
                    }
                    self.shell.mark_dirty();
                    return true;
                }
                _ => {
                    d.lista = None;
                }
            }
        }

        match k.code {
            // Solo con la caja vacía, igual que la flecha que abre la barra:
            // con texto escrito, ↑ no puede significar dos cosas.
            KeyCode::Up if d.tarea.is_empty() => d.abrir_lista(),
            KeyCode::Esc => {
                self.delegar = None;
            }
            KeyCode::Enter if !d.tarea.trim().is_empty() => {
                let tarea = d.tarea.trim().to_string();
                let agente = d.agente.clone();
                self.delegar = None;
                self.encargar(tarea, Some(agente));
            }
            // Enter con la caja vacía no envía nada, y tampoco cierra: cerrar
            // por una pulsación de más es perder lo que estabas eligiendo.
            KeyCode::Enter => {}
            KeyCode::Char(c) if !ctrl && !alt => {
                let byte = byte_de(&d.tarea, d.cursor);
                d.tarea.insert(byte, c);
                d.cursor += 1;
            }
            KeyCode::Backspace => {
                if d.cursor > 0 {
                    d.cursor -= 1;
                    let byte = byte_de(&d.tarea, d.cursor);
                    d.tarea.remove(byte);
                }
            }
            KeyCode::Left => d.cursor = d.cursor.saturating_sub(1),
            KeyCode::Right => d.cursor = (d.cursor + 1).min(d.tarea.chars().count()),
            KeyCode::Home => d.cursor = 0,
            KeyCode::End => d.cursor = d.tarea.chars().count(),
            KeyCode::Char('u') if ctrl => {
                d.tarea.clear();
                d.cursor = 0;
            }
            _ => return false,
        }
        self.shell.mark_dirty();
        true
    }

    /// Deja fijado con quién van los subagentes desde ahora.
    ///
    /// Se queda para la sesión y no solo para este encargo: quien elige un
    /// agente en la barra suele querer ese mismo para lo siguiente, y volver a
    /// elegirlo cada vez sería cobrar dos pulsaciones por una decisión que ya
    /// se tomó. Es exactamente lo que hace `/agente modelo <n>`, y por eso lo
    /// cuenta igual — un cambio de modelo que no se dice no se distingue de
    /// que no haya pasado nada.
    fn fijar_agente(&mut self, nombre: String) {
        if let Some(d) = self.delegar.as_mut() {
            d.agente = nombre.clone();
        }
        if self.sub_modelo.as_deref() == Some(nombre.as_str()) {
            return;
        }
        self.sub_modelo = Some(nombre.clone());
        self.push(Role::System, format!("Los subagentes van con «{nombre}»."));
    }
}

#[cfg(test)]
mod tests {
    use super::{Delegar, Posible};

    fn barra() -> Delegar {
        Delegar {
            tarea: String::new(),
            cursor: 0,
            agente: "sonnet".into(),
            agentes: ["haiku", "sonnet", "qwen3.6-27b"]
                .into_iter()
                .map(|n| Posible {
                    nombre: n.into(),
                    proveedor: if n == "qwen3.6-27b" { "groq" } else { "claude" }.into(),
                    detalle: "una nota".into(),
                    motor: crate::agente::Motor::Daemon(n.into()),
                })
                .collect(),
            lista: None,
        }
    }

    /// La lista se abre por donde estás, no por el principio: es desde ahí
    /// desde donde se decide cambiar.
    #[test]
    fn la_lista_se_abre_en_el_agente_de_ahora() {
        let mut d = barra();
        d.abrir_lista();
        assert_eq!(d.lista, Some(1));
    }

    /// Y da la vuelta por los dos extremos, que con dieciocho agentes ahorra
    /// recorrer la lista entera para llegar al último.
    #[test]
    fn la_lista_da_la_vuelta() {
        let mut d = barra();
        d.lista = Some(0);
        d.mover(-1);
        assert_eq!(d.lista, Some(2));
        d.mover(1);
        assert_eq!(d.lista, Some(0));
    }

    /// Un agente que ya no está no deja la lista señalando a la nada.
    #[test]
    fn si_el_agente_no_esta_se_empieza_por_el_primero() {
        let mut d = barra();
        d.agente = "uno-que-se-fue".into();
        d.abrir_lista();
        assert_eq!(d.lista, Some(0));
        assert_eq!(d.proveedor(), "", "y no se inventa de quién era");
    }

    /// De quién es el agente sale del catálogo, no del nombre.
    #[test]
    fn se_sabe_de_quien_es_el_agente() {
        let mut d = barra();
        assert_eq!(d.proveedor(), "claude");
        d.agente = "qwen3.6-27b".into();
        assert_eq!(d.proveedor(), "groq");
    }
}
