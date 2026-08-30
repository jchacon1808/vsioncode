//! ¿Sabe el modelo trazar un plan, y luego decir por qué paso va?
//!
//! Esto no lo puede comprobar un test. Los tests de unidad sujetan que
//! vsioncode **lee** un bloque `plan:` y una línea `PASO 3`; lo que no pueden
//! saber es si un modelo pequeño, leyendo el prompt de `plan::peticion`,
//! termina de verdad con ese bloque en vez de con una lista en prosa —que se
//! lee igual de bien y no se puede aprobar con una tecla—.
//!
//! Tres cosas se miden aquí, y las tres han fallado alguna vez en algún modelo:
//!
//! 1. **Que el plan llegue en su bloque**, con pasos dentro.
//! 2. **Que no se ponga a trabajar** en la fase de trazar: un bloque `edit:`
//!    aquí es trabajo hecho sin aprobar. Los `bash` de mirar sí valen, que para
//!    eso está la fase.
//! 3. **Que marque el paso** al ejecutar el plan aprobado. Sin la marca el
//!    plan funciona igual, pero la barra deja de saber por dónde va.
//!
//!     cargo run --example plan -- gemini
use vsioncode::plan;

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
    let (url, key, modelo, esfuerzo) = match cual.as_str() {
        "gemini" => (
            "https://generativelanguage.googleapis.com/v1beta/openai",
            clave("GEMINI_API_KEY"),
            "gemini-3.5-flash-lite",
            "minimal",
        ),
        "nvidia" => (
            "https://integrate.api.nvidia.com/v1",
            clave("NVIDIA_API_KEY"),
            "nvidia/nemotron-3-super-120b-a12b",
            "none",
        ),
        _ => (
            "https://api.groq.com/openai/v1",
            clave("GROQ_API_KEY"),
            "qwen/qwen3.6-27b",
            "none",
        ),
    };

    let sis = vsioncode::ai::sistema(&vsioncode::ai::BrainConfig::default(), false);
    let trabajo = vsioncode::ai::modo_de_trabajo(true);

    // Un encargo pequeño y uno de los de verdad. El pequeño es el que destapa
    // lo contrario de lo que se teme: con algo obvio, el modelo tiende a
    // hacerlo en vez de planearlo.
    let encargos = [
        "añadir un comando /pausa que pare la captura durante cinco minutos",
        "cambiar el formato del registro para que guarde también el usuario del sistema",
    ];

    let cliente = reqwest::Client::new();
    let pedir = |mensajes: Vec<serde_json::Value>| {
        let cliente = cliente.clone();
        let (url, key, modelo, esfuerzo) = (url, key.clone(), modelo, esfuerzo);
        async move {
            let cuerpo = serde_json::json!({
                "model": modelo,
                "reasoning_effort": esfuerzo,
                "messages": mensajes,
            });
            // Los planes gratuitos miden tokens por minuto y aquí van prompts
            // largos seguidos: sin pausa, lo que falla es la cuota y no el
            // modelo, y el banco diría lo que no es.
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            let r = cliente
                .post(format!("{url}/chat/completions"))
                .bearer_auth(&key)
                .json(&cuerpo)
                .send()
                .await
                .expect("petición");
            let v: serde_json::Value = r.json().await.expect("json");
            v["choices"][0]["message"]["content"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("(sin respuesta) {v}"))
        }
    };

    println!("modelo: {modelo}\n");
    for encargo in encargos {
        println!("=== {encargo}");

        // Aquí no hay terminal que conteste, y lo primero que hace un modelo
        // bien instruido es mirar: sin esto el banco medía solo el primer turno
        // y nunca llegaba a ver un plan.
        //
        // Y no vale contestarle «no hay salida»: probado, un modelo que cumple
        // el prompt se niega a planear a ciegas —que es lo que se le pide— y el
        // banco medía esa negativa en vez del formato. Se le da entonces el
        // proyecto de verdad, leído del disco por el propio banco, que es lo
        // más parecido a lo que le devolvería la TUI.
        let mut mensajes = vec![
            serde_json::json!({ "role": "system", "content": sis }),
            serde_json::json!({
                "role": "user",
                "content": format!("{}{trabajo}", plan::peticion(encargo, "/home/newt/vsioncode")),
            }),
        ];
        let mut texto = String::new();
        let mut vueltas = 0;
        for _ in 0..3 {
            vueltas += 1;
            texto = pedir(mensajes.clone()).await;
            // Repartir entre subagentes es enterarse igual que lanzar un
            // comando, y es lo que el prompt le pide para lo que sea leer
            // mucho. Contarlo como «se quedó en prosa» era llamar fallo al
            // comportamiento correcto.
            if plan::extraer(&texto).is_some() || !se_entera(&texto) {
                break;
            }
            mensajes.push(serde_json::json!({ "role": "assistant", "content": texto }));
            mensajes.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "En este banco no hay terminal, así que ese comando no se puede \
                     lanzar. Esto es lo que hay en el proyecto:\n\n{}\nCon esto basta: \
                     traza ya el plan.",
                    contexto()
                ),
            }));
        }
        if vueltas > 1 {
            println!("      (miró antes de escribir: {vueltas} turnos)");
        }

        let trazado = plan::extraer(&texto);
        let ediciones = edita(&texto);

        match &trazado {
            Some(p) if !p.pasos.is_empty() => {
                println!("[OK ] plan con {} pasos — «{}»", p.pasos.len(), p.objetivo);
                for (i, paso) in p.pasos.iter().enumerate() {
                    println!("      {}. {paso}", i + 1);
                }
            }
            Some(_) => println!("[MAL] bloque plan vacío: no hay nada que aprobar"),
            // Mirar antes de escribir el plan es exactamente lo que se le pide,
            // así que sin herramientas esto no es un fallo: es el modelo
            // pidiendo los datos que en la TUI sí tendría.
            None if se_entera(&texto) => {
                println!("[ ~ ] todavía no hay plan: sigue enterándose");
            }
            None => println!("[MAL] ni plan ni comandos: se quedó en prosa"),
        }
        if !ediciones {
            println!("      no toca ficheros: bien, esta fase es pensar");
        } else {
            println!("[MAL] ha escrito un bloque edit trazando el plan: eso es trabajar");
        }
        for l in texto.lines() {
            println!("      | {l}");
        }

        // Y el otro extremo: con el plan aprobado, ¿marca el paso?
        let Some(p) = trazado.filter(|p| !p.pasos.is_empty()) else {
            println!();
            continue;
        };
        let ejecutando = pedir(vec![
            serde_json::json!({ "role": "system", "content": sis }),
            serde_json::json!({
                "role": "user",
                "content": format!("{}{trabajo}", plan::encargo(&p)),
            }),
        ])
        .await;
        match plan::avance(&ejecutando) {
            Some(1) => println!("[OK ] empieza marcando PASO 1"),
            Some(n) => println!("[ ~ ] marca PASO {n}: empieza por otro sitio"),
            None => println!("[MAL] sin marca de paso: la barra no sabrá por dónde va"),
        }
        for l in ejecutando.lines() {
            println!("      | {l}");
        }
        println!();
    }
}

/// ¿Está enterándose? Da igual si mirando él o repartiendo: las dos cosas son
/// la fase de trazar haciendo lo que tiene que hacer.
fn se_entera(texto: &str) -> bool {
    !vsioncode::ai::extract_commands(texto).is_empty()
        || !vsioncode::agente::extraer(texto).is_empty()
}

/// El proyecto, para dárselo al modelo en vez de una terminal.
///
/// Lo lee este banco con `std::fs` y no lanzando el comando que pidiera el
/// modelo: aquí también vale la regla de la casa —lo que ejecute algo lo
/// decide una persona—, y para medir el formato del plan basta con que sepa
/// qué ficheros hay y cuánto ocupa cada uno.
fn contexto() -> String {
    let mut v: Vec<String> = Vec::new();
    for d in ["src", "src/app", "docs"] {
        let Ok(dir) = std::fs::read_dir(d) else {
            continue;
        };
        for e in dir.flatten() {
            let ruta = e.path();
            let lineas = std::fs::read_to_string(&ruta)
                .map(|t| t.lines().count())
                .unwrap_or(0);
            if lineas > 0 {
                v.push(format!("{} ({lineas} líneas)", ruta.display()));
            }
        }
    }
    v.sort();
    v.join("\n")
}

/// ¿Hay algún bloque de edición en lo que ha dicho?
///
/// Basta con mirar la valla: aquí no interesa si el cambio se podría aplicar,
/// solo si el modelo se ha puesto a escribir ficheros cuando le habíamos dicho
/// que todavía no.
fn edita(texto: &str) -> bool {
    texto.lines().any(|l| l.trim_start().starts_with("```edit"))
}
