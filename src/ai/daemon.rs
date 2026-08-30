//! El cerebro que habla con `claude` por tuberías.
//!
//! ## Por qué un daemon y no un proceso por pregunta
//!
//! Lo obvio sería lanzar `claude -p "..."` cada vez que preguntas. Medido en
//! esta máquina, eso cuesta entre 2 y 5 segundos **antes** de que empiece a
//! pensar: descubrimiento de CLAUDE.md, MCP, plugins, LSP, keychain. Es el
//! cuello de botella dominante, muy por encima de la red.
//!
//! Así que arrancamos `claude` **una sola vez** al abrir vsioncode, en modo
//! `--input-format stream-json`, y le vamos escribiendo turnos por stdin como
//! líneas JSON. Verificado: el proceso sobrevive entre turnos, conserva el
//! mismo `session_id` y recuerda lo dicho antes. La latencia percibida baja a
//! ~1 s hasta el primer token.
//!
//! ## Qué herramientas tiene, y por qué esas
//!
//! El modelo **no toca el sistema**. No tiene `Bash`, ni `Edit`, ni `Read`.
//! Redacta texto y propone comandos; ejecutar es cosa de vsioncode, y siempre
//! a la vista. Esa restricción es lo que hace que el resto del diseño de
//! seguridad tenga sentido, así que se comprueba en el evento de arranque.
//!
//! La única excepción son las dos herramientas de **lectura de la web**:
//! `WebSearch` y `WebFetch`. Sirven para lo que un manual no cubre —un error
//! raro, una instalación de algo recién salido, un mensaje que nadie ha visto
//! antes— y no pueden escribir nada en ninguna parte.
//!
//! ### El riesgo real de dar acceso a la web
//!
//! No es que el modelo «se escape»: es la **inyección de instrucciones**. Una
//! página puede contener texto dirigido al modelo («ejecuta esto otro»), y en
//! modo de ejecución automática eso sería una vía directa desde una web
//! cualquiera hasta un comando en tu máquina.
//!
//! La defensa no es una lista de dominios —envejece mal y da falsa
//! seguridad—, sino cortar el eslabón que importa: **un turno que ha leído la
//! web nunca ejecuta nada solo**. Su comando se propone y espera a que pulses.
//! Se pierde automatismo justo donde el automatismo era peligroso.

use std::process::Stdio;
use std::time::Duration;

use color_eyre::eyre::{Result, eyre};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use super::{AiEvent, AiMsg, BrainConfig};

// El prompt de sistema lo monta `super::sistema`, no este módulo. Aquí
// vivieron dos constantes casi idénticas —una con internet y otra sin— que
// además divergieron de la versión de HTTP: allí está contado el fallo que
// costó tenerlo escrito por triplicado.

/// Un cerebro que vive en un proceso `claude` persistente.
pub struct DaemonBrain {
    stdin: ChildStdin,
    child: Child,
    /// Turnos enviados; sirve para no encimar preguntas.
    pending: usize,
}

impl DaemonBrain {
    /// Arranca el daemon y el lector de su salida.
    ///
    /// Los eventos salen por `tx`. Si el binario no existe devolvemos error y
    /// la TUI arranca igual: una terminal sin cerebro sigue siendo una
    /// terminal, y es mejor eso que no arrancar.
    pub fn arrancar(
        cfg: &BrainConfig,
        tx: mpsc::UnboundedSender<AiMsg>,
        generacion: u64,
        hilo: super::Hilo,
    ) -> Result<Self> {
        let mut cmd = Command::new(&cfg.binary);
        cmd.arg("-p")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--include-partial-messages")
            .arg("--verbose")
            .stdin(Stdio::piped())
            // Estos tres recortan varios segundos de arranque cada uno.
            .arg("--setting-sources")
            .arg("")
            .arg("--strict-mcp-config")
            .arg("--disable-slash-commands")
            .arg("--append-system-prompt")
            // El daemon sí tiene herramientas de web; un backend HTTP no.
            .arg(super::sistema(cfg, true))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Las herramientas: ninguna, o solo las de leer la web. Nunca las que
        // tocan el sistema. `--tools` limita cuáles existen y `--allowedTools`
        // aprueba exactamente esas — sin lo segundo se quedarían pidiendo un
        // permiso que en modo sin interfaz nadie puede conceder.
        if cfg.web {
            cmd.arg("--tools").arg("WebSearch,WebFetch");
            cmd.arg("--allowedTools").arg("WebSearch").arg("WebFetch");
        } else {
            cmd.arg("--tools").arg("");
        }

        if let Some(m) = &cfg.model {
            cmd.arg("--model").arg(m);
        }
        if let Some(e) = &cfg.effort {
            cmd.arg("--effort").arg(e);
        }

        // Estrenar la sesión o retomarla. Esto es lo que permite cambiar el
        // esfuerzo sin perder el hilo: la conversación la guarda el proceso que
        // vamos a matar, y `--resume` la trae de vuelta al que arranca. Sin id
        // no se pierde nada respecto a antes: `claude` se inventa uno y la
        // conversación empieza de cero, que es lo que pasaba siempre.
        if let Some(s) = &cfg.sesion {
            cmd.arg(hilo.bandera()).arg(s);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| eyre!("no pude arrancar «{}»: {e}", cfg.binary))?;

        let stdin = child.stdin.take().ok_or_else(|| eyre!("sin stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| eyre!("sin stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| eyre!("sin stderr"))?;

        // Lector de stdout: una línea JSON por evento.
        let tx2 = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(v) => {
                        for ev in translate(&v) {
                            if tx2.send((generacion, ev)).is_err() {
                                return; // la TUI se cerró
                            }
                        }
                    }
                    // Una línea ilegible no debe matar la sesión: el esquema
                    // gana campos entre versiones y hay que ser tolerante.
                    Err(_) => continue,
                }
            }
            let _ = tx2.send((
                generacion,
                AiEvent::Died("el proceso cerró su salida".into()),
            ));
        });

        // Lector de stderr: solo para poder contar qué pasó si muere.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut last = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    last = line;
                }
            }
            if !last.is_empty() {
                let _ = tx.send((generacion, AiEvent::Error(last)));
            }
        });

        Ok(Self {
            stdin,
            child,
            pending: 0,
        })
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    pub fn turn_finished(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    /// Envía un turno de usuario.
    pub async fn ask(&mut self, text: &str) -> Result<()> {
        self.ask_con(text, Vec::new()).await
    }

    /// Un turno con imágenes.
    ///
    /// El daemon habla el formato de bloques de Anthropic, así que una imagen
    /// entra como un bloque más junto al texto. Estuvo sin implementar y el
    /// efecto era peor que no tener la función: la imagen se adjuntaba, se
    /// anunciaba en pantalla, y al otro lado solo llegaba el nombre del
    /// fichero — con lo que el modelo respondía que no la veía y proponía
    /// buscarla por el disco.
    pub async fn ask_con(&mut self, text: &str, imagenes: Vec<(&str, String)>) -> Result<()> {
        let mut bloques = vec![json!({ "type": "text", "text": text })];
        for (tipo, datos) in imagenes {
            bloques.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": tipo, "data": datos },
            }));
        }
        let msg = json!({
            "type": "user",
            "message": { "role": "user", "content": bloques }
        });
        let mut line = serde_json::to_string(&msg)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        self.pending += 1;
        Ok(())
    }

    pub async fn shutdown(&mut self) {
        let _ = self.stdin.shutdown().await;
        let _ = tokio::time::timeout(Duration::from_millis(400), self.child.wait()).await;
        let _ = self.child.start_kill();
    }
}

/// Traduce un evento crudo del flujo a cero o más [`AiEvent`].
///
/// Discriminamos por `type` a mano en lugar de con un enum de serde: el
/// esquema de Claude Code añade tipos entre versiones (`thinking_tokens`,
/// `status`, `prompt_suggestion` aparecieron así) y prefiero ignorar lo
/// desconocido en silencio antes que romperme en una actualización.
/// Los tokens del turno, del `usage` que trae el `result`.
///
/// Los nombres son los de la API de Anthropic y no se traducen: son el
/// contrato con el otro lado, y renombrarlos aquí solo conseguiría que un
/// cambio de esquema pasara desapercibido.
fn uso_de(u: &Value) -> super::Uso {
    super::Uso {
        entrada: u["input_tokens"].as_u64().unwrap_or(0),
        salida: u["output_tokens"].as_u64().unwrap_or(0),
        cache_escritura: u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
        cache_lectura: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
    }
}

fn translate(v: &Value) -> Vec<AiEvent> {
    let mut out = Vec::new();
    match v.get("type").and_then(Value::as_str) {
        Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            out.push(AiEvent::Ready {
                model: v["model"].as_str().unwrap_or("?").to_string(),
                tools: v["tools"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| t.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
        Some("user") => {
            // El resultado de una herramienta vuelve como mensaje de usuario.
            if let Some(cs) = v["message"]["content"].as_array() {
                for c in cs {
                    if c.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let error = c["is_error"].as_bool().unwrap_or(false);
                    let vacio = match &c["content"] {
                        Value::String(s) => s.trim().is_empty(),
                        Value::Array(a) => a.is_empty(),
                        Value::Null => true,
                        _ => false,
                    };
                    out.push(AiEvent::ToolResult {
                        ok: !error && !vacio,
                    });
                }
            }
        }
        Some("assistant") => {
            // Los bloques de uso de herramienta llegan completos aquí, con sus
            // argumentos ya montados; en el flujo parcial vienen troceados y
            // habría que reensamblar el JSON a mano.
            if let Some(cs) = v["message"]["content"].as_array() {
                for c in cs {
                    if c.get("type").and_then(Value::as_str) == Some("tool_use") {
                        let name = c["name"].as_str().unwrap_or("?").to_string();
                        // El flujo emite a veces el bloque antes de tener
                        // montados sus argumentos, y entonces trae relleno.
                        // Anunciar «buscando: placeholder» es peor que no
                        // anunciar nada: parece que hace algo raro.
                        let bruto = c["input"]["query"]
                            .as_str()
                            .or_else(|| c["input"]["url"].as_str())
                            .or_else(|| c["input"]["prompt"].as_str())
                            .unwrap_or("")
                            .trim();
                        // Y el relleno puede venir en cualquiera de los tres
                        // campos, no solo en el primero que exista: se ha visto
                        // `url: https://example.com` con `prompt: placeholder`
                        // al lado, y colándose por `url` pasaba el filtro.
                        let relleno = ["query", "url", "prompt"].iter().any(|k| {
                            c["input"][*k]
                                .as_str()
                                .is_some_and(|v| v.trim().eq_ignore_ascii_case("placeholder"))
                        });
                        if relleno {
                            continue;
                        }
                        // `example.com` y sus hermanos los reserva la IANA para
                        // documentación. Nadie los consulta a propósito: son la
                        // otra cara del mismo relleno.
                        let bajo = bruto.to_ascii_lowercase();
                        let dominio_de_ejemplo = ["example.com", "example.org", "example.net"]
                            .iter()
                            .any(|d| bajo.contains(d));
                        if dominio_de_ejemplo {
                            continue;
                        }
                        // `about:blank`, `data:` y compañía no son internet.
                        // El modelo las emite por error de vez en cuando, y
                        // tratarlas como una lectura de la web bloqueaba
                        // después comandos que no tenían nada que ver.
                        let esquema_falso = ["about:", "data:", "file:", "chrome:"]
                            .iter()
                            .any(|e| bruto.to_ascii_lowercase().starts_with(e));
                        if esquema_falso {
                            continue;
                        }
                        out.push(AiEvent::Tool {
                            name,
                            detail: bruto.to_string(),
                        });
                    }
                }
            }
        }
        Some("stream_event") => {
            let ev = &v["event"];
            match ev.get("type").and_then(Value::as_str) {
                Some("message_start") => out.push(AiEvent::TurnStart),
                Some("content_block_delta") => {
                    let d = &ev["delta"];
                    match d.get("type").and_then(Value::as_str) {
                        // El texto visible.
                        Some("text_delta") => {
                            if let Some(t) = d["text"].as_str() {
                                out.push(AiEvent::Text(t.to_string()));
                            }
                        }
                        // El pensamiento NO se pinta. Medido: para responder
                        // «Ok», 98 de 105 tokens fueron pensamiento. Mostrarlo
                        // sería ruido; ignorarlo del todo sería mentir sobre
                        // que está trabajando, así que solo anima el indicador.
                        Some("thinking_delta") => out.push(AiEvent::Thinking),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Some("rate_limit_event") => {
            let i = &v["rate_limit_info"];
            out.push(AiEvent::Quota {
                kind: i["rateLimitType"].as_str().unwrap_or("?").into(),
                status: i["status"].as_str().unwrap_or("?").into(),
                resets_at: i["resetsAt"].as_u64().unwrap_or(0),
            });
        }
        Some("result") => {
            if v["is_error"].as_bool().unwrap_or(false) {
                out.push(AiEvent::Error(
                    v["result"]
                        .as_str()
                        .unwrap_or("error desconocido")
                        .to_string(),
                ));
            }
            out.push(AiEvent::TurnEnd {
                cost_usd: v["total_cost_usd"].as_f64().unwrap_or(0.0),
                ms: v["duration_ms"].as_u64().unwrap_or(0),
                uso: uso_de(&v["usage"]),
            });
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn traduce_init() {
        let v = json!({"type":"system","subtype":"init","model":"claude-haiku-4-5",
                       "tools":["WebSearch","WebFetch"]});
        match &translate(&v)[0] {
            AiEvent::Ready { model, tools } => {
                assert_eq!(model, "claude-haiku-4-5");
                assert_eq!(tools.len(), 2);
            }
            other => panic!("esperaba Ready, vino {other:?}"),
        }
    }

    /// El flujo manda a veces el bloque de herramienta antes de tener sus
    /// argumentos, con relleno dentro. Anunciarlo confunde más que callarse.
    /// `about:blank` no es internet. El modelo la emite por error a veces, y
    /// contarla como lectura de la web bloqueaba después comandos locales que
    /// no tenían nada que ver con ninguna página.
    #[test]
    fn una_url_que_no_es_una_web_no_cuenta() {
        for url in ["about:blank", "data:text/html,x", "file:///etc/hosts"] {
            let v = json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","name":"WebFetch","input":{"url":url}}]}});
            assert!(translate(&v).is_empty(), "«{url}» no debería anunciarse");
        }
    }

    #[test]
    fn distingue_una_herramienta_que_trajo_algo_de_una_que_fallo() {
        let bien = json!({"type":"user","message":{"content":[
            {"type":"tool_result","content":"La última versión es 15.2.0"}]}});
        assert!(matches!(
            &translate(&bien)[0],
            AiEvent::ToolResult { ok: true }
        ));

        let mal = json!({"type":"user","message":{"content":[
            {"type":"tool_result","is_error":true,"content":"no se pudo abrir"}]}});
        assert!(matches!(
            &translate(&mal)[0],
            AiEvent::ToolResult { ok: false }
        ));

        let vacio = json!({"type":"user","message":{"content":[
            {"type":"tool_result","content":""}]}});
        assert!(matches!(
            &translate(&vacio)[0],
            AiEvent::ToolResult { ok: false }
        ));
    }

    #[test]
    fn ignora_el_relleno_del_flujo() {
        let v = json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"WebSearch","input":{"query":"placeholder"}}]}});
        assert!(translate(&v).is_empty());
    }

    /// La asimetría que se comía las propuestas.
    ///
    /// Un turno con búsqueda son dos mensajes del modelo —lo que dice antes de
    /// salir a internet y lo que dice después—, y por tanto dos `message_start`,
    /// pero un solo `result`. Quien lea `TurnStart` como «empieza el turno» y
    /// vacíe ahí lo acumulado se lleva por delante la primera mitad, que es
    /// donde suele ir el bloque de comando o la edición.
    #[test]
    fn un_turno_con_busqueda_trae_dos_comienzos_y_un_solo_final() {
        let flujo = [
            json!({"type":"stream_event","event":{"type":"message_start"}}),
            json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","name":"WebSearch","input":{"query":"error E0308"}}]}}),
            json!({"type":"user","message":{"content":[
                {"type":"tool_result","content":"lo que trajo"}]}}),
            json!({"type":"stream_event","event":{"type":"message_start"}}),
            json!({"type":"result","total_cost_usd":0.01,"duration_ms":1200}),
        ];
        let eventos: Vec<AiEvent> = flujo.iter().flat_map(translate).collect();
        let comienzos = eventos
            .iter()
            .filter(|e| matches!(e, AiEvent::TurnStart))
            .count();
        let finales = eventos
            .iter()
            .filter(|e| matches!(e, AiEvent::TurnEnd { .. }))
            .count();
        assert_eq!(comienzos, 2, "uno antes de la búsqueda y otro después");
        assert_eq!(finales, 1, "el turno acaba una sola vez");
    }

    /// El relleno también viene disfrazado de dirección.
    ///
    /// Visto en vivo: el bloque trae `url: https://example.com` y el
    /// `placeholder` al lado, en `prompt`. Quedarse con el primer campo que
    /// exista dejaba pasar un «abriendo: https://example.com» que el modelo
    /// no había pedido.
    #[test]
    fn el_relleno_disfrazado_de_direccion_tampoco_pasa() {
        let con_prompt = json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"WebFetch",
             "input":{"url":"https://example.com","prompt":"placeholder"}}]}});
        assert!(translate(&con_prompt).is_empty());

        let solo_dominio = json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"WebFetch","input":{"url":"http://example.org/x"}}]}});
        assert!(translate(&solo_dominio).is_empty());
    }

    #[test]
    fn detecta_el_uso_de_una_herramienta() {
        let v = json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"WebSearch","input":{"query":"error E0308 rust"}}]}});
        match &translate(&v)[0] {
            AiEvent::Tool { name, detail } => {
                assert_eq!(name, "WebSearch");
                assert_eq!(detail, "error E0308 rust");
            }
            other => panic!("esperaba Tool, vino {other:?}"),
        }
    }

    #[test]
    fn traduce_texto_e_ignora_pensamiento() {
        let texto = json!({"type":"stream_event","event":{"type":"content_block_delta",
            "delta":{"type":"text_delta","text":"hola"}}});
        let pensar = json!({"type":"stream_event","event":{"type":"content_block_delta",
            "delta":{"type":"thinking_delta","thinking":"mmm"}}});
        assert!(matches!(&translate(&texto)[0], AiEvent::Text(t) if t == "hola"));
        assert!(matches!(&translate(&pensar)[0], AiEvent::Thinking));
    }

    #[test]
    fn traduce_cuota() {
        let v = json!({"type":"rate_limit_event","rate_limit_info":
            {"status":"allowed","resetsAt":1787270400u64,"rateLimitType":"five_hour"}});
        assert!(matches!(&translate(&v)[0], AiEvent::Quota { kind, .. } if kind == "five_hour"));
    }

    #[test]
    fn un_tipo_desconocido_no_rompe_nada() {
        let v = json!({"type":"algo_que_no_existe_todavia","x":1});
        assert!(translate(&v).is_empty());
    }
    /// Los tokens del turno se leen del `usage`, que es lo único que permite
    /// saber si la caché está entrando o se está pagando el contexto entero.
    #[test]
    fn traduce_los_tokens_del_turno() {
        let v = json!({"type":"result","total_cost_usd":0.01,"duration_ms":10,
                       "usage":{"input_tokens":12,"output_tokens":34,
                                "cache_creation_input_tokens":56,
                                "cache_read_input_tokens":78}});
        match translate(&v).into_iter().next().expect("un evento") {
            AiEvent::TurnEnd { uso, .. } => {
                assert_eq!(uso.entrada, 12);
                assert_eq!(uso.salida, 34);
                assert_eq!(uso.cache_escritura, 56);
                assert_eq!(uso.cache_lectura, 78);
                assert_eq!(uso.entrada_total(), 12 + 56 + 78);
            }
            otro => panic!("esperaba TurnEnd, vino {otro:?}"),
        }
    }

    /// Sin `usage` —un proveedor que no lo manda, o un error— no se inventa
    /// nada: ceros, y `/coste` dice que no hay datos.
    #[test]
    fn sin_usage_no_se_inventan_tokens() {
        let v = json!({"type":"result","total_cost_usd":0.0,"duration_ms":1});
        match translate(&v).into_iter().next().expect("un evento") {
            AiEvent::TurnEnd { uso, .. } => assert!(uso.vacio()),
            otro => panic!("esperaba TurnEnd, vino {otro:?}"),
        }
    }
}
