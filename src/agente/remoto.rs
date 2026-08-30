//! Un subagente servido por HTTP: Groq, NVIDIA, Gemini, Ollama…
//!
//! El de `claude` es un proceso con sus herramientas dentro. Este no tiene
//! ninguna, así que se las damos nosotros: pide ficheros en texto, [`lector`]
//! se los da, y así hasta que responde. Es el mismo trato —solo lectura, y
//! solo dentro del directorio de trabajo—, con la diferencia de que aquí el
//! límite lo pone nuestro código y no la promesa de otro programa.
//!
//! ## Por qué esto tenía que existir
//!
//! Porque delegar estaba atado a la suscripción. Quien configuraba cinco
//! proveedores y veinte modelos podía hablar con cualquiera de ellos, pero al
//! repartir trabajo solo salían tres alias de `claude`: el resto de su
//! configuración no existía para `/agente`. Y un subagente es justo el sitio
//! donde un modelo rápido y barato rinde mejor —leer mucho y contar poco—, así
//! que la parte cara de la casa estaba haciendo el trabajo más tonto.
//!
//! ## Las rondas
//!
//! Cada vuelta es una llamada al proveedor, y se paga. Por eso hay tope: sin
//! él, un modelo que no encuentra lo que busca pide ficheros hasta agotar la
//! paciencia de nadie. Al llegar al final se le pide que responda con lo que
//! tenga, que es mejor informe que ninguno.

use color_eyre::eyre::{Result, eyre};
use serde_json::{Value, json};

use super::{Encargo, Resultado};
use crate::agente::lector::{Lector, MAX_PETICIONES, peticiones};
use crate::ai::http::Proveedor;

/// Cuántas veces puede pedir material antes de tener que responder.
///
/// Seis salieron de la forma de las tareas reales: listar, leer dos o tres
/// ficheros y comprobar un par de usos. Con menos se queda a medias en un
/// módulo grande; con más, el gasto crece sin que el informe mejore.
const MAX_RONDAS: usize = 6;

/// Cuánto material se le puede entregar en total, en bytes.
///
/// La conversación se reenvía entera en cada vuelta, así que lo leído en la
/// primera ronda se vuelve a pagar en la cuarta. Sin este tope, un subagente
/// que va pidiendo ficheros crece hasta chocar con el límite por minuto del
/// proveedor —medido: Groq corta en 8.000 tokens en su plan gratuito— y la
/// ronda que revienta se lleva por delante todo lo leído hasta entonces.
///
/// Cuando se acaba no se corta: se le dice que responda con lo que tenga. Un
/// informe incompleto que dice qué le faltó sirve; un error, no.
const PRESUPUESTO_BYTES: usize = 48 * 1024;

/// Lo que se le añade al encargo para que sepa pedir.
///
/// Va aparte de [`super::SISTEMA`] porque es lo único que cambia entre los dos
/// tipos de subagente: uno tiene herramientas de verdad y este las pide por
/// escrito. El resto del trato —hallazgos y no diario, breve, solo lectura— es
/// el mismo y se escribe una sola vez.
const PROTOCOLO: &str = "\n
PUEDES LEER EL PROYECTO. No de memoria: pídelo. Escribe una orden al principio de una \
línea y en el mensaje siguiente te doy el contenido de verdad, del disco.

LEER ruta/al/fichero
LEER ruta/al/fichero:120-260
BUSCAR texto
BUSCAR texto EN src/
LISTAR src/
LISTAR *.rs

Reglas de las órdenes:
- Si la respuesta depende de lo que ponga en el proyecto, tu PRIMER mensaje son órdenes y \
nada más. Responder sin haber mirado es inventárselo.
- Empieza por LISTAR o BUSCAR cuando no sepas dónde está algo; no adivines rutas.
- Como mucho cuatro órdenes por mensaje, y solo las que de verdad van juntas.
- La orden abre la línea. Si escribes «voy a leer x» no es una orden, es una frase.
- Cuando ya sepas la respuesta, escribe el informe SIN ninguna orden dentro: un mensaje \
sin órdenes es tu respuesta final y se la doy a quien te encargó esto.
- Si te digo que un fichero no existe o que hay más líneas, hazme caso: eso es el disco, \
no una opinión.";

/// Lanza un subagente contra un proveedor HTTP y espera su informe.
pub async fn lanzar(prov: &Proveedor, cwd: &str, encargo: &Encargo) -> Result<Resultado> {
    if prov.api_key.trim().is_empty() && !prov.base_url.contains("localhost") {
        return Err(eyre!("«{}» no tiene clave configurada", prov.id));
    }
    // Instalarlo dos veces es inofensivo; no instalarlo es un pánico dentro de
    // `reqwest`. Ver la misma línea en `ai::http`.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cliente = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| eyre!("no pude preparar el cliente: {e}"))?;

    let lector = Lector::nuevo(cwd);
    let mut entregado = 0usize;
    let mut mensajes = vec![
        json!({ "role": "system", "content": format!("{}{PROTOCOLO}", super::SISTEMA) }),
        json!({ "role": "user", "content": encargo.tarea.clone() }),
    ];

    for ronda in 0..MAX_RONDAS {
        let texto = una_vuelta(&cliente, prov, &mensajes).await?;
        let pide = peticiones(&texto);

        // Sin órdenes, esto es el informe. Es la única condición de salida que
        // no depende de que el modelo diga una palabra mágica, y por eso es la
        // que se usa: las palabras mágicas se olvidan.
        if pide.is_empty() {
            return Ok(Resultado {
                titulo: encargo.titulo.clone(),
                texto: texto.trim().to_string(),
                // El proveedor no dice lo que cuesta y no hay tabla de precios
                // aquí: inventar una cifra sería peor que no dar ninguna.
                coste_usd: 0.0,
                ok: true,
            });
        }

        let mut respuesta = String::new();
        for p in pide.iter().take(MAX_PETICIONES) {
            respuesta.push_str(&lector.atender(p));
            respuesta.push_str("\n\n");
        }
        entregado += respuesta.len();
        if ronda + 2 >= MAX_RONDAS || entregado >= PRESUPUESTO_BYTES {
            respuesta.push_str(
                "Esta es la última consulta que puedo atenderte. Responde ya con lo que \
                 tengas, sin más órdenes, y di qué te ha quedado sin comprobar.",
            );
        }

        mensajes.push(json!({ "role": "assistant", "content": texto }));
        mensajes.push(json!({ "role": "user", "content": respuesta }));
    }

    // Se acabaron las rondas y sigue pidiendo. Lo que ha averiguado hasta aquí
    // vale más que un error, así que se devuelve diciendo lo que pasó.
    Ok(Resultado {
        titulo: encargo.titulo.clone(),
        texto: format!(
            "(se agotaron las {MAX_RONDAS} consultas al proyecto sin llegar a un informe)\n\n{}",
            ultimo_dicho(&mensajes)
        ),
        coste_usd: 0.0,
        ok: false,
    })
}

/// Una llamada al proveedor. Sin streaming: aquí nadie mira la pantalla.
async fn una_vuelta(
    cliente: &reqwest::Client,
    prov: &Proveedor,
    mensajes: &[Value],
) -> Result<String> {
    let mut cuerpo = json!({
        "model": prov.modelo,
        "messages": mensajes,
        "stream": false,
    });
    // Los dos dialectos del «no pienses tanto», igual que en el cerebro: se
    // mandan solo si el modelo los declara, que a quien no los entiende le
    // provocan un 400 y una ronda perdida.
    if let Some(e) = &prov.esfuerzo {
        cuerpo["reasoning_effort"] = json!(e);
    }
    if let Some(p) = prov.pensar {
        cuerpo["chat_template_kwargs"] = json!({ "enable_thinking": p });
    }

    let r = cliente
        .post(format!("{}/chat/completions", prov.base_url))
        .bearer_auth(&prov.api_key)
        .json(&cuerpo)
        .send()
        .await
        .map_err(|e| eyre!("no llegué a «{}»: {e}", prov.id))?;

    if !r.status().is_success() {
        let codigo = r.status();
        let detalle = r.text().await.unwrap_or_default();
        let detalle: String = detalle.chars().take(200).collect();
        return Err(eyre!("«{}» respondió {codigo}: {detalle}", prov.id));
    }

    let v: Value = r
        .json()
        .await
        .map_err(|e| eyre!("«{}» no devolvió JSON: {e}", prov.id))?;
    let texto = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default();
    Ok(sin_pensamiento(texto))
}

/// Quita el monólogo interno de los modelos que lo escriben en el contenido.
///
/// Los que razonan entre `<think>` y `</think>` meterían ahí las órdenes a
/// medio pensar —«podría LEER esto»— y se ejecutarían como si las hubiera
/// pedido. Aquí no llega por trozos, así que basta con recortar en vez de la
/// máquina de estados que necesita el cerebro.
fn sin_pensamiento(t: &str) -> String {
    let mut s = t.to_string();
    while let Some(i) = s.find("<think>") {
        match s[i..].find("</think>") {
            Some(j) => s.replace_range(i..i + j + "</think>".len(), ""),
            // Abierto y sin cerrar: lo que queda es pensamiento entero.
            None => {
                s.truncate(i);
                break;
            }
        }
    }
    s.trim().to_string()
}

/// Lo último que dijo, para no tirar el trabajo cuando se agotan las rondas.
fn ultimo_dicho(mensajes: &[Value]) -> String {
    mensajes
        .iter()
        .rev()
        .find(|m| m["role"] == "assistant")
        .and_then(|m| m["content"].as_str())
        .unwrap_or("(no dijo nada)")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_pensamiento_no_se_lee_como_ordenes() {
        let t = sin_pensamiento("<think>quizá LEER src/x.rs</think>El informe: nada.");
        assert_eq!(t, "El informe: nada.");
        assert!(peticiones(&t).is_empty());
    }

    /// Un `<think>` sin cerrar es lo que llega cuando el modelo se corta a
    /// media frase. Quedarse con el principio sería quedarse con el monólogo.
    #[test]
    fn un_pensamiento_sin_cerrar_no_deja_basura() {
        assert_eq!(sin_pensamiento("hola <think>y aquí se cortó"), "hola");
    }
}
