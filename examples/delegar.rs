//! ¿Va cada subagente con el modelo que se le ha pedido, y cuánto cuesta?
//!
//! Un test de unidad comprueba la regla de gama —quién debería ir con qué— pero
//! no que `claude --model haiku` arranque de verdad, ni que un modelo de Groq
//! sepa pedir los ficheros que necesita, ni cuánto se paga por un encargo real.
//! Eso solo se ve lanzando uno, y por eso esto es un ejemplo y no un test: gasta
//! cuota.
//!
//! El dato que se viene a buscar aquí es la **diferencia**: el mismo encargo con
//! dos modelos, con su coste y su tiempo al lado. Es lo que convierte «sonnet
//! basta para esto» en una afirmación comprobada en vez de una intuición.
//!
//! Desde que un subagente puede ir con cualquier modelo del fichero, esto sirve
//! además para lo que ningún test alcanza: si el de Groq de turno entiende el
//! protocolo de LEER/BUSCAR/LISTAR o se pone a inventarse el contenido de los
//! ficheros. Se le pasa cualquier nombre de los que ofrece `/agente`.
//!
//!     cargo run --example delegar
//!     cargo run --example delegar -- haiku "cuántos ficheros hay en src/"
//!     cargo run --example delegar -- qwen3.6-27b
use std::time::Instant;

use vsioncode::agente::{Encargo, lanzar, modelo_por_defecto, posibles, resolver};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let cual = args.next();
    let tarea = args.next().unwrap_or_else(|| {
        "¿Qué hace src/agente.rs y qué invariante de seguridad sujeta? Tres líneas.".into()
    });

    let (catalogo, entorno) = vsioncode::proveedores::cargar();
    let disponibles = posibles(&catalogo, &entorno, "claude");
    if disponibles.is_empty() {
        println!("No hay ningún modelo configurado con el que delegar.");
        return;
    }
    println!(
        "Se puede delegar en: {}\n",
        disponibles
            .iter()
            .map(|p| p.nombre.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Sin argumento se prueban los dos que importan comparar: el defecto y el
    // que está debajo. Con argumento, solo ese.
    let cuales: Vec<String> = match cual {
        Some(c) => vec![c],
        None => vec![modelo_por_defecto(None), "haiku".into()],
    };

    let cwd = std::env::current_dir().unwrap().display().to_string();
    for c in cuales {
        let Some((nombre, motor)) = resolver(&catalogo, &entorno, "claude", &c) else {
            println!("\n=== {c} — no está configurado en esta máquina");
            continue;
        };
        let encargo = Encargo {
            titulo: format!("prueba con {nombre}"),
            tarea: tarea.clone(),
            modelo: Some(nombre.clone()),
        };
        let t0 = Instant::now();
        match lanzar(&motor, "claude", &cwd, &encargo).await {
            Ok(r) => println!(
                "\n=== {nombre} ({}) — {:.1} s, {:.4} $ {}\n{}",
                motor.nombre(),
                t0.elapsed().as_secs_f32(),
                r.coste_usd,
                if r.ok { "" } else { "(FALLÓ)" },
                r.texto
            ),
            Err(e) => println!("\n=== {nombre} — no salió: {e}"),
        }
    }
}
