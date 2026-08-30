//! Subagentes: repartir encargos y juntar lo que traen.
//!
//! Submódulo de `app` para seguir viendo lo privado de [`App`] sin abrir sus
//! campos al resto del programa.

use super::{App, Role, aviso_subagentes, plural};
use crate::agente::{self, Encargo, Motor, Posible, Resultado};
use crate::slash;

impl App {
    /// Reparte trabajo entre subagentes.
    ///
    /// Se lanzan de verdad en paralelo y se espera a todos: al agente
    /// principal le vuelve un solo mensaje con lo que averiguó cada uno. Ir
    /// devolviéndoselos de uno en uno lo obligaría a razonar con la mitad de
    /// la información y a menudo a pedir el resto otra vez.
    pub(super) fn repartir(&mut self, encargos: Vec<Encargo>) {
        // Sin ningún modelo con el que trabajar no hay reparto que empezar. Se
        // dice aquí y no al lanzar cada uno: cuatro veces el mismo aviso no
        // explica mejor que una, y encolarlos sería prometer que se harán
        // después.
        if self.motor_de(None).is_none() {
            self.push(
                Role::System,
                "No puedo repartir: no hay ningún modelo configurado con el que lanzar \
                 un subagente. Añade un proveedor con /configurar."
                    .into(),
            );
            return;
        }

        // Se pregunta una vez y se usa el mismo número en el reparto y en lo
        // que se cuenta: el cupo se calcula con la memoria libre, y volver a
        // pedirlo tres líneas después podría dar otro y contradecirse.
        let cupo = agente::max_a_la_vez();
        let sitio = cupo.saturating_sub(self.subagentes.len());
        if sitio == 0 {
            // El mismo fallo que el de abajo, en la otra rama: se anunciaba
            // «espero a que acaben» y no esperaba nadie, porque los encargos
            // se tiraban aquí mismo. Y como `thinking` ya se apagó antes de
            // llamar, el turno se quedaba muerto sin nada que lo reanudara.
            let n = encargos.len();
            self.sub_cola.extend(encargos);
            self.push(
                Role::System,
                format!(
                    "Ya hay subagentes trabajando; {} esperando turno.",
                    plural(n, "encargo", "encargos")
                ),
            );
            return;
        }

        let (van, esperan) = encargos.split_at(encargos.len().min(sitio));

        // Los que no caben se guardan, no se tiran.
        //
        // Antes se decía «los otros tendrán que ir después» y no iban nunca:
        // se descartaban en silencio. El agente creía haber repartido cuatro
        // encargos y solo volvían dos, sin que nadie dijera nada.
        if !esperan.is_empty() {
            self.sub_cola.extend_from_slice(esperan);
            self.push(
                Role::System,
                format!(
                    "Lanzo {}; {} esperando turno (caben {} a la vez con la memoria \
                     libre que hay ahora).",
                    plural(van.len(), "subagente", "subagentes"),
                    esperan.len(),
                    cupo
                ),
            );
        }

        let binario = self.cfg.binary.clone();
        let cwd = self.ledger.cwd().to_string();

        for e in van {
            // El nombre se resuelve **antes** de contar el subagente como
            // lanzado: quedarse con un pendiente que nunca arranca deja el
            // turno esperando a algo que no va a volver.
            let Some((nombre, motor)) = self.motor_de(e.modelo.as_deref()) else {
                continue;
            };
            // El reloj arranca con el primero y no con cada uno: lo que la
            // barra cuenta es cuánto lleva esperando el turno, no cuánto lleva
            // el último en salir.
            self.sub_desde.get_or_insert_with(std::time::Instant::now);
            self.subagentes.push(crate::app::Subagente {
                titulo: e.titulo.clone(),
                modelo: nombre.clone(),
                desde: std::time::Instant::now(),
            });
            self.sub_pendientes += 1;
            // Con qué modelo va, dicho en la conversación y no solo por dentro.
            // Es lo único que enseña que un opus arriba no está lanzando cuatro
            // opus abajo, y sin verlo nadie se fiaría de que el ahorro ocurre.
            // Con el nombre pedido y no con el del motor: si escribiste
            // «qwen3.6-27b» quieres leer eso, no la URL de Groq.
            self.push(Role::System, format!("subagente ({nombre}): {}", e.titulo));

            let (b, c, enc, tx) = (binario.clone(), cwd.clone(), e.clone(), self.sub_tx.clone());
            tokio::spawn(async move {
                let r = agente::lanzar(&motor, &b, &c, &enc)
                    .await
                    .unwrap_or_else(|err| Resultado {
                        titulo: enc.titulo.clone(),
                        texto: format!("no pude hacerlo: {err}"),
                        coste_usd: 0.0,
                        ok: false,
                    });
                let _ = tx.send(r);
            });
        }
        self.shell.mark_dirty();
    }

    /// Lo que espera turno, por su título.
    ///
    /// La barra los enseña detrás de los que están trabajando. Sin esto, pedir
    /// tres y ver dos parece que uno se ha perdido: el mensaje que lo explica
    /// sube con la conversación y a los diez segundos ya no está a la vista.
    pub fn en_cola(&self) -> Vec<&str> {
        self.sub_cola.iter().map(|e| e.titulo.as_str()).collect()
    }

    /// El techo de gama de los subagentes, que es también su defecto.
    ///
    /// Manda lo que haya fijado el usuario con `/agente modelo`; si no ha
    /// fijado nada, la regla de [`agente::modelo_por_defecto`]. Es un solo
    /// número para dos cosas a propósito: el defecto y el límite tienen que
    /// salir del mismo sitio, o acabarían contándose distinto en el reparto y
    /// en lo que se le enseña al usuario.
    ///
    /// Y si esa regla nombra algo que aquí no existe —no hay proceso `claude`,
    /// o el fichero no declara ninguno—, manda el primero de los que sí hay.
    /// Un defecto que no se puede lanzar deja `/agente` roto sin decir por qué.
    pub(super) fn techo_subagente(&self) -> String {
        if let Some(m) = &self.sub_modelo {
            return m.clone();
        }
        let regla = agente::modelo_por_defecto(self.cfg.model.as_deref());
        if self.resolver_subagente(&regla).is_some() {
            return regla;
        }
        self.subagentes_posibles()
            .into_iter()
            .next()
            .map(|p| p.nombre)
            .unwrap_or(regla)
    }

    /// Todo lo que puede llevar un subagente en esta máquina.
    ///
    /// La lista la arma [`agente::posibles`] con el fichero del usuario
    /// entero. Aquí solo se le dan las tres cosas que hacen falta para
    /// saberlo: qué hay declarado, de dónde salen las claves y con qué binario
    /// se lanzaría un `claude`.
    pub(super) fn subagentes_posibles(&self) -> Vec<Posible> {
        agente::posibles(&self.catalogo, &self.entorno, &self.cfg.binary)
    }

    /// De un nombre escrito a mano al de la lista y al motor que lo lanza.
    pub(super) fn resolver_subagente(&self, nombre: &str) -> Option<(String, Motor)> {
        agente::resolver(&self.catalogo, &self.entorno, &self.cfg.binary, nombre)
    }

    /// Con qué puede ir un subagente, para el menú.
    pub(super) fn subagentes_del_menu(&self) -> Vec<(String, String)> {
        self.subagentes_posibles()
            .into_iter()
            .map(|p| (p.nombre, p.detalle))
            .collect()
    }

    /// Lo mismo, agrupado por servidor, para decirlo en una frase.
    ///
    /// Con veinte modelos configurados, enumerarlos separados por comas era
    /// una línea ilegible. Agrupados se lee de un vistazo qué sirve cada uno,
    /// que es la pregunta que se hace quien lee ese mensaje.
    pub(super) fn subagentes_por_proveedor(&self) -> String {
        let mut v: Vec<(String, Vec<String>)> = Vec::new();
        for p in self.subagentes_posibles() {
            match v.iter_mut().find(|(id, _)| *id == p.proveedor) {
                Some((_, nombres)) => nombres.push(p.nombre),
                None => v.push((p.proveedor, vec![p.nombre])),
            }
        }
        v.into_iter()
            .map(|(id, nombres)| format!("{id}: {}", nombres.join(", ")))
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// Qué motor le toca a un encargo, y con qué nombre se cuenta.
    ///
    /// Un nombre con errata no tumba el reparto: se cae en el defecto, igual
    /// que cuando nadie ha pedido nada. Devuelve `None` solo cuando no hay
    /// **ningún** modelo con el que trabajar, que es otra cosa y se dice
    /// aparte.
    fn motor_de(&self, pedido: Option<&str>) -> Option<(String, Motor)> {
        pedido
            .and_then(|n| self.resolver_subagente(n))
            .or_else(|| self.resolver_subagente(&self.techo_subagente()))
    }

    /// Lanza un subagente porque lo ha pedido el usuario, no el cerebro.
    ///
    /// No pasa por [`agente::bajo_techo`]: si escribe `/agente opus`, va con
    /// opus. El techo existe para que el modelo no decida el gasto de otro, y
    /// aquí el que decide es quien paga.
    ///
    /// Lo que traiga vuelve por el mismo camino que el resto —a la conversación
    /// y al cerebro—, que es lo que convierte esto en delegar y no en preguntar
    /// aparte: si el informe no llegara arriba, la respuesta siguiente del
    /// cerebro no lo tendría y habría que pegárselo a mano.
    pub(super) fn encargar(&mut self, tarea: String, modelo: Option<String>) {
        let titulo = titulo_de(&tarea);
        self.push(Role::User, format!("Encargo a un subagente: {tarea}"));
        self.repartir(vec![Encargo {
            titulo,
            tarea,
            modelo,
        }]);
    }

    /// Atiende `/agente`: delegar una tarea, o decidir con qué modelo se delega.
    ///
    /// Delegar deja de ser cosa solo del cerebro. Hay trabajo que uno sabe de
    /// antemano que es leer mucho y contar poco —«dime qué hace este módulo»—,
    /// y hacerlo pasar por el modelo principal para que lo reparta cuesta un
    /// turno de ida y otro de vuelta del modelo más caro de la casa, justo para
    /// decidir algo que ya estaba decidido.
    pub(super) fn slash_agente(&mut self, a: slash::Agente) {
        match a {
            // `/agente` a secas abre la barra en vez de contar con qué van.
            //
            // El mensaje que había decía el nombre del agente y enumeraba los
            // que hay — que es exactamente lo que enseña la barra, con la
            // diferencia de que allí además se elige y se escribe el encargo.
            // Leer una lista para después escribir otro comando eran dos pasos
            // para una decisión.
            slash::Agente::Como => self.abrir_delegar(),
            slash::Agente::Fijar(m) => {
                let Some((nombre, _)) = self.resolver_subagente(&m) else {
                    let hay = self.subagentes_por_proveedor();
                    self.push(
                        Role::System,
                        if hay.is_empty() {
                            "No hay ningún modelo configurado con el que lanzar un \
                             subagente. Añade un proveedor con /configurar."
                                .to_string()
                        } else {
                            format!("No conozco el modelo «{m}». Los que hay: {hay}.")
                        },
                    );
                    return;
                };
                // El techo se explica distinto según de dónde sea. La escalera
                // de gamas es la de claude: fijar uno de otro proveedor no
                // «baja» nada, cambia de cuenta.
                let cola = if crate::agente::reconocer(&nombre).is_some() {
                    "Es también el techo: si el agente pide uno más caro para un encargo \
                     suyo, se le baja a este."
                } else {
                    "No es de la suscripción, así que ese trabajo no gasta cuota de claude: \
                     si el agente pide uno de claude para un encargo suyo, se le cambia a este."
                };
                self.sub_modelo = Some(nombre.clone());
                self.push(
                    Role::System,
                    format!("Los subagentes van con «{nombre}». {cola}"),
                );
            }
            slash::Agente::Lanzar { modelo, tarea } => {
                // Un modelo escrito a mano puede venir con errata, y ahí sí hay
                // que decirlo: a diferencia del bloque `agente(…)`, aquí no hay
                // nada que caiga de vuelta a un defecto sensato sin que parezca
                // que se ha ignorado lo que se pidió.
                let modelo = match modelo {
                    Some(m) => match self.resolver_subagente(&m) {
                        Some((n, _)) => Some(n),
                        None => {
                            let hay = self.subagentes_por_proveedor();
                            self.push(
                                Role::System,
                                format!("No conozco el modelo «{m}». Los que hay: {hay}."),
                            );
                            return;
                        }
                    },
                    None => None,
                };
                self.encargar(tarea, modelo);
            }
        }
    }

    pub(super) fn on_subagente(&mut self, r: Resultado) {
        self.subagentes.retain(|s| s.titulo != r.titulo);
        self.last_cost += r.coste_usd;
        self.sub_pendientes = self.sub_pendientes.saturating_sub(1);
        self.sub_recogidos.push(r);
        if self.subagentes.is_empty() {
            self.sub_desde = None;
        }

        if self.sub_pendientes > 0 {
            self.shell.mark_dirty();
            return;
        }

        // Se ha liberado sitio: entran los que estaban esperando turno.
        if !self.sub_cola.is_empty() {
            let siguientes = std::mem::take(&mut self.sub_cola);
            self.repartir(siguientes);
            return;
        }

        // Ya están todos: se le devuelve al principal en un solo mensaje.
        let recogidos = std::mem::take(&mut self.sub_recogidos);
        let mut t = String::from("Han terminado los subagentes. Esto es lo que traen:\n\n");
        for r in &recogidos {
            t.push_str(&format!("### {}\n{}\n\n", r.titulo, r.texto));
        }
        t.push_str(
            "---\nSigue con la tarea usando esto. Si necesitas ejecutar algo, dalo en un \
             bloque bash; si necesitas repartir más trabajo, en bloques agente.",
        );
        self.push(Role::System, aviso_subagentes(recogidos.len()));
        self.send(t);
    }
}

/// Un título corto a partir de la tarea, para lo que lanza el usuario.
///
/// El modelo pone título a sus encargos; el usuario escribe una frase y no
/// tiene por qué inventarse dos nombres para lo mismo. Se corta por palabras y
/// no por bytes: partir un carácter multibyte por la mitad es un pánico, y aquí
/// se escribe en español.
fn titulo_de(tarea: &str) -> String {
    const ANCHO: usize = 40;
    let limpio = tarea.split_whitespace().collect::<Vec<_>>().join(" ");
    if limpio.chars().count() <= ANCHO {
        return limpio;
    }
    let corto: String = limpio.chars().take(ANCHO).collect();
    let corte = corto.rfind(' ').unwrap_or(corto.len());
    format!("{}…", corto[..corte].trim_end())
}

#[cfg(test)]
mod tests {
    use super::titulo_de;

    #[test]
    fn el_titulo_corto_se_queda_entero() {
        assert_eq!(titulo_de("  mirar  los tests "), "mirar los tests");
    }

    /// Con acentos, que es lo normal aquí: cortar por bytes daría un pánico.
    #[test]
    fn el_titulo_largo_se_corta_por_una_palabra() {
        let t =
            titulo_de("revisa la configuración del módulo de audio y dime qué invariantes asume");
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= 41, "{t}");
        assert!(!t.contains("  "));
    }
}
