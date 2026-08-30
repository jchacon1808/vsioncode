//! El modo de voz: oyente continuo y respuesta hablada.
//!
//! Vive aparte porque es la pieza más independiente del estado: entra por su
//! propio brazo del `select!`, lleva su propia generación y los campos que
//! toca no los mira nadie más. Es submódulo de `app` —y no un módulo hermano—
//! para seguir viendo lo privado de [`App`] sin abrirlo al resto del programa.

use super::{App, Focus, Role};
use crate::voz::{self, Boca, Oido, VozEvento};

impl App {
    /// Enciende o apaga el modo de voz continuo.
    ///
    /// Continuo de verdad: no hay que pulsar nada para hablar. El micrófono se
    /// queda escuchando, corta por frases, y cada frase entra como si la
    /// hubieras escrito. Si además puede hablar, contesta en voz alta.
    pub(super) fn alternar_voz(&mut self) {
        if self.oido.is_some() {
            self.apagar_voz();
            return;
        }

        if !self.motores.puede_oir() {
            let falta = self.motores.que_falta().join("; ");
            self.push(Role::System, format!("No puedo escuchar: {falta}"));
            return;
        }

        self.voz_gen += 1;
        match Oido::arrancar(&self.voz_modelo, self.voz_gen, self.voz_tx.clone()) {
            Ok(o) => {
                self.oido = Some(o);
                self.boca = Boca::nueva(&self.motores);
                let con_voz = if self.boca.is_some() {
                    "y te contesto en voz alta"
                } else {
                    "pero contestaré por escrito (falta piper)"
                };
                self.push(
                    Role::System,
                    format!(
                        "Modo de voz encendido: habla cuando quieras {con_voz}. \
                         Calibrando el ruido de fondo…"
                    ),
                );
            }
            Err(e) => self.push(Role::System, format!("No pude arrancar el oyente: {e}")),
        }
    }

    pub(super) fn apagar_voz(&mut self) {
        // La generación sube ANTES de cerrar: desde este instante, nada de lo
        // que emita el oyente anterior cuenta, tarde lo que tarde en morir.
        self.voz_gen += 1;
        if let Some(mut o) = self.oido.take() {
            // `block_on` de futures dentro del runtime de tokio no hace
            // progresar la entrada y salida de tokio y se queda colgado. Este
            // cierre tiene que correr en el runtime, no fuera.
            tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(o.cerrar()));
        }
        // Callar ANTES de soltarla: si primero se suelta, la tarea de voz se
        // queda sin quien le diga que pare y termina de leer lo que tenía.
        if let Some(b) = self.boca.take() {
            b.callar();
        }
        self.oyendo = false;
        self.push(Role::System, "Modo de voz apagado.".into());
    }

    pub(super) async fn on_voz(&mut self, ev: VozEvento) {
        match ev {
            VozEvento::Listo { modelo, aviso } => {
                self.oyendo = false;
                let mut t = format!("Escuchando con «{modelo}». Habla cuando quieras.");
                if let Some(a) = aviso {
                    t.push_str(&format!("\n\nOjo: {a}."));
                }
                self.push(Role::System, t);
            }
            VozEvento::Oyendo => self.oyendo = true,
            VozEvento::Callado => self.oyendo = false,
            VozEvento::Parcial(t) => {
                self.oyendo = false;
                if voz::parece_alucinacion(&t) {
                    return;
                }

                // ¿Nos hemos oído a nosotros mismos, o es él interrumpiendo?
                if let Some(b) = self.boca.as_mut()
                    && b.hablando()
                {
                    if voz::parece_eco(&t, &b.diciendo()) {
                        return;
                    }
                    // No se parece a lo que estamos diciendo: quiere hablar.
                    // Callarse en el acto es lo que distingue una
                    // conversación de un monólogo por turnos.
                    b.callar();
                    self.push(Role::System, "(te escucho)".into());
                }
                // Se va acumulando en la caja, a la vista. Ni se envía todavía
                // ni se pisa lo anterior: hablar con pausas es lo normal.
                if !self.input.is_empty() && !self.input.ends_with(' ') {
                    self.input.push(' ');
                }
                self.input.push_str(&t);
                self.cursor = self.input.chars().count();
                self.focus = Focus::Agent;
            }
            VozEvento::FinTurno => {
                // Lo dicho entra por donde entra lo escrito: mismos comandos
                // de barra, mismo contexto. Decir «barra modo ejecutar»
                // funciona igual que teclearlo.
                if !self.input.trim().is_empty() {
                    self.submit_input();
                }
            }
            VozEvento::Error(e) => self.push(Role::System, format!("Micrófono: {e}")),
            VozEvento::Cerrado => {
                // Pasa si el micrófono desaparece (se desenchufa, lo coge otro
                // programa). Decir solo «se cerró» dejaría a alguien hablándole
                // a una pantalla que ya no escucha.
                self.oido = None;
                self.oyendo = false;
                self.boca = None;
                self.push(
                    Role::System,
                    "El micrófono dejó de dar audio, así que el modo de voz se ha apagado. \
                     Alt+V para volver a intentarlo."
                        .into(),
                );
            }
        }
        self.shell.mark_dirty();
    }

    /// Dice en voz alta la última respuesta del agente.
    ///
    /// **Sin dejar de escuchar.** Mientras habla se oye a sí mismo, pero eso se
    /// resuelve comparando lo transcrito con lo que está diciendo, no
    /// tapándose los oídos. Cerrar el micrófono mientras habla significa que
    /// durante ocho segundos no puedes ni decirle «para, no era eso», y eso es
    /// lo que hace que una conversación no fluya.
    ///
    /// No es `async` aunque lo parezca: `decir` deja el texto en la tarea de
    /// voz y vuelve. Serlo obligaba a esperarla desde un sitio síncrono, y
    /// esa espera es justo lo que no puede haber dentro del runtime.
    pub(super) fn hablar(&mut self, texto: &str) {
        let Some(boca) = self.boca.as_mut() else {
            return;
        };
        if let Err(e) = boca.decir(texto) {
            self.push(Role::System, format!("No pude hablar: {e}"));
        }
    }

    pub fn voz_encendida(&self) -> bool {
        self.oido.is_some()
    }
}
