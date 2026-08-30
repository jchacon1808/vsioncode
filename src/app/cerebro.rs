//! El cerebro: hablar con el modelo y gobernar su ciclo de vida.
//!
//! Es el módulo del que dependen todos los demás —cualquier cosa que quiera
//! decirle algo al modelo pasa por `send`—, y por eso sale el último de los
//! que tienen entrada propia en el `select!`: hasta que existe, cada corte
//! anterior arrastra un `send` consigo.
//!
//! Aquí vive también lo que decide **de qué generación** es una respuesta.
//! Cambiar de modelo o interrumpir un turno levanta un proceso nuevo, y el
//! viejo puede seguir hablando un rato: sin el número de generación, lo que
//! contesta el que ya no manda se cuela en la conversación como si nada.
//!
//! Submódulo de `app` para seguir viendo lo privado de [`App`] sin abrir sus
//! campos al resto del programa.

use std::time::Instant;

use super::{App, ChatMsg, Mode, Role, plural};
use crate::ai::{AiEvent, AiMsg, Brain, Hilo, extract_commands};
use crate::{agente, edicion};

impl App {
    /// Filtra lo que llega de un cerebro que ya no es el actual.
    pub(super) fn on_ai_msg(&mut self, (generacion, ev): AiMsg) {
        if generacion == self.generacion {
            self.on_ai(ev);
        }
    }
    fn on_ai(&mut self, ev: AiEvent) {
        match ev {
            AiEvent::Ready { model, tools } => {
                self.model = model;
                // Invariante del diseño: el modelo solo puede leer la web.
                // Cualquier otra herramienta significaría que puede tocar el
                // sistema por su cuenta, y todo lo demás se apoya en que no.
                let indebidas = crate::ai::herramientas_indebidas(&tools);
                if !indebidas.is_empty() {
                    self.push(
                        Role::System,
                        format!(
                            "AVISO DE SEGURIDAD: el agente arrancó con herramientas que no \
                             debería tener ({}). No uses el modo ejecutar hasta comprobarlo.",
                            indebidas.join(", ")
                        ),
                    );
                }
                self.web = tools.iter().any(|t| t.starts_with("Web"));
            }
            AiEvent::TurnStart => {
                self.thinking = true;
                self.thinking_since = Some(Instant::now());
                // `message_start` NO es el principio del turno: cuando el
                // modelo sale a internet, el turno trae un mensaje suyo antes
                // de la llamada y otro después —dos `message_start`— y un solo
                // `result` al final. Vaciar aquí el acumulador se llevaba por
                // delante lo escrito antes de la búsqueda, y de ahí es de donde
                // `TurnEnd` saca los comandos y las ediciones: la propuesta
                // desaparecía y había que pedirla otra vez. El turno empieza
                // cuando se envía, así que el vaciado vive en `send_con`.
                //
                // Por lo mismo, la burbuja solo se abre si no hay una abierta:
                // en el segundo mensaje ya hay dónde escribir, y tras el aviso
                // de la herramienta —que es de sistema— hace falta una nueva.
                if !matches!(self.chat.last(), Some(m) if m.role == Role::Agent) {
                    self.push(Role::Agent, String::new());
                }
            }
            AiEvent::Thinking => {
                self.thinking = true;
            }
            AiEvent::Tool { name, detail } => {
                // Que el agente salga a internet se ve siempre, y se ve qué
                // busca. Un agente que consulta fuentes a tus espaldas no es
                // más cómodo, es menos fiable.
                //
                // Pero *intentarlo* no marca el turno: eso solo lo hace haber
                // recibido algo. Ver más abajo, en ToolResult.
                let que = if detail.is_empty() {
                    name.clone()
                } else {
                    detail
                };
                let verbo = if name == "WebFetch" {
                    "abriendo"
                } else {
                    "buscando"
                };
                self.push(Role::System, format!("{verbo}: {que}"));
                self.buscando = Some(que);
            }
            AiEvent::Text(t) => {
                // Los emojis se piden en el prompt y se quitan aquí. Pedirlo
                // no basta con modelos pequeños, y no es cuestión de gusto:
                // muchos ocupan dos columnas, o una, o ninguna según la
                // terminal, y entonces el ancho con el que se envolvió el
                // texto deja de cuadrar con lo que se pinta.
                let t = crate::ai::sin_emojis(&t);
                self.streaming.push_str(&t);
                if let Some(last) = self.chat.last_mut() {
                    if last.role == Role::Agent {
                        last.text.push_str(&t);
                    } else {
                        self.chat.push(ChatMsg {
                            role: Role::Agent,
                            text: t,
                        });
                    }
                }
                self.chat_scroll = 0;
                self.shell.mark_dirty();
            }
            AiEvent::Respuesta(texto) => {
                // Solo lo necesita el backend HTTP, que es sin estado: sin
                // esto, en el turno siguiente no recordaría lo que dijo.
                if let Some(b) = self.brain.as_mut() {
                    b.anotar_respuesta(texto);
                }
            }
            AiEvent::TurnEnd { cost_usd, uso, .. } => {
                self.thinking = false;
                self.thinking_since = None;
                self.last_cost += cost_usd;
                self.uso.sumar(&uso);
                self.uso_ultimo = uso;
                self.turnos += 1;
                if let Some(b) = self.brain.as_mut() {
                    b.turn_finished();
                }
                // Al cerrar el turno miramos si propuso algo ejecutable.
                let dicho = std::mem::take(&mut self.streaming);

                // ¿Te devuelve el mando de este comando? La palabra es suya y
                // se la queda vsioncode: en pantalla queda su explicación y su
                // comando, sin una etiqueta que no dice nada a quien lee.
                let cede = crate::ai::cede_el_mando(&dicho);
                if cede
                    && let Some(u) = self.chat.last_mut()
                    && u.role == Role::Agent
                {
                    u.text = crate::ai::sin_peticion(&u.text);
                }

                // ¿Ha dicho por qué paso del plan va? La marca es de vsioncode y
                // se la queda: en pantalla quedan sus frases, no una etiqueta
                // que a quien lee no le dice nada.
                if let Some(n) = crate::plan::avance(&dicho) {
                    self.marcar_paso(n);
                    if let Some(u) = self.chat.last_mut()
                        && u.role == Role::Agent
                    {
                        u.text = crate::plan::sin_avance(&u.text);
                    }
                }

                // ¿Ha pedido que le suelten la correa? Se le ofrece al humano;
                // concederlo no es cosa suya. El mismo principio que con los
                // comandos: propone, decides.
                if crate::ai::pide_modo_libre(&dicho) && !self.libre && self.mode == Mode::Ejecutar
                {
                    self.pide_libre = true;
                    if let Some(u) = self.chat.last_mut()
                        && u.role == Role::Agent
                    {
                        u.text = crate::ai::sin_peticion(&u.text);
                    }
                    self.push(
                        Role::System,
                        "El agente pide modo libre para esto. Alt+L se lo concede.".into(),
                    );
                }

                // ¿Ha pedido una skill? Va antes que todo lo demás: lo que
                // proponga después de leerla puede no parecerse a lo que iba
                // a proponer sin ella, así que no toca ejecutar nada todavía.
                if let Some(nombre) = crate::skills::pedida(&dicho) {
                    if let Some(u) = self.chat.last_mut()
                        && u.role == Role::Agent
                    {
                        u.text = crate::skills::sin_peticion(&u.text);
                    }
                    self.servir_skill(&nombre);
                    self.shell.mark_dirty();
                    return;
                }

                // ¿Ha encontrado de dónde traerse la que le mandaste buscar?
                //
                // Va aquí y no más abajo por lo mismo que la petición: lo
                // accionable del turno es esto, y un bloque de comando detrás
                // sería otra cosa distinta compitiendo por la misma tecla.
                //
                // Se arma el comando y se deja en tu línea **sin Enter**, que
                // es exactamente lo que pasa cuando el origen lo escribes tú.
                // El modelo dice de dónde; bajar de la red y escribir en el
                // disco siguen sin ser cosa suya, y aquí menos que nunca: lo
                // que se trae son las instrucciones que va a seguir después.
                if let Some(origen) = crate::skills::propuesta(&dicho) {
                    if let Some(u) = self.chat.last_mut()
                        && u.role == Role::Agent
                    {
                        u.text = crate::skills::sin_peticion(&u.text);
                    }
                    // Directo al armado, sin volver a pasar por la decisión de
                    // buscar: si lo que devuelve no vale como origen, se dice y
                    // se acabó. Reenviarlo a la búsqueda sería un bucle de
                    // llamadas al modelo pagado a cuenta de una errata suya.
                    self.armar_instalacion(&origen);
                    self.shell.mark_dirty();
                    return;
                }

                // ¿Ha repartido trabajo? Los subagentes van antes que los
                // comandos: lo que traigan puede cambiar qué comando toca.
                let mut encargos = agente::extraer(&dicho);
                // El modelo puede pedir gama baja para un encargo tonto, pero
                // no puede subirse a sí mismo el presupuesto: el techo lo pone
                // quien paga. Ver `agente::bajo_techo`.
                agente::bajo_techo(&mut encargos, &self.techo_subagente());
                if !encargos.is_empty() {
                    self.thinking = false;
                    self.auto.ha_producido();
                    self.repartir(encargos);
                    self.shell.mark_dirty();
                    return;
                }

                // ¿Ha traído un plan? Cierra el turno: lo accionable pasa a
                // ser la decisión del usuario, y un comando detrás sería
                // trabajo empezado sin que nadie lo apruebe.
                //
                // Vale en **cualquier fase viva**, y eso es lo que separa una
                // ruta de un formulario. Mirándolo solo mientras se trazaba, un
                // plan ya propuesto no se podía corregir hablando —«quita el
                // paso 2»— y uno en marcha que resultaba equivocado no tenía
                // por dónde replantearse: al agente solo le quedaba seguir un
                // plan que ya sabía malo o abandonarlo entero.
                //
                // Fuera de una planificación viva no se mira: el protocolo
                // viaja en la petición y no en el prompt de sistema —se paga en
                // cada turno y solo sirve aquí—, así que ahí un bloque `plan:`
                // es prosa del modelo y no una orden para vsioncode.
                if self.plan.is_some()
                    && let Some(p) = crate::plan::extraer(&dicho)
                {
                    self.plan_trazado(p);
                    self.thinking = false;
                    self.shell.mark_dirty();
                    return;
                }

                // ¿Ha propuesto cambiar un fichero? Va antes que los
                // comandos: si hay una edición, es lo accionable.
                // La última por el mismo motivo que el comando: si el turno
                // trae dos versiones del mismo cambio, manda la de después.
                let ediciones = edicion::extraer(&dicho, std::path::Path::new(self.ledger.cwd()));
                if let Some(e) = ediciones.last() {
                    match edicion::preparar(e) {
                        Ok(p) => {
                            // En libre las escribe él. En cualquier otro modo
                            // se queda esperando tu pulsación, que es lo que
                            // siempre ha hecho.
                            //
                            // Y trazando un plan tampoco, aunque el modo libre
                            // esté encendido: esa fase es para enterarse, y una
                            // edición aplicada mientras se piensa el trabajo es
                            // trabajo hecho sin aprobar.
                            if self.libre && !self.trazando_plan() {
                                self.aplicar_edicion(p);
                            } else {
                                let (fuera, dentro) = edicion::resumen(&p);
                                let extra = if p.como == edicion::Como::Exacto {
                                    String::new()
                                } else {
                                    format!(" ({})", p.como.label())
                                };
                                self.push(
                                    Role::System,
                                    format!(
                                        "Cambio en {}: −{fuera} +{dentro} líneas{extra}.\n{}\n\
                                         Alt+↵ para aplicarlo, Ctrl+N para descartarlo.",
                                        p.ruta.display(),
                                        edicion::diff(&p),
                                    ),
                                );
                                self.edicion = Some(p);
                            }
                        }
                        Err(f) => self.push(
                            Role::System,
                            format!("No pude preparar el cambio en {}: {f}", e.ruta.display()),
                        ),
                    }
                    self.thinking = false;
                    self.shell.mark_dirty();
                    return;
                }

                // Marcadores de edición que no se han podido leer: hay que
                // decirlo. Callar deja al modelo diciendo «ya está cambiado»
                // sobre un fichero que nadie ha tocado.
                if edicion::parece_edicion_fallida(&dicho) {
                    self.push(
                        Role::System,
                        "Has escrito un cambio pero no pude leerlo: la etiqueta del bloque \
                         tiene que ser ```edit: ruta/al/fichero. NO se ha modificado nada."
                            .into(),
                    );
                    self.thinking = false;
                    self.shell.mark_dirty();
                    return;
                }

                // El último, no el primero: un turno puede traer una propuesta
                // y luego su corrección —típico si el modelo busca en internet
                // a mitad de turno—, y quedarse con la primera es ejecutar la
                // versión que él mismo descartó.
                let propuesto = extract_commands(&dicho).into_iter().next_back();
                self.hablar(&dicho);
                match propuesto {
                    Some(cmd) => self.tras_la_respuesta(cmd, cede),
                    None => {
                        // Sin bloque de comando, el agente ha terminado.
                        if self.auto.pasos > 0 {
                            self.push(
                                Role::System,
                                format!(
                                    "Hecho en {}.",
                                    plural(self.auto.pasos, "comando", "comandos")
                                ),
                            );
                            self.auto.reiniciar();
                        }
                        // Y si lo que se acaba es un plan, se dice por dónde
                        // quedó. Es lo único de la tanda que vsioncode sabe y
                        // el modelo no: cuántos pasos había.
                        self.cerrar_plan("Ha terminado");
                    }
                }
                self.shell.mark_dirty();
            }
            AiEvent::ToolResult { ok } => {
                // Aquí, y solo aquí, se da el turno por contaminado.
                //
                // Marcarlo al *intentar* la búsqueda parecía más prudente y era
                // peor: una llamada fallida —el modelo pidiendo «about:blank»
                // por error, por ejemplo— bloqueaba después comandos
                // puramente locales del mismo turno, sin que hubiera entrado
                // ni una letra de fuera. Avisos así se aprenden a ignorar.
                if ok {
                    self.web_en_turno = true;
                } else {
                    self.buscando = None;
                }
            }
            AiEvent::WebRemota => {
                // Algunos proveedores ejecutan las herramientas en su
                // servidor y no las anuncian dentro del flujo SSE. Mientras
                // no haya un resultado por herramienta que comprobar, el
                // único comportamiento seguro es tratar el turno como web.
                self.web_en_turno = true;
            }
            AiEvent::Quota { kind, status, .. } => {
                self.quota = Some(format!("{kind}:{status}"));
            }
            AiEvent::Error(e) => {
                self.thinking = false;
                // Quedarse sin cuota no es haber terminado la tarea.
                //
                // Antes el error se anunciaba y, acto seguido, al no venir
                // ningún bloque de comando, el bucle daba la tarea por hecha:
                // «Hecho en 19 comandos». Parecía que había acabado y en
                // realidad se había quedado a medias.
                let sin_cuota = e.to_lowercase().contains("limit")
                    || e.to_lowercase().contains("quota")
                    || e.to_lowercase().contains("rate");
                if sin_cuota && self.auto.pasos > 0 {
                    let n = self.auto.pasos;
                    self.auto.reiniciar();
                    self.push(
                        Role::System,
                        format!(
                            "Se acabó la cuota tras {}. La tarea queda A MEDIAS, no \
                             terminada. {e}",
                            plural(n, "comando", "comandos")
                        ),
                    );
                } else {
                    self.push(Role::System, format!("Error del agente: {e}"));
                }
            }
            AiEvent::Died(e) => {
                self.thinking = false;
                self.brain = None;
                self.push(
                    Role::System,
                    format!("El agente se cerró ({e}). La terminal sigue."),
                );
            }
        }
    }
    pub(super) fn send(&mut self, prompt: String) {
        self.send_con(prompt, Vec::new());
    }
    pub(super) fn send_con(&mut self, prompt: String, imagenes: Vec<(&'static str, String)>) {
        // Cada turno empieza sin haber leído nada de internet. La marca solo
        // vale para el turno en curso.
        self.web_en_turno = false;
        self.buscando = None;
        // Y empieza sin nada dicho. Aquí y no en `TurnStart`, que llega una vez
        // por mensaje del modelo: esto ocurre exactamente una vez por turno.
        // También recoge lo que dejó a medias un turno muerto por un error,
        // que es lo único que aquel vaciado protegía de verdad.
        self.streaming.clear();
        let Some(brain) = self.brain.as_mut() else {
            self.push(Role::System, "No hay agente conectado.".into());
            return;
        };
        self.thinking = true;
        self.thinking_since = Some(Instant::now());
        // Un solo criterio en todo el fichero: lo que espera a tokio se espera
        // dentro del runtime.
        //
        // Aquí hubo mucho tiempo un `futures::executor::block_on` con el
        // argumento de que escribir una línea en una tubería no bloquea. Con
        // una imagen adjunta esa «línea» son varios megas de base64 —mucho más
        // de lo que traga una tubería de golpe—, y en cuanto no cabe entera
        // quien tiene que despertarnos es el reactor de tokio, al que ese
        // `block_on` no hace correr.
        let fut = brain.ask_con(&prompt, imagenes);
        if let Err(e) =
            tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
        {
            self.thinking = false;
            self.push(Role::System, format!("No pude enviar la pregunta: {e}"));
        } else {
            // A partir de aquí la conversación existe también para `claude`, y
            // solo a partir de aquí se puede retomar. Antes del primer turno
            // `cfg.sesion` es un uuid nuestro que él no ha visto nunca.
            self.sesion_estrenada = true;
        }
        self.shell.mark_dirty();
    }
    /// Corta el turno en curso y deja el cerebro en condiciones de responder.
    ///
    /// Se relanza en vez de limitarse a dejar de esperar, porque las dos veces
    /// que esto hace falta —el proceso colgado y la conexión rota— el cerebro
    /// no vuelve solo. Dejar de mirar no arregla nada; el siguiente turno se
    /// quedaría igual de mudo y sin que nadie supiera por qué.
    pub(super) fn interrumpir_turno(&mut self) {
        self.thinking = false;
        self.thinking_since = None;
        self.streaming.clear();
        self.auto.reiniciar();
        // Lo que estuviera planificando se acaba aquí, pero se dice qué queda.
        self.plan_cortado();
        // **Retomando, no empezando de cero.** Cortar un turno es decir «para»,
        // no «olvida». Esto se relanzaba con hilo nuevo y el precio era el peor
        // posible: pulsabas Esc a los veinte comandos de una investigación y el
        // agente que volvía no sabía nada de lo que acababa de averiguar —ni de
        // lo que ya había probado y descartado—. El proceso hay que relanzarlo
        // igual (las dos veces que esto hace falta, el proceso colgado y la
        // conexión rota, no vuelve solo); lo que no hay que tirar es la
        // conversación, y `--resume` la trae.
        //
        // Con un proveedor por HTTP no hay sesión que retomar: la historia la
        // guarda el propio backend y arrancar otro la pierde. Ahí se dice, que
        // prometer continuidad y no darla es peor que la propia pérdida.
        if self.cfg.proveedor.is_none() {
            self.relanzar_retomando(
                "Turno cortado. Seguimos donde estábamos: lo que ya se había hablado sigue ahí.",
            );
        } else {
            self.relanzar_cerebro(
                "Turno cortado. Con este proveedor la conversación empieza de nuevo; el \
                 registro de comandos se conserva.",
            );
        }
    }
    /// Le manda al modelo la skill que ha pedido.
    ///
    /// Leer una skill es leer un fichero de texto y ponérselo delante. **No se
    /// ejecuta nada** de lo que traiga dentro: ni sus `scripts/`, ni los
    /// bloques de comandos de sus ejemplos. Si de leerla concluye que hay que
    /// ejecutar algo, lo propone como cualquier otra cosa y pasa por el mismo
    /// filtro de riesgo. Una capacidad con un camino propio «porque es
    /// distinta» sería el principio del fin del modelo de seguridad.
    fn servir_skill(&mut self, nombre: &str) {
        let Some(skill) = crate::skills::buscar(&self.skills, nombre).cloned() else {
            let hay = self
                .skills
                .iter()
                .map(|s| s.nombre.clone())
                .collect::<Vec<_>>()
                .join(", ");
            self.push(
                Role::System,
                format!("El agente pidió la skill «{nombre}», y no existe."),
            );
            self.send(if hay.is_empty() {
                "No hay ninguna skill instalada. Sigue sin ella.".to_string()
            } else {
                format!(
                    "No existe ninguna skill «{nombre}». Las que hay son: {hay}. Sigue con \
                     lo que estabas haciendo, con una de esas o sin ninguna."
                )
            });
            return;
        };

        // Mandar dos veces la misma son treinta mil caracteres pagados por
        // nada. Y como aquí no se le contesta, tampoco puede quedarse pidiendo
        // en bucle una skill que ya tiene delante.
        if !self.skills_leidas.insert(skill.nombre.clone()) {
            self.push(
                Role::System,
                format!("(«{}» ya se la mandé en esta conversación)", skill.nombre),
            );
            return;
        }

        match crate::skills::leer(&skill) {
            Ok(cuerpo) => {
                self.push(
                    Role::System,
                    format!(
                        "skill: {} ({})",
                        skill.nombre,
                        plural(cuerpo.lines().count(), "línea", "líneas")
                    ),
                );
                self.send(crate::skills::inyectar(&skill, &cuerpo));
            }
            Err(e) => {
                // Si no se pudo leer, no cuenta como leída: la culpa puede ser
                // de un permiso que se arregle en un minuto.
                self.skills_leidas.remove(&skill.nombre);
                self.push(
                    Role::System,
                    format!("No pude leer {}: {e}", skill.ruta.display()),
                );
                self.send(format!(
                    "No he podido leerte la skill «{}». Sigue sin ella.",
                    skill.nombre
                ));
            }
        }
    }
    /// Relanza el cerebro empezando conversación nueva, con un aviso propio.
    ///
    /// Es el camino para cuando cambia **con quién** hablas: otro modelo, otro
    /// proveedor, otras herramientas. Ahí olvidar no es un daño colateral, es
    /// lo correcto: lo dicho a otro no se le puede dar por sabido a este. Cada
    /// llamante trae su aviso porque decir «cambiado a haiku» cuando lo que
    /// cambió fue el esfuerzo es desconcertante.
    pub(super) fn relanzar_cerebro(&mut self, aviso: &str) {
        self.relanzar(aviso, Hilo::Nuevo);
    }

    /// Relanza el proceso **sin** perder la conversación.
    ///
    /// Para lo que cambia cómo piensa el modelo y no quién es: hoy, el
    /// esfuerzo. `claude` lo fija al arrancar, así que tocarlo obliga a matar
    /// el proceso —y con él se iba el hilo entero, que vive dentro—. Pulsar
    /// `Alt+L` a mitad de un trabajo te dejaba con un agente que no sabía de
    /// qué estabais hablando, justo cuando le pedías que se pusiera serio.
    ///
    /// Retomando la sesión el proceso es nuevo y la conversación no, así que
    /// las skills leídas siguen leídas: no hay que volver a contárselas.
    pub(super) fn relanzar_retomando(&mut self, aviso: &str) {
        self.relanzar(aviso, Hilo::Retomar);
    }

    fn relanzar(&mut self, aviso: &str, hilo: Hilo) {
        // Retomar una sesión que nunca tuvo un turno no es retomar nada: es
        // pedirle a `claude` una conversación que no existe, y contesta
        // muriéndose. Aquí y no solo en quien llama, que el error sería el
        // mismo viniera de donde viniera.
        let hilo = hilo.realista(self.sesion_estrenada);
        if let Some(b) = self.brain.as_mut() {
            // `shutdown` cierra una tubería y espera hasta 400 ms a que el
            // proceso muera: entrada y salida y temporizador de tokio. Con
            // `futures::executor::block_on` el reactor no corre mientras
            // esperamos, así que la espera tiene que ser del runtime.
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(b.shutdown())
            });
        }
        self.generacion += 1;
        if hilo == Hilo::Nuevo {
            // Cerebro nuevo, conversación nueva: lo que ya no recuerda, tampoco
            // hay que darlo por dicho. Y sesión nueva, porque un `--session-id`
            // ya usado sería justo lo contrario de empezar de cero.
            self.cfg.sesion = crate::ai::sesion_nueva();
            // Sesión nueva es sesión sin estrenar: hasta que no viaje un turno
            // no hay nada que `claude` pueda retomar.
            self.sesion_estrenada = false;
            self.skills_leidas.clear();
            // Un cerebro que empieza de cero no ha visto ningún comando, así
            // que el registro reciente le vuelve a hacer falta entero.
            // Retomando no: ahí la conversación sigue y lo mandado sigue dicho.
            self.ctx_enviado = 0;
            // Y tampoco le han explicado cómo se trabaja aquí: el recordatorio
            // corto da por sabido lo que este ya no sabe.
            self.modo_explicado = None;
            // El plan vive en la conversación, así que se va con ella. Dejarlo
            // puesto sería una barra diciendo «plan 3/7» sobre un cerebro que
            // no ha oído hablar de ningún plan — y un Alt+↵ que mandaría a
            // ejecutar unos pasos a alguien que no sabe de dónde salen.
            // Retomando no se toca: ahí la conversación sigue siendo la misma.
            self.descartar_plan();
        }
        match Brain::spawn(&self.cfg, self.ai_tx.clone(), self.generacion, hilo) {
            Ok(b) => {
                self.brain = Some(b);
                self.auto.reiniciar();
                self.thinking = false;
                self.push(Role::System, aviso.to_string());
            }
            Err(e) => {
                self.brain = None;
                // Con qué modelo ha fallado es lo primero que hace falta saber
                // cuando lo que se acaba de tocar es justamente el modelo.
                let cual = match self.cfg.model.as_deref() {
                    Some(m) => format!("«{m}»"),
                    None => "el agente".into(),
                };
                self.push(Role::System, format!("No pude arrancar {cual}: {e}"));
            }
        }
    }
    /// Cambia de proveedor si el nombre es uno declarado; si no, de modelo.
    ///
    /// Un solo comando para las dos cosas a propósito: desde fuera, «cambiar a
    /// groq» y «cambiar a opus» son la misma intención —quiero que me conteste
    /// otro—, y obligar a recordar cuál de los dos es cada uno solo añade una
    /// forma de equivocarse.
    pub(super) fn cambiar_a(&mut self, nombre: &str) {
        let elegido = self
            .catalogo
            .proveedor
            .iter()
            .find(|p| p.id.eq_ignore_ascii_case(nombre))
            .cloned();

        let Some(p) = elegido else {
            // ¿Es uno de los modelos declarados? Entonces se cambia también de
            // servidor, exactamente como al elegirlo en la ventana.
            //
            // Sin esto, `/modelo gpt-oss-20b` escribía ese nombre en
            // `cfg.model` —el campo de los alias de claude— y el proveedor
            // seguía siendo el que estuviera: el menú ofrecía un modelo que al
            // elegirlo no cambiaba de modelo. Ofrecerlo en la lista y que la
            // lista no funcione es peor que no ofrecerlo.
            if let Some((servidor, id)) = self.donde_esta(nombre) {
                self.cambiar_a_modelo(&servidor, &id);
                return;
            }
            // Ni servidor ni modelo declarado: se le pasa tal cual al que ya
            // está, que es lo que permite probar un modelo recién salido sin
            // tener que escribirlo antes en el fichero.
            self.cambiar_modelo(nombre.to_string());
            return;
        };

        if p.es_daemon() {
            self.cfg.proveedor = None;
            self.relanzar_cerebro(&format!("Ahora pregunto a «{}».", p.id));
            return;
        }

        let prov = p.a_proveedor(&self.entorno);
        if prov.api_key.is_empty() && !prov.base_url.contains("localhost") {
            self.push(
                Role::System,
                format!(
                    "«{}» no tiene clave. La busco en {}; declárala ahí o expórtala.",
                    p.id,
                    self.catalogo.env_file.as_deref().unwrap_or("el entorno")
                ),
            );
            return;
        }

        let aviso = format!("Ahora pregunto a «{}» con {}.", prov.id, prov.modelo);
        self.cfg.proveedor = Some(prov);
        self.relanzar_cerebro(&aviso);
    }
    /// De qué servidor es un modelo, por su nombre corto o por su id entero.
    ///
    /// Se admiten los dos porque el menú enseña el corto —`gpt-oss-120b`, sin
    /// el prefijo del fabricante, que lo necesita la API y no la persona— y el
    /// fichero declara el entero. Escribir cualquiera de los dos tiene que
    /// llevar al mismo sitio.
    ///
    /// Con el nombre repetido en dos servidores gana el primero del fichero,
    /// que es el orden que ha elegido el usuario. Para desempatar a mano está
    /// la ventana, que los enseña agrupados.
    fn donde_esta(&self, nombre: &str) -> Option<(String, String)> {
        let n = nombre.trim();
        self.catalogo.proveedor.iter().find_map(|p| {
            p.a_elegir()
                .into_iter()
                .find(|m| m.id.eq_ignore_ascii_case(n) || m.corto().eq_ignore_ascii_case(n))
                .map(|m| (p.id.clone(), m.id))
        })
    }

    /// Relanza el cerebro con otro modelo, sin cerrar vsioncode.
    ///
    /// El daemon lleva la conversación dentro, así que cambiar de modelo
    /// implica empezar un hilo nuevo. Se avisa: perder el contexto por
    /// sorpresa es peor que cambiar de modelo a mano.
    fn cambiar_modelo(&mut self, modelo: String) {
        let anterior = self.model.clone();
        // Se guarda antes de relanzar porque `Brain::spawn` lee de `cfg` a
        // quién hay que preguntarle.
        self.cfg.model = Some(modelo.clone());
        self.relanzar_cerebro(&format!(
            "Cambiado a «{modelo}» (antes {anterior}). Empieza una conversación nueva; el \
             registro de comandos se conserva."
        ));
    }
}
