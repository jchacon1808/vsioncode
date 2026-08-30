//! Qué modelos sirve de verdad cada proveedor configurado.
//!
//! Pregunta a `GET {base_url}/models`, que casi todos los que hablan el
//! dialecto de OpenAI exponen. Es la lista **de esa cuenta**: los catálogos
//! publicados en la web envejecen y no dicen a qué tienes acceso tú.
//!
//! No imprime ninguna clave: se usan para autenticarse y nada más.
//!
//! ```text
//! cargo run --release --example modelos
//! ```

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (cfg, entorno) = vsioncode::proveedores::cargar();

    let cliente = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("cliente");

    for p in &cfg.proveedor {
        if p.es_daemon() {
            continue;
        }
        let prov = p.a_proveedor(&entorno);
        if prov.api_key.is_empty() && !prov.base_url.contains("localhost") {
            println!("\n### {} — sin clave, no pregunto\n", prov.id);
            continue;
        }

        let url = format!("{}/models", prov.base_url);
        let r = cliente.get(&url).bearer_auth(&prov.api_key).send().await;

        let cuerpo = match r {
            Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
            Ok(r) => {
                println!("\n### {} — {} responde {}\n", prov.id, url, r.status());
                continue;
            }
            Err(e) => {
                println!("\n### {} — no pude preguntar: {e}\n", prov.id);
                continue;
            }
        };

        let v: serde_json::Value = serde_json::from_str(&cuerpo).unwrap_or_default();
        let mut ids: Vec<String> = v["data"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();

        println!("\n### {} — {} modelos\n", prov.id, ids.len());
        for id in &ids {
            println!("  {id}");
        }
    }
}
