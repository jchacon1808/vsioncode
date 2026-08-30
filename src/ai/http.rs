//! El cerebro que habla por HTTP.
//!
//! Un solo cliente para todos los proveedores que hablan el dialecto de
//! OpenAI: Groq, NVIDIA, Gemini, OpenRouter, Cerebras, Ollama y compañía.
//! Cambia la URL y la clave; el resto es idéntico.
//!
//! ## Por qué esto es más fácil de lo que parece
//!
//! Porque vsioncode **no usa llamada a herramientas para proponer comandos**:
//! saca los bloques ` ```bash ` del texto. Y ahí es justo donde los
//! proveedores se diferencian entre sí y donde los modelos pequeños fallan.
//! Al no depender de eso, a un proveedor solo se le exige seguir
//! instrucciones de formato — un listón que cumple casi cualquiera.
//!
//! ## Lo que sí cambia respecto al daemon
//!
//! El daemon de `claude` guarda la conversación dentro del proceso. Esto es
//! **sin estado**: hay que reenviar el historial entero en cada turno. Cuesta
//! tokens de verdad y crece con la conversación, así que se recorta y se
//! enseña en `/coste`. No es un detalle de implementación: es dinero.
//!
//! ## El pensamiento de los modelos que razonan
//!
//! Hay dos costumbres y ninguna es opcional de manejar:
//!
//! - Unos lo mandan en un campo aparte (`reasoning`), y basta con ignorarlo.
//! - Otros lo escriben **dentro** del contenido, entre `<think>` y
//!   `</think>`. Sin filtrarlo, el panel enseñaría el monólogo interno del
//!   modelo en vez de su respuesta.
//!
//! Y como llega por trozos, las etiquetas se parten entre chunks. De ahí que
//! el filtro sea una pequeña máquina de estados y no un `replace`.

use std::time::Instant;

use color_eyre::eyre::{Result, eyre};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{AiEvent, AiMsg, BrainConfig};

/// Un proveedor que habla el dialecto de OpenAI.
#[derive(Debug, Clone)]
pub struct Proveedor {
    /// Cómo lo llamas tú: «groq», «nvidia», «gemini»…
    pub id: String,
    /// Sin barra final.
    pub base_url: String,
    pub api_key: String,
    pub modelo: String,
    /// Cuánto se le deja pensar antes de escribir, si el modelo lo admite.
    ///
    /// Aquí es de las cosas que más se notan. Casi todos los modelos nuevos
    /// razonan **por defecto**, y en una terminal eso son segundos de pantalla
    /// quieta antes de la primera palabra: `gemini-3.6-flash` tardaba 3,7 s en
    /// arrancar solo por venir con `medium` puesto de fábrica.
    ///
    /// No se traduce ni se normaliza: va tal cual al campo `reasoning_effort`.
    /// Cada proveedor admite unos valores distintos —`none`, `minimal`, `low`,
    /// `medium`, `high`— y unificarlos aquí sería inventar una equivalencia
    /// que no existe y mandar valores que el servidor rechaza.
    pub esfuerzo: Option<String>,
    /// El otro interruptor de razonamiento, el de la familia Nemotron.
    ///
    /// NVIDIA no usa `reasoning_effort` en sus Nemotron sino
    /// `chat_template_kwargs: {"enable_thinking": false}`. Son dos maneras de
    /// pedir lo mismo y ningún proveedor entiende la del otro, así que están
    /// las dos y cada modelo declara la suya.
    pub pensar: Option<bool>,
}

/// Un turno de la conversación, tal y como se le manda al proveedor.
#[derive(Debug, Clone, serde::Serialize)]
struct Turno {
    role: &'static str,
    content: String,
    /// Imágenes que acompañan a este turno, como `data:` URI.
    ///
    /// Van aparte del texto y no dentro porque el dialecto de OpenAI las
    /// quiere en bloques de contenido, no en la cadena. Se guardan aquí para
    /// que sigan viajando en los turnos siguientes: quitarlas del historial
    /// haría que el modelo perdiera de vista la captura en cuanto le
    /// preguntaras una segunda cosa sobre ella.
    #[serde(skip)]
    imagenes: Vec<String>,
}

impl Turno {
    /// El turno tal y como lo espera la API.
    ///
    /// Sin imágenes, `content` es una cadena — que es lo que entiende todo el
    /// mundo, incluidos los proveedores que no saben de bloques. Con
    /// imágenes, pasa a ser una lista de bloques, que es la única forma de
    /// mandar píxeles por este dialecto.
    fn a_json(&self) -> Value {
        if self.imagenes.is_empty() {
            return json!({ "role": self.role, "content": self.content });
        }
        let mut bloques = vec![json!({ "type": "text", "text": self.content })];
        for uri in &self.imagenes {
            bloques.push(json!({
                "type": "image_url",
                "image_url": { "url": uri },
            }));
        }
        json!({ "role": self.role, "content": bloques })
    }
}

/// Cuánta conversación se reenvía.
///
/// Al ser sin estado, el historial entero viaja en cada turno: sin tope, una
/// charla larga acaba costando más en reenviar lo dicho que en decir lo nuevo.
/// Se conservan el mensaje de sistema y los últimos intercambios.
const MAX_TURNOS: usize = 24;

pub struct HttpBrain {
    proveedor: Proveedor,
    /// Los sistemas Compound de Groq pueden buscar y leer páginas en remoto.
    /// Se guarda aquí porque la configuración ya no cambia durante la vida de
    /// este cerebro; `/web` lo relanza para aplicar el valor nuevo.
    web: bool,
    compound: bool,
    gpt_oss: bool,
    cliente: reqwest::Client,
    historia: Vec<Turno>,
    tx: mpsc::UnboundedSender<AiMsg>,
    generacion: u64,
    pending: usize,
}

impl HttpBrain {
    pub fn arrancar(
        proveedor: Proveedor,
        cfg: &BrainConfig,
        tx: mpsc::UnboundedSender<AiMsg>,
        generacion: u64,
    ) -> Result<Self> {
        if proveedor.api_key.trim().is_empty() && !proveedor.base_url.contains("localhost") {
            return Err(eyre!("«{}» no tiene clave configurada", proveedor.id));
        }

        // El proveedor criptográfico se instala aquí y no solo en `main`.
        //
        // `reqwest` entra en **pánico** al crear un cliente si no hay ninguno
        // instalado, así que dejar esto en manos de quien nos llame convierte
        // un olvido en una caída. Instalarlo dos veces es inofensivo: la
        // segunda devuelve error y se ignora.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let cliente = reqwest::Client::builder()
            // Un turno puede tardar en empezar si el proveedor está saturado,
            // pero no puede quedarse colgado para siempre.
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| eyre!("no pude preparar el cliente: {e}"))?;

        let compound = super::es_groq_compound(&proveedor);
        let gpt_oss = super::es_groq_gpt_oss(&proveedor);
        let web = cfg.web && (compound || gpt_oss);
        let historia = vec![Turno {
            role: "system",
            content: super::sistema(cfg, web),
            imagenes: Vec::new(),
        }];

        // El backend HTTP está listo en cuanto se construye: no hay proceso
        // que esperar. Se avisa igual, para que la interfaz enseñe el modelo.
        let _ = tx.send((
            generacion,
            AiEvent::Ready {
                model: format!("{} · {}", proveedor.id, proveedor.modelo),
                // Compound ejecuta estas herramientas en los servidores de
                // Groq. Se declaran con los mismos nombres semánticos que el
                // daemon para que la interfaz mantenga una sola política.
                tools: if web {
                    vec!["WebSearch".into(), "WebFetch".into()]
                } else {
                    Vec::new()
                },
            },
        ));

        Ok(Self {
            proveedor,
            web,
            compound,
            gpt_oss,
            cliente,
            historia,
            tx,
            generacion,
            pending: 0,
        })
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    pub fn turn_finished(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    pub async fn ask(&mut self, texto: &str) -> Result<()> {
        self.ask_con(texto, Vec::new()).await
    }

    /// Un turno con imágenes adjuntas.
    pub async fn ask_con(&mut self, texto: &str, imagenes: Vec<String>) -> Result<()> {
        self.historia.push(Turno {
            role: "user",
            content: texto.to_string(),
            imagenes,
        });
        self.recortar();

        let mensajes: Vec<Value> = self.historia.iter().map(Turno::a_json).collect();
        let mut cuerpo = json!({
            "model": self.proveedor.modelo,
            "messages": mensajes,
            "stream": true,
        });
        // Los dos dialectos del «no pienses tanto». Solo se mandan si el
        // modelo los declara: enviar un `reasoning_effort` a quien no lo
        // entiende es un 400 y un turno perdido.
        if let Some(e) = &self.proveedor.esfuerzo {
            cuerpo["reasoning_effort"] = json!(e);
        }
        if let Some(p) = self.proveedor.pensar {
            cuerpo["chat_template_kwargs"] = json!({ "enable_thinking": p });
        }
        if self.compound {
            // Compound trae también ejecución de código y Wolfram por
            // defecto. Aquí se le da únicamente lectura de la web, para que
            // `/web` no abra ninguna capacidad que el usuario no pidió.
            let herramientas: Vec<&str> = if self.web {
                vec!["web_search", "visit_website"]
            } else {
                Vec::new()
            };
            cuerpo["compound_custom"] = json!({
                "tools": { "enabled_tools": herramientas }
            });
        }
        if self.gpt_oss {
            // GPT-OSS ofrece también un intérprete Python remoto. Solo se
            // declara el navegador: activar web nunca concede ejecución.
            if self.web {
                cuerpo["tools"] = json!([{ "type": "browser_search" }]);
                cuerpo["tool_choice"] = json!("auto");
            } else {
                // Estos modelos pueden traer herramientas nativas por
                // defecto. `none` es la forma explícita de que `/web` las
                // cierre sin cambiar de modelo.
                cuerpo["tool_choice"] = json!("none");
            }
        }

        let peticion = self
            .cliente
            .post(format!("{}/chat/completions", self.proveedor.base_url))
            .bearer_auth(&self.proveedor.api_key)
            .json(&cuerpo);

        self.pending += 1;
        let (tx, generacion, web) = (self.tx.clone(), self.generacion, self.web);

        tokio::spawn(async move {
            let empezado = Instant::now();
            let _ = tx.send((generacion, AiEvent::TurnStart));
            if web {
                // Compound devuelve qué herramientas ejecutó solo en su
                // respuesta final, no en los deltas SSE. Hasta que el cliente
                // lea también ese formato, el turno entero se considera web:
                // nunca se ejecuta solo un comando posiblemente influido por
                // una página externa.
                let _ = tx.send((generacion, AiEvent::WebRemota));
            }

            let respuesta = match peticion.send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send((generacion, AiEvent::Error(format!("no llegué: {e}"))));
                    let _ = tx.send((
                        generacion,
                        AiEvent::TurnEnd {
                            cost_usd: 0.0,
                            ms: 0,
                            uso: Default::default(),
                        },
                    ));
                    return;
                }
            };

            if !respuesta.status().is_success() {
                let codigo = respuesta.status();
                let detalle = respuesta.text().await.unwrap_or_default();
                let _ = tx.send((generacion, AiEvent::Error(explicar(codigo, &detalle))));
                let _ = tx.send((
                    generacion,
                    AiEvent::TurnEnd {
                        cost_usd: 0.0,
                        ms: empezado.elapsed().as_millis() as u64,
                        uso: Default::default(),
                    },
                ));
                return;
            }

            let mut flujo = respuesta.bytes_stream().eventsource();
            let mut filtro = FiltroPensamiento::default();
            let mut completa = String::new();

            while let Some(Ok(ev)) = flujo.next().await {
                if ev.data.trim() == "[DONE]" {
                    break;
                }
                let Ok(v) = serde_json::from_str::<Value>(&ev.data) else {
                    continue;
                };
                let delta = &v["choices"][0]["delta"];

                // El razonamiento en campo aparte no se pinta, pero sí anima
                // el indicador: si no, parece que no está haciendo nada.
                if delta.get("reasoning").and_then(Value::as_str).is_some() {
                    let _ = tx.send((generacion, AiEvent::Thinking));
                }

                let Some(trozo) = delta["content"].as_str() else {
                    continue;
                };
                let (visible, pensando) = filtro.pasar(trozo);
                if pensando {
                    let _ = tx.send((generacion, AiEvent::Thinking));
                }
                if !visible.is_empty() {
                    completa.push_str(&visible);
                    if tx.send((generacion, AiEvent::Text(visible))).is_err() {
                        return;
                    }
                }
            }

            let _ = tx.send((generacion, AiEvent::Respuesta(completa)));
            let _ = tx.send((
                generacion,
                AiEvent::TurnEnd {
                    // Los proveedores compatibles no devuelven coste. Se deja
                    // en cero antes que inventarlo: un número inventado en la
                    // barra es peor que no tener número. Con los tokens,
                    // igual: el `usage` del final no viaja en el flujo SSE.
                    cost_usd: 0.0,
                    ms: empezado.elapsed().as_millis() as u64,
                    uso: Default::default(),
                },
            ));
        });

        Ok(())
    }

    /// Guarda en el historial lo que respondió, para el turno siguiente.
    pub fn anotar_respuesta(&mut self, texto: String) {
        if !texto.trim().is_empty() {
            self.historia.push(Turno {
                role: "assistant",
                content: texto,
                imagenes: Vec::new(),
            });
            self.recortar();
        }
    }

    fn recortar(&mut self) {
        recortar(&mut self.historia);
    }

    pub async fn shutdown(&mut self) {
        // No hay proceso que matar: la petición en vuelo, si la hay, muere
        // sola cuando el receptor deje de escuchar.
    }
}

/// Deja el mensaje de sistema y los últimos turnos.
///
/// Función libre y no método porque es lógica pura: así se prueba sin montar
/// un cliente HTTP, que además exige tener instalado el proveedor
/// criptográfico y no viene a cuento para comprobar un recorte de lista.
fn recortar(historia: &mut Vec<Turno>) {
    if historia.len() <= MAX_TURNOS {
        return;
    }
    let sobran = historia.len() - MAX_TURNOS;
    // El primero es el mensaje de sistema y no se toca nunca: sin él, el
    // modelo deja de saber en qué está y empieza a contestar como un chat.
    historia.drain(1..=sobran);
}

/// Traduce un error HTTP a algo accionable.
fn explicar(codigo: reqwest::StatusCode, detalle: &str) -> String {
    let motivo = serde_json::from_str::<Value>(detalle)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .map(str::to_string)
                .or_else(|| v["error"].as_str().map(str::to_string))
        })
        .unwrap_or_else(|| detalle.chars().take(160).collect());

    match codigo.as_u16() {
        401 | 403 => format!("la clave no vale o no tiene acceso: {motivo}"),
        404 => format!("ese modelo no existe en este proveedor: {motivo}"),
        429 => format!("te has quedado sin cuota en este proveedor: {motivo}"),
        500..=599 => format!("el proveedor está caído ({codigo}): {motivo}"),
        _ => format!("error {codigo}: {motivo}"),
    }
}

/// Quita el pensamiento que algunos modelos escriben dentro de la respuesta.
///
/// Modelos como Qwen abren `<think>`, razonan durante párrafos y cierran
/// `</think>` antes de contestar. Sin filtrarlo, el panel enseñaría ese
/// monólogo en lugar de la respuesta.
///
/// Es una máquina de estados y no un `replace` porque el texto llega por
/// trozos: la etiqueta puede partirse entre dos chunks, y entonces buscar la
/// cadena completa no encuentra nada. Guarda lo que podría ser el principio de
/// una etiqueta y decide cuando llega el resto.
#[derive(Default)]
pub struct FiltroPensamiento {
    dentro: bool,
    /// Lo retenido por si es el principio de una etiqueta partida.
    pendiente: String,
}

const ABRE: &str = "<think>";
const CIERRA: &str = "</think>";

impl FiltroPensamiento {
    /// Devuelve lo que se puede pintar, y si estaba pensando.
    pub fn pasar(&mut self, trozo: &str) -> (String, bool) {
        self.pendiente.push_str(trozo);
        let mut visible = String::new();
        let mut penso = false;

        loop {
            if self.dentro {
                match self.pendiente.find(CIERRA) {
                    Some(i) => {
                        penso = true;
                        self.pendiente = self.pendiente[i + CIERRA.len()..].to_string();
                        self.dentro = false;
                    }
                    None => {
                        // Se conserva solo lo que podría ser una etiqueta a
                        // medias; lo demás es pensamiento y se tira.
                        penso = true;
                        self.pendiente = cola_parcial(&self.pendiente, CIERRA);
                        break;
                    }
                }
            } else {
                match self.pendiente.find(ABRE) {
                    Some(i) => {
                        visible.push_str(&self.pendiente[..i]);
                        self.pendiente = self.pendiente[i + ABRE.len()..].to_string();
                        self.dentro = true;
                    }
                    None => {
                        // Todo es visible menos lo que pueda ser el principio
                        // de un `<think>` que aún no ha llegado entero.
                        let cola = cola_parcial(&self.pendiente, ABRE);
                        let corte = self.pendiente.len() - cola.len();
                        visible.push_str(&self.pendiente[..corte]);
                        self.pendiente = cola;
                        break;
                    }
                }
            }
        }
        (visible, penso)
    }
}

/// El sufijo del texto que podría ser el principio de `marca`.
///
/// Ojo con cortar por bytes: en «qué tal» los últimos tres bytes caen dentro
/// de la «é» y Rust entra en pánico. Hay que saltarse las posiciones que no
/// son frontera de carácter — y en español eso pasa constantemente.
fn cola_parcial(texto: &str, marca: &str) -> String {
    let max = marca.len().min(texto.len());
    for n in (1..=max).rev() {
        let inicio = texto.len() - n;
        if !texto.is_char_boundary(inicio) {
            continue;
        }
        let cola = &texto[inicio..];
        if marca.starts_with(cola) {
            return cola.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filtrar(trozos: &[&str]) -> String {
        let mut f = FiltroPensamiento::default();
        trozos.iter().map(|t| f.pasar(t).0).collect()
    }

    #[test]
    fn el_texto_normal_pasa_entero() {
        assert_eq!(filtrar(&["hola ", "qué tal"]), "hola qué tal");
    }

    /// El caso medido con `qwen3.6-27b`: escribe párrafos de razonamiento
    /// dentro de la respuesta. Enseñarlos sería enseñar el monólogo interno
    /// del modelo en vez de lo que quiso decir.
    #[test]
    fn quita_el_pensamiento() {
        assert_eq!(
            filtrar(&["<think>a ver, el usuario pide…</think>Muestra los sockets."]),
            "Muestra los sockets."
        );
    }

    /// Lo que obliga a que esto sea una máquina de estados: al llegar por
    /// trozos, la etiqueta se parte y buscar la cadena entera no encuentra
    /// nada.
    #[test]
    fn aguanta_la_etiqueta_partida_entre_trozos() {
        assert_eq!(
            filtrar(&["<th", "ink>pensando</thi", "nk>respuesta"]),
            "respuesta"
        );
        assert_eq!(filtrar(&["<", "think>", "x", "</", "think>", "ok"]), "ok");
    }

    #[test]
    fn el_texto_de_antes_y_de_despues_se_conserva() {
        assert_eq!(
            filtrar(&["antes <think>medio</think> después"]),
            "antes  después"
        );
    }

    /// Un `<` suelto no puede tragarse el resto de la respuesta.
    #[test]
    fn un_menor_que_suelto_no_rompe_nada() {
        assert_eq!(filtrar(&["si a < b entonces"]), "si a < b entonces");
        assert_eq!(filtrar(&["usa <div> y <span>"]), "usa <div> y <span>");
    }

    #[test]
    fn avisa_de_que_esta_pensando() {
        let mut f = FiltroPensamiento::default();
        assert!(!f.pasar("hola").1);
        assert!(f.pasar("<think>mmm").1);
    }

    /// El texto que llega es español y viene por trozos: cortar por bytes sin
    /// mirar dónde empieza cada carácter revienta con cualquier acento.
    #[test]
    fn los_acentos_no_lo_hacen_caer() {
        assert_eq!(filtrar(&["hola qué tal"]), "hola qué tal");
        assert_eq!(filtrar(&["año ", "ñu ", "corazón"]), "año ñu corazón");
        assert_eq!(
            filtrar(&["<think>ñ</think>Está en la línea 42"]),
            "Está en la línea 42"
        );
        // Y partido justo por dentro de un carácter multibyte.
        let s = "qué";
        assert_eq!(filtrar(&[&s[..2], &s[2..]]), "qué");
    }

    #[test]
    fn la_cola_parcial_reconoce_un_prefijo() {
        assert_eq!(cola_parcial("texto <th", "<think>"), "<th");
        assert_eq!(cola_parcial("texto <", "<think>"), "<");
        assert_eq!(cola_parcial("texto normal", "<think>"), "");
    }

    #[test]
    fn los_errores_se_explican_en_castellano() {
        use reqwest::StatusCode;
        let json = r#"{"error":{"message":"model not found"}}"#;
        assert!(explicar(StatusCode::NOT_FOUND, json).contains("no existe"));
        assert!(explicar(StatusCode::TOO_MANY_REQUESTS, json).contains("cuota"));
        assert!(explicar(StatusCode::UNAUTHORIZED, json).contains("clave"));
        // Y el motivo del proveedor se conserva, que es lo accionable.
        assert!(explicar(StatusCode::NOT_FOUND, json).contains("model not found"));
    }

    #[test]
    fn el_historial_no_crece_sin_fin_y_conserva_el_sistema() {
        let mut h = vec![Turno {
            role: "system",
            content: "eres vsioncode".into(),
            imagenes: Vec::new(),
        }];
        for i in 0..40 {
            h.push(Turno {
                role: "user",
                content: format!("mensaje {i}"),
                imagenes: Vec::new(),
            });
        }
        recortar(&mut h);
        assert_eq!(h.len(), MAX_TURNOS);
        assert_eq!(h[0].role, "system", "el sistema no se tira nunca");
        assert!(
            h.last().unwrap().content.contains("39"),
            "y se conserva lo más reciente"
        );
    }

    #[test]
    fn un_historial_corto_no_se_toca() {
        let mut h = vec![
            Turno {
                role: "system",
                content: "s".into(),
                imagenes: Vec::new(),
            },
            Turno {
                role: "user",
                content: "hola".into(),
                imagenes: Vec::new(),
            },
        ];
        recortar(&mut h);
        assert_eq!(h.len(), 2);
    }
}
