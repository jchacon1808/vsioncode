//! Cuánto tarda cada modelo en empezar a hablar, medido aquí.
//!
//! En una terminal interactiva el tiempo hasta la **primera palabra** manda
//! sobre casi todo lo demás: un modelo que piensa veinte segundos antes de
//! escribir es inservible por muy listo que sea. Y eso no viene en la
//! documentación de nadie.
//!
//! Se pregunta lo mismo a todos, con el prompt de sistema de verdad, y se
//! mira además si respetan el formato —un bloque ```bash— y el idioma.
//!
//! ```text
//! cargo run --release --example latencia -- groq
//! cargo run --release --example latencia            # todos
//! ```

use std::time::{Duration, Instant};

use vsioncode::ai::http::Proveedor;
use vsioncode::ai::{AiEvent, Brain, BrainConfig};

/// Una pregunta corriente de las que recibe esta herramienta: un comando que
/// ha fallado y hay que explicar por qué y proponer el siguiente.
const PREGUNTA: &str = "He ejecutado `git push` y ha fallado con «rejected — non-fast-forward». \
¿Por qué y qué hago?";

/// Qué se mide, y con qué interruptor de razonamiento cada uno.
///
/// El cuarto campo es `reasoning_effort` y el quinto `enable_thinking`: son
/// dos dialectos distintos para pedir lo mismo, y cada modelo entiende el
/// suyo. Mandar el que no toca es un 400.
type Candidato = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<bool>,
);

fn candidatos() -> Vec<Candidato> {
    const G: &str = "GROQ_API_KEY";
    const N: &str = "NVIDIA_API_KEY";
    const E: &str = "GEMINI_API_KEY";
    vec![
        // Groq: los gpt-oss no saben apagar el razonamiento, su mínimo es low.
        ("groq", "openai/gpt-oss-120b", G, Some("low"), None),
        ("groq", "openai/gpt-oss-20b", G, Some("low"), None),
        ("groq", "qwen/qwen3.6-27b", G, Some("none"), None),
        ("groq", "groq/compound", G, None, None),
        ("groq", "groq/compound-mini", G, None, None),
        // NVIDIA. Los Nemotron usan enable_thinking; los demás, effort.
        (
            "nvidia",
            "nvidia/nemotron-3-ultra-550b-a55b",
            N,
            Some("none"),
            None,
        ),
        (
            "nvidia",
            "nvidia/nemotron-3-super-120b-a12b",
            N,
            Some("none"),
            None,
        ),
        (
            "nvidia",
            "nvidia/nemotron-3.5-lightning-30b-a3b",
            N,
            None,
            Some(false),
        ),
        (
            "nvidia",
            "nvidia/nemotron-3-nano-30b-a3b",
            N,
            None,
            Some(false),
        ),
        ("nvidia", "moonshotai/kimi-k3", N, None, None),
        ("nvidia", "moonshotai/kimi-k2.6", N, None, None),
        ("nvidia", "z-ai/glm-5.2", N, None, Some(false)),
        ("nvidia", "z-ai/glm-5.1", N, None, Some(false)),
        ("nvidia", "minimaxai/minimax-m3", N, None, None),
        (
            "nvidia",
            "deepseek-ai/deepseek-v4-flash-0731",
            N,
            Some("none"),
            None,
        ),
        ("nvidia", "meta/llama-3.3-70b-instruct", N, None, None),
        ("nvidia", "qwen/qwen3-next-80b-a3b-instruct", N, None, None),
        // Gemini: el 2.5-lite es el único que viene sin pensar de fábrica.
        ("gemini", "gemini-2.5-flash-lite", E, Some("none"), None),
        ("gemini", "gemini-3.5-flash-lite", E, Some("minimal"), None),
        ("gemini", "gemini-3.6-flash", E, Some("minimal"), None),
        ("gemini", "gemini-3.5-flash", E, Some("minimal"), None),
    ]
}

fn base(id: &str) -> &'static str {
    match id {
        "groq" => "https://api.groq.com/openai/v1",
        "nvidia" => "https://integrate.api.nvidia.com/v1",
        _ => "https://generativelanguage.googleapis.com/v1beta/openai",
    }
}

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Las claves salen de donde ya estén; no se imprimen nunca.
    let (_, entorno) = vsioncode::proveedores::cargar();
    let clave = |n: &str| {
        entorno
            .get(n)
            .cloned()
            .or_else(|| std::env::var(n).ok())
            .unwrap_or_default()
    };

    // Primer argumento: el proveedor. Segundo: parte del nombre del modelo,
    // para volver sobre uno concreto sin repetir la tanda entera.
    let filtro = std::env::args().nth(1);
    let solo = std::env::args().nth(2);
    println!(
        "{:<9} {:<40} {:>9} {:>9}  formato",
        "dónde", "modelo", "1er tok", "total"
    );

    for (prov, modelo, var, esfuerzo, pensar) in candidatos() {
        if let Some(f) = &filtro
            && f != prov
        {
            continue;
        }
        if let Some(m) = &solo
            && !modelo.contains(m.as_str())
        {
            continue;
        }
        let api_key = clave(var);
        if api_key.is_empty() {
            println!("{prov:<9} {modelo:<40} {:>9}", "sin clave");
            continue;
        }

        let cfg = BrainConfig {
            proveedor: Some(Proveedor {
                id: prov.into(),
                base_url: base(prov).into(),
                api_key,
                modelo: modelo.into(),
                esfuerzo: esfuerzo.map(str::to_string),
                pensar,
            }),
            web: false,
            ..Default::default()
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let Ok(mut brain) = Brain::spawn(&cfg, tx, 0, vsioncode::ai::Hilo::Nuevo) else {
            println!("{prov:<9} {modelo:<40} {:>9}", "no arranca");
            continue;
        };

        let arranque = Instant::now();
        if brain.ask(PREGUNTA).await.is_err() {
            println!("{prov:<9} {modelo:<40} {:>9}", "no acepta");
            continue;
        }

        let mut primero: Option<Duration> = None;
        let mut texto = String::new();
        let mut fallo = None;
        let limite = Instant::now() + Duration::from_secs(75);

        while Instant::now() < limite {
            match tokio::time::timeout(Duration::from_secs(70), rx.recv()).await {
                Ok(Some((_, AiEvent::Text(t)))) => {
                    primero.get_or_insert_with(|| arranque.elapsed());
                    texto.push_str(&t);
                }
                Ok(Some((_, AiEvent::Error(e) | AiEvent::Died(e)))) => {
                    // Sin esto, un modelo que devuelve un error se contaba
                    // como «sin respuesta» y no se distinguía de uno lento.
                    fallo = Some(e.chars().take(70).collect::<String>());
                    break;
                }
                Ok(Some((_, AiEvent::TurnEnd { .. }))) => break,
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        brain.shutdown().await;

        if let Some(e) = fallo {
            println!("{prov:<9} {modelo:<40}   {e}");
            continue;
        }

        let bash = texto.contains("```bash") || texto.contains("```sh");
        // Una palabra que no aparece en inglés, para ver si contestó en el
        // idioma que se le pidió sin montar un detector.
        let espanol = texto.contains(" que ") || texto.contains(" está ") || texto.contains(" el ");
        let marca = match (bash, espanol) {
            (true, true) => "bash + español",
            (true, false) => "bash, otro idioma",
            (false, true) => "español, SIN bash",
            (false, false) => "ni formato ni idioma",
        };

        match primero {
            Some(p) => println!(
                "{prov:<9} {modelo:<40} {:>7} ms {:>7} ms  {marca}",
                p.as_millis(),
                arranque.elapsed().as_millis()
            ),
            None => println!("{prov:<9} {modelo:<40} {:>9}", "sin respuesta"),
        }
    }
}
