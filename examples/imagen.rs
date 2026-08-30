//! ¿Ve el modelo la imagen que le mandamos?
//!
//! Prueba de punta a punta lo único que no se puede comprobar de otra forma:
//! que los píxeles llegan de verdad. Codificar bien la imagen y que la
//! petición no dé error no demuestra nada — un modelo que no la mira contesta
//! igual, inventándose lo que hay dentro.
//!
//! ```text
//! cargo run --release --example imagen -- captura.png
//! cargo run --release --example imagen -- captura.png claude
//! ```

use std::time::{Duration, Instant};
use vsioncode::ai::{AiEvent, Brain, BrainConfig};

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let ruta = std::env::args().nth(1).expect("ruta de la imagen");
    let quien = std::env::args().nth(2).unwrap_or_else(|| "gemini".into());

    let (cat, env) = vsioncode::proveedores::cargar();
    let p = cat
        .proveedor
        .iter()
        .find(|p| p.id == quien)
        .expect("ese proveedor no está en tu configuración");

    let mut cfg = BrainConfig {
        web: false,
        ..Default::default()
    };
    if !p.es_daemon() {
        // El primero suyo que sepa mirar, sea cual sea el de por defecto.
        let m = p
            .ofrecidos()
            .into_iter()
            .find(|m| m.ve_imagenes())
            .expect("ninguno de sus modelos mira imágenes");
        cfg.proveedor = Some(p.con_modelo(&env, &m));
    }
    println!(
        "modelo: {}",
        cfg.proveedor
            .as_ref()
            .map(|p| p.modelo.clone())
            .unwrap_or_else(|| "el daemon de claude".into())
    );

    let a = vsioncode::adjunto::leer(std::path::Path::new(&ruta)).expect("leer la imagen");
    let imgs = vsioncode::adjunto::imagenes_crudas(&[a]);
    match imgs.first() {
        Some((tipo, b64)) => println!("codificada: {tipo}, {} caracteres", b64.len()),
        None => {
            println!("no se pudo codificar: ¿es una imagen y cabe?");
            return;
        }
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut brain =
        Brain::spawn(&cfg, tx, 0, vsioncode::ai::Hilo::Nuevo).expect("arrancar el cerebro");
    brain
        .ask_con("¿Qué ves en esta imagen? Descríbelo en una frase.", imgs)
        .await
        .expect("preguntar");

    let limite = Instant::now() + Duration::from_secs(120);
    let mut texto = String::new();
    while Instant::now() < limite {
        match tokio::time::timeout(Duration::from_secs(90), rx.recv()).await {
            Ok(Some((_, AiEvent::Text(t)))) => texto.push_str(&t),
            Ok(Some((_, AiEvent::Error(e)))) => {
                println!("ERROR: {e}");
                break;
            }
            Ok(Some((_, AiEvent::TurnEnd { .. }))) => break,
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    brain.shutdown().await;
    println!("\n--- respuesta ---\n{}", texto.trim());
}
