//! Conversación hablada.
//!
//! Modo continuo: el micrófono se queda escuchando, tú hablas cuando quieras y
//! vsioncode contesta en voz alta mientras trabaja. No hay que pulsar nada
//! para hablar.
//!
//! ## Las dos mitades, y por qué son procesos aparte
//!
//! - **Oír**: un oyente en Python (`voz/escucha.py`) que corta por frases y
//!   transcribe con whisper. Va por `uv`, así que no hay que compilar nada ni
//!   instalar nada en el sistema.
//! - **Hablar**: `piper`, que sintetiza a un fichero y `aplay` lo reproduce.
//!
//! Igual que con `claude`, son programas externos que se detectan al arrancar.
//! Si no están, vsioncode funciona exactamente igual sin voz. Meterlos dentro
//! del binario habría multiplicado su tamaño por cien y habría atado el
//! proyecto a un motor concreto.
//!
//! ## Hablar como se habla, no como se dicta
//!
//! Al hablar de verdad uno hace pausas para pensar en medio de la frase:
//! «pues… estoy probando esto, que no lo había probado… por aquí». Un oyente
//! que corte al primer silencio parte eso en cuatro mensajes y contesta a
//! cada trozo por separado.
//!
//! Por eso hay dos silencios: uno corto que cierra un trozo —se transcribe y
//! **se va viendo crecer el texto en la caja**, sin enviarse— y otro largo que
//! cierra el turno y lo envía. Así se puede pensar a media frase, y además da
//! tiempo a leer lo que ha entendido y corregirlo antes de que salga.
//!
//! ## Lo que no se lee en voz alta
//!
//! Los bloques de comando. Escuchar «sudo nmap guion ese uve mayúscula guion o
//! mayúscula guion pe ene ciento noventa y dos punto ciento sesenta y ocho…»
//! es insufrible, y además es justo la parte que quieres leer con los ojos
//! porque la vas a ejecutar. Se lee la explicación y se dice cuántos comandos
//! propone.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use color_eyre::eyre::{Result, eyre};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

/// Un evento, junto con la generación del oyente que lo emitió.
///
/// Existe por un fallo real: al apagar y volver a encender la voz quedaba
/// algún oyente vivo, y sus transcripciones seguían entrando en la
/// conversación. El usuario veía mensajes suyos que nunca había dicho.
///
/// Con el contador, lo que dice un oyente que ya no es el actual se ignora,
/// aunque por lo que sea siga vivo.
pub type VozMsg = (u64, VozEvento);

/// Lo que el oyente le cuenta a la aplicación.
#[derive(Debug, Clone)]
pub enum VozEvento {
    /// El oyente está en marcha.
    ///
    /// `aviso` trae un problema detectado en el micrófono, con la forma de
    /// arreglarlo. Sin esto, un micro saturado se traduce en «no me oye» y no
    /// hay manera de saber por qué.
    Listo {
        modelo: String,
        aviso: Option<String>,
    },
    /// Ha empezado a detectar voz. Sirve para encender el indicador.
    Oyendo,
    /// Ha dejado de detectarla.
    ///
    /// Se emite también cuando el trozo se descarta por corto: sin eso, un
    /// ruido que enciende el indicador lo dejaba encendido para siempre.
    Callado,
    /// Un trozo reconocido. Se va añadiendo a la caja para verlo crecer,
    /// como en la app de Claude: sin eso no sabes si te está oyendo hasta que
    /// contesta, y si se equivocó ya es tarde.
    Parcial(String),
    /// Has terminado de hablar: enviar lo acumulado.
    FinTurno,
    Error(String),
    Cerrado,
}

/// Dónde viven los motores de voz.
pub fn dir_voz() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("vsioncode/voz")
}

/// Dónde deja `huggingface_hub` lo que se baja.
///
/// Las dos variables antes que la ruta de siempre, en el orden en que las mira
/// la propia biblioteca: quien tiene el disco pequeño mueve la caché, y no
/// mirarlas sería decir que no hay nada descargado en una máquina que lo tiene
/// todo.
fn cache_hf() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("HUGGINGFACE_HUB_CACHE") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("HF_HOME") {
        return Some(PathBuf::from(d).join("hub"));
    }
    dirs::home_dir().map(|h| h.join(".cache/huggingface/hub"))
}

/// Los modelos de reconocimiento que ya están bajados en esta máquina.
///
/// `faster-whisper` no instala nada: se trae el modelo la primera vez que se
/// le pide y lo deja en la caché. Mirar ahí es la única forma de saber cuáles
/// están ya, y sin eso `/voz` ofrecía dos nombres escritos en el binario
/// —y rechazaba el resto— aunque hubiera otros descargados y listos para usar.
///
/// Devuelve la lista vacía si la caché no existe o no se puede leer, que es lo
/// correcto: entonces el menú se queda con los de serie y no promete nada.
pub fn modelos_descargados() -> Vec<String> {
    let Some(hub) = cache_hf() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&hub) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        // `models--Systran--faster-whisper-tiny` → `tiny`. Por el sufijo y no
        // por el nombre entero del repositorio: los publica más de una cuenta
        // y el tamaño es lo único que hay que escribir en `/voz`.
        .filter_map(|n| n.rsplit_once("faster-whisper-").map(|(_, t)| t.to_string()))
        .filter(|t| !t.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Qué hay instalado. Se mira una vez al arrancar.
#[derive(Debug, Clone, Default)]
pub struct Motores {
    pub piper: Option<PathBuf>,
    pub voz_modelo: Option<PathBuf>,
    pub uv: bool,
    pub arecord: bool,
    pub aplay: bool,
}

impl Motores {
    pub fn detectar() -> Self {
        let d = dir_voz();
        let piper = [d.join("piper/piper"), d.join("piper")]
            .into_iter()
            .find(|p| p.is_file());
        let voz_modelo = std::fs::read_dir(&d).ok().and_then(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "onnx"))
        });
        Self {
            piper,
            voz_modelo,
            uv: en_path("uv"),
            arecord: en_path("arecord"),
            aplay: en_path("aplay"),
        }
    }

    /// ¿Se puede escuchar?
    pub fn puede_oir(&self) -> bool {
        self.uv && self.arecord
    }

    /// ¿Se puede hablar?
    pub fn puede_hablar(&self) -> bool {
        self.piper.is_some() && self.voz_modelo.is_some() && self.aplay
    }

    /// Qué falta, dicho de forma que se pueda arreglar.
    pub fn que_falta(&self) -> Vec<&'static str> {
        let mut f = Vec::new();
        if !self.uv {
            f.push("«uv» no está en el PATH (hace falta para el reconocedor)");
        }
        if !self.arecord {
            f.push("«arecord» no está (viene en alsa-utils)");
        }
        if self.piper.is_none() {
            f.push("falta piper en ~/.local/share/vsioncode/voz/");
        }
        if self.voz_modelo.is_none() {
            f.push("falta una voz .onnx en ~/.local/share/vsioncode/voz/");
        }
        if !self.aplay {
            f.push("«aplay» no está (viene en alsa-utils)");
        }
        f
    }
}

fn en_path(nombre: &str) -> bool {
    crate::rutas::en_el_path(nombre)
}

/// El oyente.
pub struct Oido {
    stdin: ChildStdin,
    child: Child,
    /// Identificador del grupo de procesos, para matar también a los nietos.
    grupo: Option<u32>,
    /// En pausa mientras vsioncode habla, para no transcribirse a sí mismo.
    pausado: Arc<AtomicBool>,
}

impl Oido {
    /// Arranca el oyente continuo.
    pub fn arrancar(
        modelo: &str,
        generacion: u64,
        tx: mpsc::UnboundedSender<VozMsg>,
    ) -> Result<Self> {
        let script = escribir_script()?;

        let mut cmd = Command::new("uv");
        // Grupo de procesos propio: al cerrarlo se mata el árbol entero. Sin
        // esto, el Python que lanza `uv` sobrevive con el micrófono abierto.
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd
            .arg("run")
            .arg("--quiet")
            .arg("--with")
            .arg("faster-whisper")
            .arg("python")
            .arg(&script)
            .arg(modelo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| eyre!("no pude arrancar el oyente: {e}"))?;

        let stdin = child.stdin.take().ok_or_else(|| eyre!("sin stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| eyre!("sin stdout"))?;

        tokio::spawn(async move {
            let mut lineas = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lineas.next_line().await {
                let Ok(v) = serde_json::from_str::<Value>(&l) else {
                    continue;
                };
                let ev = match v["tipo"].as_str() {
                    Some("listo") => VozEvento::Listo {
                        modelo: v["modelo"].as_str().unwrap_or("?").into(),
                        aviso: v["aviso"].as_str().map(str::to_string),
                    },
                    Some("oyendo") => VozEvento::Oyendo,
                    Some("callado") => VozEvento::Callado,
                    Some("parcial") => match v["texto"].as_str() {
                        Some(t) if !t.trim().is_empty() => VozEvento::Parcial(t.trim().to_string()),
                        _ => continue,
                    },
                    Some("fin_turno") => VozEvento::FinTurno,
                    Some("error") => VozEvento::Error(v["texto"].as_str().unwrap_or("?").into()),
                    _ => continue,
                };
                if tx.send((generacion, ev)).is_err() {
                    return;
                }
            }
            let _ = tx.send((generacion, VozEvento::Cerrado));
        });

        Ok(Self {
            stdin,
            grupo: child.id(),
            child,
            pausado: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Deja de escuchar del todo.
    ///
    /// Ya no se usa al hablar —para eso está el descarte de eco por texto—,
    /// pero sigue haciendo falta para dejar el micrófono libre sin cerrar el
    /// oyente entero.
    pub async fn pausar(&mut self) {
        if !self.pausado.swap(true, Ordering::Relaxed) {
            let _ = self.stdin.write_all(b"pausa\n").await;
            let _ = self.stdin.flush().await;
        }
    }

    pub async fn reanudar(&mut self) {
        if self.pausado.swap(false, Ordering::Relaxed) {
            let _ = self.stdin.write_all(b"sigue\n").await;
            let _ = self.stdin.flush().await;
        }
    }

    /// Cierra el oyente y **todo lo que colgaba de él**.
    ///
    /// Primero se pide por las buenas, para que suelte el micrófono con
    /// orden. Si en medio segundo no se ha ido, se mata el grupo entero: es
    /// preferible un cierre brusco a dejar un proceso grabando.
    pub async fn cerrar(&mut self) {
        let _ = self.stdin.write_all(b"fin\n").await;
        let _ = self.stdin.flush().await;

        let salio = tokio::time::timeout(std::time::Duration::from_millis(500), self.child.wait())
            .await
            .is_ok();

        if let Some(pid) = self.grupo {
            // El signo menos es lo importante: mata el grupo, no solo al
            // proceso. Ahí es donde vive el Python que abrió el micrófono.
            if !salio {
                ordenar_en_silencio("kill", &["-TERM", &format!("-{pid}")]);
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            ordenar_en_silencio("kill", &["-KILL", &format!("-{pid}")]);
        }
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

/// Lanza una orden del sistema y se traga lo que diga.
///
/// Anular las dos salidas no es manía de limpieza: por debajo de la TUI está
/// la terminal real, y lo que un proceso hijo escriba ahí aparece encima del
/// dibujo sin que nada lo repinte —queda pegado hasta el siguiente
/// fotograma completo—. Y estas órdenes protestan por lo más normal del
/// mundo: `kill` avisa cuando el proceso ya se había muerto solo, que es
/// justo lo que pasa casi siempre al apagar la voz.
///
/// Se espera a que termine, y no se comprueba el resultado a propósito: si
/// matar falla porque ya estaba muerto, el trabajo está hecho igual.
fn ordenar_en_silencio(orden: &str, args: &[&str]) {
    let _ = std::process::Command::new(orden)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// La voz.
///
/// Vive en su propia tarea, y eso resuelve dos problemas a la vez.
///
/// **Tardaba en empezar.** Piper sintetizaba la respuesta entera antes de que
/// sonara nada: con un párrafo son varios segundos de silencio incómodo. Ahora
/// se parte en frases y se va reproduciendo la primera mientras se prepara la
/// siguiente, así que empieza a hablar casi al momento.
///
/// **Y no se callaba.** La síntesis bloqueaba el bucle de la aplicación, así
/// que al apagar la voz la orden de callar llegaba tarde o alcanzaba solo al
/// último trozo, y seguía hablando sola. Ahora hay un contador: al callar sube,
/// y la tarea abandona todo lo que fuera de la cuenta anterior.
pub struct Boca {
    ordenes: mpsc::UnboundedSender<String>,
    /// Sube con cada orden nueva y con cada «cállate». La tarea compara antes
    /// de cada frase y suelta lo que ya no toca.
    turno: Arc<AtomicU64>,
    hablando: Arc<AtomicBool>,
    dicho: Arc<Mutex<String>>,
}

impl Boca {
    pub fn nueva(m: &Motores) -> Option<Self> {
        let piper = m.piper.clone()?;
        let modelo = m.voz_modelo.clone()?;
        limpiar_dichos_huerfanos();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        let turno = Arc::new(AtomicU64::new(0));
        let hablando = Arc::new(AtomicBool::new(false));
        let dicho = Arc::new(Mutex::new(String::new()));

        let (t2, h2, d2) = (turno.clone(), hablando.clone(), dicho.clone());
        tokio::spawn(async move {
            while let Some(texto) = rx.recv().await {
                let mio = t2.load(Ordering::SeqCst);
                h2.store(true, Ordering::SeqCst);
                if let Ok(mut g) = d2.lock() {
                    *g = texto.clone();
                }

                for (i, frase) in frases(&texto).into_iter().enumerate() {
                    // ¿Sigue siendo esta la orden vigente?
                    if t2.load(Ordering::SeqCst) != mio {
                        break;
                    }
                    let wav = ruta_dicho(i);
                    if sintetizar(&piper, &modelo, &frase, &wav).is_err() {
                        break;
                    }
                    if t2.load(Ordering::SeqCst) != mio {
                        break;
                    }
                    if reproducir(&wav, &t2, mio).await.is_err() {
                        break;
                    }
                }

                if t2.load(Ordering::SeqCst) == mio {
                    h2.store(false, Ordering::SeqCst);
                    if let Ok(mut g) = d2.lock() {
                        g.clear();
                    }
                }
            }
        });

        Some(Self {
            ordenes: tx,
            turno,
            hablando,
            dicho,
        })
    }

    pub fn hablando(&self) -> bool {
        self.hablando.load(Ordering::SeqCst)
    }

    /// Lo que se está diciendo ahora mismo, para reconocer el eco.
    pub fn diciendo(&self) -> String {
        self.dicho.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Corta en seco lo que esté diciendo.
    pub fn callar(&self) {
        self.turno.fetch_add(1, Ordering::SeqCst);
        self.hablando.store(false, Ordering::SeqCst);
        if let Ok(mut g) = self.dicho.lock() {
            g.clear();
        }
        // Y por si acaso, se mata cualquier reproducción que siguiera viva.
        // Es basto, pero «apagar la voz» tiene que apagar la voz de verdad.
        ordenar_en_silencio("pkill", &["-f", &patron_aplay()]);
    }

    /// Dice un texto en voz alta, sin bloquear la interfaz.
    pub fn decir(&mut self, texto: &str) -> Result<()> {
        let limpio = para_hablar(texto);
        if limpio.trim().is_empty() {
            return Ok(());
        }
        self.callar();
        self.ordenes
            .send(limpio)
            .map_err(|_| eyre!("la voz dejó de responder"))?;
        Ok(())
    }
}

/// Parte en frases para poder empezar a hablar antes de tenerlo todo listo.
///
/// Frases muy cortas se juntan: sintetizar «Vale.» por separado añade una
/// pausa que suena antinatural.
fn frases(texto: &str) -> Vec<String> {
    let mut fuera = Vec::new();
    let mut actual = String::new();
    for parte in texto.split_inclusive(['.', '?', '!', ':', '\n']) {
        actual.push_str(parte);
        if actual.trim().chars().count() >= 40 {
            fuera.push(std::mem::take(&mut actual));
        }
    }
    if !actual.trim().is_empty() {
        fuera.push(actual);
    }
    if fuera.is_empty() {
        fuera.push(texto.to_string());
    }
    fuera
}

/// El fichero donde piper deja la frase que va a sonar.
///
/// Se alternan dos para poder sintetizar la siguiente mientras suena la
/// anterior, y llevan el PID porque el nombre fijo era el mismo para todos: con
/// dos vsioncode abiertos, uno reescribía la frase que el otro estaba
/// reproduciendo. Es la misma carrera que `pty.rs` pagó con su `init.bash`.
fn ruta_dicho(i: usize) -> PathBuf {
    dir_voz().join(format!("dicho-{}-{}.wav", std::process::id(), i % 2))
}

/// El patrón con el que se remata la reproducción propia.
///
/// Con el PID dentro, apagar la voz aquí no puede callar el `aplay` de otro
/// vsioncode abierto en otra terminal.
fn patron_aplay() -> String {
    format!("aplay -q .*vsioncode/voz/dicho-{}-", std::process::id())
}

/// Borra los `dicho-*.wav` que dejó un vsioncode que ya no está.
///
/// Al llevar el PID en el nombre dejan de reciclarse solos, y esto no es
/// `/tmp`: nadie los limpia por nosotros y quedarían al lado de la voz `.onnx`
/// para siempre.
fn limpiar_dichos_huerfanos() {
    let Ok(entradas) = std::fs::read_dir(dir_voz()) else {
        return;
    };
    for e in entradas.flatten() {
        let nombre = e.file_name();
        let Some(nombre) = nombre.to_str() else {
            continue;
        };
        let Some(resto) = nombre.strip_prefix("dicho") else {
            continue;
        };
        if !resto.ends_with(".wav") {
            continue;
        }
        // `dicho-<pid>-<n>.wav`. Lo que no encaje es del diseño anterior, que
        // compartía dos ficheros entre todas las sesiones: se borra sin más.
        match resto
            .strip_prefix('-')
            .and_then(|r| r.split('-').next())
            .and_then(|p| p.parse::<u32>().ok())
        {
            Some(pid) if crate::sistema::vivo(pid) => {}
            _ => {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Sintetiza a un fichero.
///
/// Encadenar piper y aplay por una tubería sería más elegante, pero piper
/// escribe la cabecera del WAV al final y aplay la necesita al principio.
fn sintetizar(piper: &PathBuf, modelo: &PathBuf, texto: &str, wav: &PathBuf) -> Result<()> {
    // Los dos ficheros se alternan, así que si piper no llega a escribir este
    // sigue ahí la frase de hace dos: sin quitarlo antes y exigirlo después,
    // un fallo de síntesis se oye como una frase repetida y nada lo delata.
    let _ = std::fs::remove_file(wav);

    let mut p = std::process::Command::new(piper)
        .arg("--model")
        .arg(modelo)
        .arg("--output_file")
        .arg(wav)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut si) = p.stdin.take() {
        let _ = si.write_all(texto.as_bytes());
    }
    p.wait()?;

    // Se mira el fichero y no el código de salida: lo que aplay necesita es
    // que haya un WAV con algo dentro, venga piper como venga.
    if std::fs::metadata(wav).map(|m| m.len()).unwrap_or(0) == 0 {
        return Err(eyre!("piper no escribió {}", wav.display()));
    }
    Ok(())
}

async fn reproducir(wav: &PathBuf, turno: &Arc<AtomicU64>, mio: u64) -> Result<()> {
    let mut hijo = Command::new("aplay")
        .arg("-q")
        .arg(wav)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    // Se vigila mientras suena: si llega un «cállate», se corta a media frase
    // en vez de esperar a que termine.
    loop {
        if turno.load(Ordering::SeqCst) != mio {
            let _ = hijo.start_kill();
            let _ = hijo.wait().await;
            return Err(eyre!("cancelado"));
        }
        match hijo.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(60)).await,
            Err(e) => return Err(e.into()),
        }
    }
}

/// Prepara un texto para leerlo en voz alta.
///
/// Quita lo que no se puede escuchar: bloques de comando, marcas de markdown,
/// URLs. Y avisa de cuántos comandos había, porque saber que hay uno esperando
/// sí importa aunque no se lea.
pub fn para_hablar(md: &str) -> String {
    let mut fuera = String::with_capacity(md.len());
    let mut en_bloque = false;
    let mut bloques = 0;

    for linea in md.lines() {
        if linea.trim_start().starts_with("```") {
            if !en_bloque {
                bloques += 1;
            }
            en_bloque = !en_bloque;
            continue;
        }
        if en_bloque {
            continue;
        }

        let mut l = linea.trim().to_string();
        // Las marcas de markdown se leerían como asteriscos y almohadillas.
        for m in ['*', '`', '#', '_'] {
            l = l.replace(m, "");
        }
        // Una URL leída letra a letra es un minuto de ruido.
        if l.contains("http://") || l.contains("https://") {
            l = l
                .split_whitespace()
                .filter(|p| !p.starts_with("http"))
                .collect::<Vec<_>>()
                .join(" ");
        }
        l = l.trim_start_matches(['-', '·', '•']).trim().to_string();
        if !l.is_empty() {
            fuera.push_str(&l);
            if !l.ends_with(['.', ':', '?', '!', ',']) {
                fuera.push('.');
            }
            fuera.push(' ');
        }
    }

    match bloques {
        0 => {}
        1 => fuera.push_str(" Te he dejado un comando."),
        n => fuera.push_str(&format!(" Te he dejado {n} comandos.")),
    }
    fuera.trim().to_string()
}

/// ¿Lo que se ha oído es nuestro propio altavoz?
///
/// Se comparan las palabras de lo transcrito con las de lo que se está
/// diciendo. Si la mayoría ya estaban ahí, es eco.
///
/// El umbral es alto a propósito: colar un eco como si fuera el usuario
/// provoca un bucle en el que se contesta a sí mismo, mientras que descartar
/// por error una interrupción solo cuesta que haya que repetirla.
pub fn parece_eco(oido: &str, diciendo: &str) -> bool {
    if diciendo.trim().is_empty() {
        return false;
    }
    let normaliza = |t: &str| -> Vec<String> {
        t.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|p| p.chars().count() > 2)
            .map(str::to_string)
            .collect()
    };
    let dichas = normaliza(diciendo);
    let oidas = normaliza(oido);
    if oidas.is_empty() || dichas.is_empty() {
        return false;
    }
    let coinciden = oidas.iter().filter(|p| dichas.contains(p)).count();
    coinciden * 10 >= oidas.len() * 6
}

/// ¿Esto se lo ha inventado whisper?
///
/// Los reconocedores alucinan con el ruido: producen frases con estructura
/// correcta que nadie ha dicho. El caso real que motivó esto fue «Hola, al
/// final de la semana, la primera vez que se ha llegado a la mañana, la
/// primera vez que se ha llegado a la mañana» — con el modo de voz apagado.
///
/// La marca característica es la **repetición**: al no tener nada real que
/// transcribir, el modelo se engancha y repite el mismo trozo. Detectarlo es
/// barato y no descarta habla normal, porque nadie dice la misma frase larga
/// dos veces seguidas.
///
/// Es la segunda red de seguridad; la primera es que Silero no dé por voz el
/// ruido. Colar en la conversación algo que el usuario no dijo es lo bastante
/// grave como para tener dos.
pub fn parece_alucinacion(texto: &str) -> bool {
    let limpio = texto.trim().to_lowercase();
    if limpio.len() < 25 {
        return false;
    }

    // Busca un trozo largo repetido dentro de la misma frase.
    let palabras: Vec<&str> = limpio.split_whitespace().collect();
    if palabras.len() >= 8 {
        let n = palabras.len();
        for largo in (4..=n / 2).rev() {
            for inicio in 0..=(n - largo * 2) {
                let a = &palabras[inicio..inicio + largo];
                for j in (inicio + largo)..=(n - largo) {
                    if a == &palabras[j..j + largo] {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Mide el micrófono y devuelve un veredicto en castellano.
///
/// Existe porque «no me oye» no es un dato. Puede ser la ganancia, el ruido de
/// fondo, el dispositivo equivocado o que la voz llegue floja, y cada causa se
/// arregla de otra forma. Sin medirlo solo queda adivinar — y adivinar sobre
/// el micrófono de otra persona no funciona.
pub async fn diagnosticar(segundos: f32) -> String {
    let dir = dir_voz();
    if std::fs::create_dir_all(&dir).is_err() {
        return "no pude preparar el diagnóstico".into();
    }
    let script = dir.join("microfono.py");
    if std::fs::write(&script, include_str!("../voz/microfono.py")).is_err() {
        return "no pude escribir el diagnóstico".into();
    }

    let mut cmd = Command::new("uv");
    // Grupo propio, igual que el oyente: esto también graba el micrófono, y al
    // terminar `uv` el Python nieto sobrevive. Es un fallo de privacidad.
    #[cfg(unix)]
    cmd.process_group(0);
    let hijo = cmd
        .arg("run")
        .arg("--quiet")
        .arg("--with")
        .arg("faster-whisper")
        .arg("python")
        .arg(&script)
        .arg(segundos.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let Ok(hijo) = hijo else {
        return "no pude arrancar el diagnóstico (¿está «uv» en el PATH?)".into();
    };
    // Cerrar vsioncode a media medición suelta esta tarea sin más, y entonces
    // nadie espera a este proceso: el micrófono se quedaría grabando.
    let mut guarda = MataGrupo(hijo.id());
    let salida = hijo.wait_with_output().await;
    guarda.0 = None;

    let Ok(o) = salida else {
        return "no pude esperar al diagnóstico".into();
    };
    let texto = String::from_utf8_lossy(&o.stdout);
    let Some(linea) = texto.lines().last() else {
        // Sin salida, la causa está en stderr —`uv` sin red, faster-whisper que
        // no instala—. Callársela deja al usuario mirando una pantalla en la
        // que no pasa nada.
        return match ultima_linea(&o.stderr) {
            Some(e) => format!("el diagnóstico falló: {e}"),
            None => "el diagnóstico no devolvió nada".into(),
        };
    };
    let Ok(v) = serde_json::from_str::<Value>(linea) else {
        return format!("no entendí el diagnóstico: {linea}");
    };
    if let Some(e) = v["error"].as_str() {
        return format!("Micrófono: {e}");
    }
    interpretar(&v)
}

/// Mata el grupo de procesos de un hijo de `uv` si se abandona la espera.
///
/// El `.await` puede no volver nunca —al cerrar vsioncode la tarea se suelta a
/// medias— y sin esto el nieto se queda con el micrófono abierto. Se desarma
/// en cuanto el proceso termina por su cuenta, para no mandar señales a un PID
/// que ya no es suyo.
struct MataGrupo(Option<u32>);

impl Drop for MataGrupo {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // El signo menos es lo importante: el grupo, no solo el proceso.
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg(format!("-{pid}"))
                .status();
        }
    }
}

/// La última línea con algo, recortada a lo que se puede leer de un vistazo.
fn ultima_linea(salida: &[u8]) -> Option<String> {
    let texto = String::from_utf8_lossy(salida);
    let linea = texto.lines().rev().find(|l| !l.trim().is_empty())?;
    Some(linea.trim().chars().take(200).collect())
}

/// Traduce los números a algo accionable.
fn interpretar(v: &Value) -> String {
    let fondo = v["fondo"].as_f64().unwrap_or(0.0);
    let fuerte = v["fuerte"].as_f64().unwrap_or(0.0);
    let relacion = v["relacion_db"].as_f64().unwrap_or(0.0);
    let recortado = v["recortado_pct"].as_f64().unwrap_or(0.0);
    let voz = v["trozos_con_voz"].as_u64().unwrap_or(0);
    let total = v["trozos_totales"].as_u64().unwrap_or(0);

    // La continuación con «\» es obligatoria en todo lo que se pinta: sin ella
    // la sangría del código entra en la cadena y sale en pantalla.
    let mut t = format!(
        "Micrófono: fondo {fondo:.0}, voz {fuerte:.0}, diferencia {relacion:.1} dB.\n\
         Silero encontró voz en {voz} de {total} trozos.\n\n"
    );

    // Lo que de verdad decide es la diferencia entre hablar y callar. Un
    // micrófono puede sonar altísimo y no servir, si el ruido sube con la voz.
    if recortado > 0.5 {
        t.push_str(
            "Está saturando: la onda no cabe y se pierde información. \
             Prueba «amixer -c 0 set Capture 40%».",
        );
    } else if voz == 0 && total > 0 {
        t.push_str(
            "No ha encontrado voz. Si has hablado durante la prueba, es que llega \
             demasiado floja o tapada por el ruido: acércate al micrófono y prueba \
             «wpctl set-volume @DEFAULT_AUDIO_SOURCE@ 1.0».",
        );
    } else if relacion < 6.0 {
        t.push_str(
            "Tu voz apenas sobresale del ruido de fondo, y ahí el reconocimiento \
             va a fallar por mucho que se ajuste. Un micrófono USB o unos auriculares \
             con micro lo arreglan de golpe; el previo analógico de este portátil es \
             muy ruidoso.",
        );
    } else if relacion < 12.0 {
        t.push_str("Se puede trabajar con esto, pero justo. Habla cerca del micrófono.");
    } else {
        t.push_str("El micrófono está bien: tu voz sobresale con claridad del ruido.");
    }
    t
}

/// Escribe el oyente en disco y devuelve su ruta.
fn escribir_script() -> Result<PathBuf> {
    let dir = dir_voz();
    std::fs::create_dir_all(&dir)?;
    let p = dir.join("escucha.py");
    std::fs::write(&p, include_str!("../voz/escucha.py"))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_lee_los_bloques_de_comando() {
        let md = "El puerto está ocupado.\n\n```bash\nss -ltnp | grep :8080\n```\n\nMíralo.";
        let d = para_hablar(md);
        assert!(!d.contains("ltnp"), "no puede leer el comando: {d}");
        assert!(d.contains("El puerto está ocupado"));
        assert!(d.contains("Míralo"));
    }

    /// Que no se lea el comando no significa que no se mencione: saber que hay
    /// uno esperando importa, aunque haya que mirarlo con los ojos.
    #[test]
    fn avisa_de_cuantos_comandos_hay() {
        let uno = para_hablar("Prueba esto.\n```bash\nls\n```");
        assert!(uno.contains("un comando"), "{uno}");

        let dos = para_hablar("Primero.\n```bash\nls\n```\nY luego.\n```bash\npwd\n```");
        assert!(dos.contains("2 comandos"), "{dos}");

        let ninguno = para_hablar("No hace falta ejecutar nada.");
        assert!(!ninguno.contains("comando"), "{ninguno}");
    }

    #[test]
    fn quita_las_marcas_de_markdown() {
        let d = para_hablar("El error está en **la línea 42** del `main.rs`.");
        assert!(!d.contains('*') && !d.contains('`'), "{d}");
        assert!(d.contains("la línea 42"));
    }

    #[test]
    fn no_deletrea_urls() {
        let d = para_hablar(
            "Está documentado en https://ejemplo.com/una/ruta/larga y ahí lo explican.",
        );
        assert!(!d.contains("http"), "{d}");
        assert!(d.contains("Está documentado en"));
    }

    #[test]
    fn las_frases_acaban_en_punto() {
        // Sin puntuación, la síntesis lo lee todo del tirón y sin respirar.
        let d = para_hablar("Primera línea\nSegunda línea");
        assert!(d.starts_with("Primera línea."), "{d}");
    }

    #[test]
    fn un_texto_que_es_solo_comando_no_se_lee_entero() {
        let d = para_hablar("```bash\nrm -rf /tmp/x\n```");
        assert!(!d.contains("rm"), "{d}");
        assert!(d.contains("un comando"));
    }

    /// El caso real: apareció en la conversación con la voz apagada.
    #[test]
    fn reconoce_su_propio_eco() {
        let diciendo = "El puerto está ocupado por otro proceso. Te he dejado un comando.";
        // El reconocedor oyendo el altavoz devuelve casi lo mismo.
        assert!(parece_eco(
            "el puerto está ocupado por otro proceso",
            diciendo
        ));
        assert!(parece_eco(
            "puerto ocupado por otro proceso te he dejado",
            diciendo
        ));
    }

    /// Y lo importante: que una interrupción de verdad no se confunda con eco,
    /// porque entonces no habría forma de cortarle.
    #[test]
    fn una_interrupcion_no_es_eco() {
        let diciendo = "El puerto está ocupado por otro proceso. Te he dejado un comando.";
        assert!(!parece_eco("para, no era eso lo que quería", diciendo));
        assert!(!parece_eco("espera mejor mira los logs de nginx", diciendo));
    }

    #[test]
    fn sin_nada_sonando_nada_es_eco() {
        assert!(!parece_eco("lo que sea", ""));
    }

    /// Mirar la caché no puede reventar, y lo que saque tiene que ser algo que
    /// se pueda escribir en `/voz`: un tamaño, no una ruta ni un repositorio.
    ///
    /// Sin caché la lista es vacía, y eso es lo correcto: el menú se queda con
    /// los dos de serie en vez de prometer algo que no está.
    #[test]
    fn lo_descargado_son_nombres_utilizables() {
        for m in modelos_descargados() {
            assert!(!m.is_empty());
            assert!(!m.contains('/'), "es un tamaño, no una ruta: «{m}»");
            assert!(!m.contains("--"), "sin el prefijo del repositorio: «{m}»");
        }
    }

    #[test]
    fn caza_la_frase_que_whisper_se_invento() {
        assert!(parece_alucinacion(
            "Hola, al final de la semana, la primera vez que se ha llegado a la mañana, \
             la primera vez que se ha llegado a la mañana."
        ));
    }

    #[test]
    fn no_descarta_habla_normal() {
        for frase in [
            "nada, pues estoy probando el comando este de voz",
            "escanea la red local y dime qué máquinas hay conectadas",
            "no, la verdad que esto se entiende muy bien, le estoy implementando \
             un comando de voz a un proyecto que estoy haciendo en la terminal",
            "sí sí",
            "vale",
        ] {
            assert!(!parece_alucinacion(frase), "descartó habla real: {frase}");
        }
    }

    /// Repetir una palabra suelta es normal al hablar («no, no, no me refería
    /// a eso»). Lo sospechoso es repetir un trozo largo.
    #[test]
    fn repetir_una_palabra_no_es_sospechoso() {
        assert!(!parece_alucinacion(
            "no, no, no me refería a eso, sino a lo otro que dijiste"
        ));
    }

    /// Los mensajes se pintan tal cual en la conversación: si la cadena se
    /// parte sin `\`, la sangría del código sale en pantalla.
    #[test]
    fn el_diagnostico_no_pinta_la_sangria_del_codigo() {
        let casos = [
            serde_json::json!({"recortado_pct": 3.0}),
            serde_json::json!({"trozos_con_voz": 0, "trozos_totales": 9}),
            serde_json::json!({"relacion_db": 3.0, "trozos_con_voz": 4, "trozos_totales": 9}),
            serde_json::json!({"relacion_db": 9.0, "trozos_con_voz": 4, "trozos_totales": 9}),
            serde_json::json!({"relacion_db": 20.0, "trozos_con_voz": 4, "trozos_totales": 9}),
        ];
        for v in casos {
            let t = interpretar(&v);
            assert!(!t.contains("  "), "espacios de más en: {t}");
        }
    }

    /// Dos vsioncode abiertos a la vez no pueden pisarse el fichero de la voz
    /// ni callarse el uno al otro.
    /// Solo Unix: lo que comprueba es que el patrón de `pkill` alcance a la
    /// reproducción propia y no a la de otra sesión. En Windows no hay `pkill`
    /// ni ALSA —la voz está apagada por diseño, `puede_hablar()` es falso—, así
    /// que allí esto no prueba nada del producto: prueba cómo se juntan dos
    /// rutas. Apagarla es más honesto que hacerla pasar.
    #[test]
    #[cfg(unix)]
    fn cada_sesion_tiene_sus_ficheros_de_voz() {
        assert_ne!(
            ruta_dicho(0),
            ruta_dicho(1),
            "los dos turnos no se alternan"
        );
        assert_eq!(ruta_dicho(0), ruta_dicho(2), "solo hacen falta dos");

        // Lo que pkill busca de verdad es el trozo literal tras el comodín.
        let patron = patron_aplay();
        let cola = patron.rsplit(".*").next().unwrap().to_string();
        let mia = format!("aplay -q {}", ruta_dicho(0).display());
        assert!(mia.contains(&cola), "no alcanzaría a la propia: {mia}");

        let ajena = mia.replace(
            &format!("dicho-{}-", std::process::id()),
            &format!("dicho-{}-", std::process::id() + 1),
        );
        assert!(!ajena.contains(&cola), "alcanzaría a otra sesión: {ajena}");
    }

    #[test]
    fn detecta_lo_que_falta_sin_reventar() {
        let vacio = Motores::default();
        assert!(!vacio.puede_oir() && !vacio.puede_hablar());
        assert!(!vacio.que_falta().is_empty());
    }
}
