//! Prueba de un proveedor HTTP de punta a punta.
//!
//! Comprueba lo que no comprueban los tests unitarios: que el TLS funciona con
//! el proveedor criptográfico instalado a mano, que el flujo de eventos llega
//! y se parsea, y que el filtro de pensamiento hace su trabajo con texto real.
//!
//!     cargo run --example proveedor -- groq
use vsioncode::ai::http::Proveedor;
use vsioncode::ai::{AiEvent, Brain, BrainConfig};

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cual = std::env::args().nth(1).unwrap_or_else(|| "groq".into());
    let env = std::fs::read_to_string(dirs::home_dir().unwrap().join(".coupset/.env"))
        .unwrap_or_default();
    let clave = |n: &str| {
        env.lines()
            .find_map(|l| l.strip_prefix(&format!("{n}=")))
            .unwrap_or("")
            .trim_matches(['"', '\''])
            .to_string()
    };

    let p = match cual.as_str() {
        "groq" => Proveedor {
            id: "groq".into(),
            base_url: "https://api.groq.com/openai/v1".into(),
            api_key: clave("GROQ_API_KEY"),
            modelo: "openai/gpt-oss-120b".into(),
            esfuerzo: None,
            pensar: None,
        },
        "nvidia" => Proveedor {
            id: "nvidia".into(),
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            api_key: clave("NVIDIA_API_KEY"),
            modelo: "meta/llama-3.3-70b-instruct".into(),
            esfuerzo: None,
            pensar: None,
        },
        _ => Proveedor {
            id: "gemini".into(),
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai".into(),
            api_key: clave("GEMINI_API_KEY"),
            modelo: "gemini-3.6-flash".into(),
            esfuerzo: None,
            pensar: None,
        },
    };
    println!("proveedor: {} · modelo: {}", p.id, p.modelo);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = BrainConfig {
        proveedor: Some(p),
        ..Default::default()
    };
    let mut brain = Brain::spawn(&cfg, tx, 0, vsioncode::ai::Hilo::Nuevo).expect("arrancar");

    brain
        .ask("El comando `ss -ltnp` ha fallado con «permission denied». ¿Por qué y qué hago?")
        .await
        .expect("preguntar");

    let fin = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut primero = None;
    let arranque = std::time::Instant::now();
    while std::time::Instant::now() < fin {
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv()).await {
            Ok(Some((_, ev))) => match ev {
                AiEvent::Ready { model, tools } => {
                    println!("listo: {model} · herramientas: {}", tools.len())
                }
                AiEvent::Text(t) => {
                    if primero.is_none() {
                        primero = Some(arranque.elapsed());
                    }
                    print!("{t}");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
                AiEvent::Error(e) => println!("\nERROR: {e}"),
                AiEvent::TurnEnd { ms, .. } => {
                    println!("\n---");
                    if let Some(p) = primero {
                        println!("primer token: {} ms", p.as_millis());
                    }
                    println!("turno completo: {ms} ms");
                    return;
                }
                _ => {}
            },
            _ => break,
        }
    }
    println!("\n(sin respuesta a tiempo)");
}
