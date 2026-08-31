//! El asistente de primer arranque.
//!
//! La primera vez que vsioncode se abre en una máquina no hay configuración: no
//! sabe a quién preguntar. Antes eso se resolvía con un comando escondido
//! (`/proveedores`) que escribía un fichero de ejemplo, lo cual exige saber de
//! antemano que existe. **Una opción que la interfaz no enseña no existe para
//! quien la usa**, así que la puesta en marcha se enseña sola: al abrir por
//! primera vez, una ventana explica qué es esto, qué servicios sirven, dónde se
//! saca la clave de cada uno y la pide.
//!
//! ## Dónde acaba lo que se teclea aquí
//!
//! En dos ficheros, y esa separación es deliberada:
//!
//! - `~/.config/vsioncode/proveedores.toml` — qué servicios hay y con qué
//!   modelos. Es el fichero que se comparte, se copia entre máquinas y se
//!   enseña en una captura de pantalla.
//! - `~/.config/vsioncode/.env` — las claves, con permisos `600`. El TOML solo
//!   guarda `api_key = "env:GROQ_API_KEY"`, nunca el valor.
//!
//! ## Por qué no valida la clave contra el servidor
//!
//! Porque una comprobación en red convierte la primera pantalla en una espera
//! que puede fallar por motivos ajenos a la clave —sin cobertura, servicio
//! caído, proxy—, y dejar a alguien atascado en la puerta de entrada es peor
//! que dejarle pasar con un dato por confirmar. La clave se ejercita en el
//! primer turno real, donde el error ya tiene dónde contarse.

use std::path::{Path, PathBuf};

/// Un servicio que se ofrece en la primera pantalla.
#[derive(Debug, Clone, Copy)]
pub struct Servicio {
    /// Identificador en el TOML.
    pub id: &'static str,
    /// Cómo se llama para quien lo lee.
    pub nombre: &'static str,
    /// Qué ofrece, en una línea.
    pub nota: &'static str,
    /// Dónde se consigue la clave.
    pub url: &'static str,
    /// Variable de entorno donde vivirá la clave.
    pub variable: &'static str,
    /// URL base de la API, vacía si es el proceso local.
    pub base_url: &'static str,
    /// Modelo con el que arranca.
    pub modelo: &'static str,
    /// Bloque de modelos para el TOML.
    pub modelos: &'static str,
}

impl Servicio {
    /// El proceso local de Claude Code, que no lleva clave.
    pub fn es_daemon(&self) -> bool {
        self.base_url.is_empty()
    }
}

/// Lo que se ofrece, en el orden en que conviene mirarlo.
///
/// Primero la suscripción, porque quien ya paga Claude Code no necesita ninguna
/// clave más; después los tres servicios con capa gratuita que se han probado
/// de extremo a extremo; al final el local, que no necesita cuenta.
pub const SERVICIOS: &[Servicio] = &[
    Servicio {
        id: "claude",
        nombre: "Claude Code · suscripción Anthropic",
        nota: "Usa la sesión de Claude Code ya iniciada. Sin clave de API.",
        url: "https://claude.com/claude-code",
        variable: "",
        base_url: "",
        modelo: "",
        modelos: "",
    },
    Servicio {
        id: "groq",
        nombre: "Groq",
        nota: "El más rápido. Capa gratuita: 1.000 peticiones al día.",
        url: "https://console.groq.com/keys",
        variable: "GROQ_API_KEY",
        base_url: "https://api.groq.com/openai/v1",
        modelo: "openai/gpt-oss-120b",
        modelos: r#"
  [[proveedor.modelos]]
  id = "openai/gpt-oss-120b"
  nota = "equilibrado — 642 ms"
  esfuerzo = "low"

  [[proveedor.modelos]]
  id = "qwen/qwen3.6-27b"
  nota = "el más rápido, y programa bien — 165 ms"
  esfuerzo = "none"
  imagenes = true

  [[proveedor.modelos]]
  id = "groq/compound"
  nota = "busca en internet por su cuenta — 1,6 s"
"#,
    },
    Servicio {
        id: "nvidia",
        nombre: "NVIDIA NIM",
        nota: "Catálogo amplio. Capa gratuita: ~40 peticiones por minuto.",
        url: "https://build.nvidia.com",
        variable: "NVIDIA_API_KEY",
        base_url: "https://integrate.api.nvidia.com/v1",
        modelo: "nvidia/nemotron-3-super-120b-a12b",
        modelos: r#"
  [[proveedor.modelos]]
  id = "nvidia/nemotron-3-super-120b-a12b"
  nota = "el mejor equilibrio de NVIDIA — 564 ms"
  esfuerzo = "none"

  [[proveedor.modelos]]
  id = "minimaxai/minimax-m3"
  nota = "rápido y solvente — 829 ms"

  [[proveedor.modelos]]
  id = "nvidia/nemotron-3.5-lightning-30b-a3b"
  nota = "ligero, 3B activos — 925 ms"
  pensar = false
"#,
    },
    Servicio {
        id: "gemini",
        nombre: "Google AI Studio",
        nota: "Modelos Gemini, con capa gratuita amplia.",
        url: "https://aistudio.google.com/apikey",
        variable: "GEMINI_API_KEY",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        modelo: "gemini-3.5-flash-lite",
        modelos: r#"
  [[proveedor.modelos]]
  id = "gemini-3.5-flash-lite"
  nota = "el más rápido de Google — 626 ms"
  esfuerzo = "minimal"
  imagenes = true

  [[proveedor.modelos]]
  id = "gemini-3.6-flash"
  nota = "más capaz, algo más lento — 1,0 s"
  esfuerzo = "minimal"
  imagenes = true
"#,
    },
    Servicio {
        id: "ollama",
        nombre: "Ollama · en este equipo",
        nota: "Modelos locales. Sin cuenta, sin clave y sin salir del equipo.",
        url: "https://ollama.com",
        variable: "",
        base_url: "http://localhost:11434/v1",
        modelo: "qwen3.6:8b",
        modelos: r#"
  [[proveedor.modelos]]
  id = "qwen3.6:8b"
  nota = "local; cambia el id por el que tengas descargado"
"#,
    },
];

/// En qué punto del asistente estamos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Paso {
    /// Qué es esto y qué hace falta para empezar.
    Portada,
    /// La lista de servicios.
    Eleccion,
    /// Pidiendo la clave del servicio `i`.
    Clave { i: usize },
    /// Resumen de lo guardado.
    Hecho,
}

/// Estado del asistente.
#[derive(Debug, Clone)]
pub struct Bienvenida {
    pub paso: Paso,
    /// Fila señalada en la lista.
    pub sel: usize,
    /// Lo que se está tecleando como clave.
    pub entrada: String,
    /// Servicios ya configurados, por su índice en [`SERVICIOS`].
    pub listos: Vec<usize>,
    /// Servicio que quedará como predeterminado.
    pub por_defecto: Option<usize>,
    /// Lo último que fue mal, para decirlo donde ocurrió.
    pub error: Option<String>,
    /// Qué se escribió, para contarlo al final.
    pub resumen: Vec<String>,
    /// `claude` está instalado en esta máquina.
    pub hay_claude: bool,
}

impl Default for Bienvenida {
    fn default() -> Self {
        Self::nueva()
    }
}

impl Bienvenida {
    /// Arranca el asistente sabiendo qué hay ya configurado.
    ///
    /// Esto no es un detalle de cortesía: la ventana también se abre con
    /// `/configurar` **después** de haber configurado algo, y empezar en
    /// blanco haría que al guardar se perdiera lo anterior. Así, quien solo
    /// tenga la suscripción de Anthropic la ve marcada y puede añadir un
    /// servicio más sin tocar lo que ya funciona.
    pub fn nueva() -> Self {
        let (catalogo, _) = crate::proveedores::cargar();
        let mut listos = Vec::new();
        let mut por_defecto = None;

        for (i, s) in SERVICIOS.iter().enumerate() {
            if catalogo.proveedor.iter().any(|p| p.id == s.id) {
                listos.push(i);
                if catalogo.por_defecto.as_deref() == Some(s.id) {
                    por_defecto = Some(i);
                }
            }
        }

        // Señalar el primero sin configurar: quien vuelve aquí viene a añadir
        // algo, no a mirar lo que ya tiene.
        let sel = (0..SERVICIOS.len())
            .find(|i| !listos.contains(i))
            .unwrap_or(0);

        Self {
            paso: Paso::Portada,
            sel,
            entrada: String::new(),
            listos,
            por_defecto,
            error: None,
            resumen: Vec::new(),
            hay_claude: crate::rutas::en_el_path("claude"),
        }
    }

    /// ¿Se abre sobre una configuración que ya existía?
    pub fn ya_configurado(&self) -> bool {
        !self.listos.is_empty()
    }

    /// Servicios que ya estaban antes de abrir esta ventana.
    ///
    /// Se distingue de `listos` para poder contar en el resumen solo lo que se
    /// acaba de añadir: repetir lo que ya había como si fuera nuevo haría
    /// dudar de si se ha tocado algo que funcionaba.
    pub fn nombres_previos(&self) -> Vec<&'static str> {
        self.listos.iter().map(|&i| SERVICIOS[i].nombre).collect()
    }

    /// ¿Es la primera vez que se abre en esta máquina?
    ///
    /// Se decide por la ausencia del fichero de configuración y no por una
    /// marca aparte: si alguien lo borra para empezar de cero, la puesta en
    /// marcha vuelve a salir, que es justo lo que esperaría.
    pub fn hace_falta() -> bool {
        !crate::proveedores::ruta().is_file()
    }

    /// Cuántos servicios hay para elegir, contando la fila de terminar.
    pub fn filas(&self) -> usize {
        SERVICIOS.len() + 1
    }

    /// La fila de «terminar» es la última.
    pub fn es_fila_final(&self, i: usize) -> bool {
        i == SERVICIOS.len()
    }

    pub fn mover(&mut self, paso: isize) {
        let n = self.filas() as isize;
        let actual = self.sel as isize;
        self.sel = (actual + paso).rem_euclid(n) as usize;
    }

    /// ¿Ya está configurado este servicio?
    pub fn esta_listo(&self, i: usize) -> bool {
        self.listos.contains(&i)
    }

    /// ¿Es este el que se usará al arrancar?
    pub fn es_por_defecto(&self, i: usize) -> bool {
        self.por_defecto == Some(i)
    }

    /// Marca como predeterminado el señalado, si ya está configurado.
    ///
    /// Con varios servicios configurados hay que poder decir cuál manda sin
    /// editar el TOML a mano: configurarlos en otro orden sería la única
    /// alternativa, y eso es adivinar una regla en vez de decidirla.
    pub fn marcar_por_defecto(&mut self) -> bool {
        if self.esta_listo(self.sel) {
            self.por_defecto = Some(self.sel);
            return true;
        }
        false
    }

    /// La clave de este servicio ya está en el entorno de la sesión.
    ///
    /// Quien ya exporta `GROQ_API_KEY` en su `.bashrc` no tiene por qué volver
    /// a teclearla: se declara `env:GROQ_API_KEY` y se lee de donde ya está.
    pub fn ya_en_entorno(i: usize) -> bool {
        let s = &SERVICIOS[i];
        !s.variable.is_empty() && std::env::var(s.variable).is_ok_and(|v| !v.trim().is_empty())
    }
}

/// La ruta con `~` en lugar de la carpeta personal.
///
/// No es cosmética: la ruta completa parte la línea en dos en una ventana de
/// setenta columnas, y lo que hay que leer ahí es el nombre del fichero, no el
/// prefijo que es igual para todo el mundo.
pub fn corta(p: &Path) -> String {
    let t = p.display().to_string();
    match dirs::home_dir() {
        Some(h) if t.starts_with(&h.display().to_string()) => {
            format!("~{}", &t[h.display().to_string().len()..])
        }
        _ => t,
    }
}

/// Dónde viven las claves.
pub fn ruta_env() -> PathBuf {
    crate::proveedores::ruta()
        .parent()
        .map(|d| d.join(".env"))
        .unwrap_or_else(|| PathBuf::from(".env"))
}

/// Qué significa un clic en una fila de la lista.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clic {
    /// Ahí no hay nada.
    Nada,
    /// Señalar este servicio.
    Elegir(usize),
    /// Abrir la página del que ya está señalado.
    Abrir,
}

/// Fila —dentro del recuadro— donde se pinta la dirección del señalado.
///
/// La lista arranca con dos líneas de encabezado y cada servicio ocupa dos
/// —nombre y nota—; el señalado gana una tercera con su dirección. Vive aquí y
/// no en la vista para que el ratón y el dibujado cuenten las mismas filas: si
/// cada uno las contara por su cuenta, el clic acabaría una línea por encima
/// del enlace sin que nada fallara a la vista.
pub fn fila_url(sel: usize) -> usize {
    4 + 2 * sel
}

/// A qué corresponde la fila `r` de la lista, con `sel` señalado.
pub fn clic_en(sel: usize, r: usize) -> Clic {
    if r < 2 {
        return Clic::Nada;
    }
    let url = fila_url(sel);
    if sel < SERVICIOS.len() && r == url {
        return Clic::Abrir;
    }
    // Por debajo del enlace, las filas van corridas una posición: la del
    // señalado ocupa un renglón que los de arriba no tienen.
    let i = if sel < SERVICIOS.len() && r > url {
        (r - 3) / 2
    } else {
        (r - 2) / 2
    };
    if i < SERVICIOS.len() {
        Clic::Elegir(i)
    } else {
        Clic::Nada
    }
}

/// Abre una dirección en el navegador del escritorio.
///
/// Una TUI no puede convertir un texto en un enlace pulsable: lo que pinta son
/// celdas de terminal, y las secuencias de hiperenlace no sobreviven al
/// dibujado. Así que el enlace se abre desde aquí —una tecla o un clic— con el
/// abridor del sistema, que es lo que haría el navegador de todas formas.
///
/// Se lanza y se olvida: esperar a que cierre el navegador dejaría la interfaz
/// congelada, y no hay nada que recoger de vuelta.
pub fn abrir_url(url: &str) -> std::io::Result<()> {
    // Sin `sh -c`: la dirección va como argumento, no como texto que alguien
    // interprete. Aquí las direcciones son constantes del programa, pero un
    // camino que ejecute cadenas es un camino que algún día ejecutará otra.
    let abridor = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(abridor)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Una clave con pinta de clave.
///
/// No se valida contra el servidor —ver la cabecera del módulo—, pero sí se
/// descartan los errores que se ven a simple vista: pegar una URL entera, dejar
/// el campo vacío o pegar varias líneas de un fichero.
pub fn clave_plausible(clave: &str) -> Result<(), String> {
    let c = clave.trim();
    if c.is_empty() {
        return Err("La clave está vacía.".into());
    }
    if c.contains(char::is_whitespace) {
        return Err("Una clave no lleva espacios ni saltos de línea.".into());
    }
    if c.starts_with("http://") || c.starts_with("https://") {
        return Err("Eso es la dirección de la página, no la clave.".into());
    }
    if c.chars().count() < 16 {
        return Err("Demasiado corta para ser una clave.".into());
    }
    Ok(())
}

/// La clave, tapada salvo el final.
///
/// Se enseña el rabo y no la cabeza porque es lo que distingue dos claves del
/// mismo servicio, y porque el prefijo (`gsk_`, `nvapi-`) no aporta nada.
pub fn tapada(clave: &str) -> String {
    let n = clave.chars().count();
    if n <= 4 {
        return "•".repeat(n);
    }
    let cola: String = clave.chars().skip(n - 4).collect();
    format!("{}{}", "•".repeat(n - 4), cola)
}

/// Guarda una clave en el fichero de entorno, sin tocar las demás.
///
/// Reescribe la línea de esa variable si ya estaba: repetirla dejaría dos
/// valores para el mismo nombre y ganaría uno de los dos según quién lea.
pub fn guardar_clave(variable: &str, clave: &str) -> std::io::Result<PathBuf> {
    let ruta = ruta_env();
    guardar_clave_en(&ruta, variable, clave)?;
    Ok(ruta)
}

/// Lo mismo, sobre una ruta dada.
///
/// Existe aparte porque `ruta_env()` mira la configuración de la máquina y eso
/// no se puede probar sin tocarla.
fn guardar_clave_en(ruta: &Path, variable: &str, clave: &str) -> std::io::Result<()> {
    if let Some(d) = ruta.parent() {
        crate::proveedores::crear_carpeta_privada(d)?;
    }

    // Antes de tocar nada: si el fichero ya estaba, se le aprietan aquí los
    // permisos, porque `mode()` al abrir solo cuenta cuando el fichero se crea.
    #[cfg(unix)]
    if ruta.exists() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(ruta, std::fs::Permissions::from_mode(0o600))?;
    }

    let previo = std::fs::read_to_string(ruta).unwrap_or_default();
    let mut lineas: Vec<String> = previo
        .lines()
        .filter(|l| !l.trim_start().starts_with(&format!("{variable}=")))
        .map(str::to_string)
        .collect();
    if lineas.is_empty() {
        lineas.push("# Claves de vsioncode. No se comparte este fichero.".into());
    }
    lineas.push(format!("{variable}={}", clave.trim()));
    let texto = format!("{}\n", lineas.join("\n"));

    // El fichero nace ya con 0600. Escribir primero y corregir los permisos
    // después dejaba la clave en disco legible por cualquier otro usuario de
    // la máquina durante la ventana entre las dos llamadas.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(ruta)?;
        f.write_all(texto.as_bytes())?;
    }
    // En Windows no hay un `mode()` que dar al abrir: la ACL se hereda. Así
    // que el fichero nace **vacío**, se cierra a su dueño, y solo entonces se
    // le mete la clave. La ventana sigue existiendo, pero durante ella dentro
    // no hay nada que leer, que es de lo que se trataba.
    #[cfg(windows)]
    {
        std::fs::File::create(ruta)?;
        crate::privado::cerrar(ruta)?;
        std::fs::write(ruta, texto)?;
    }
    #[cfg(not(any(unix, windows)))]
    std::fs::write(ruta, texto)?;

    Ok(())
}

/// La ruta del `.env`, escrita como una cadena que TOML sabe leer.
///
/// No se puede meter una ruta entre comillas y ya: en TOML lo que va entre
/// comillas es una cadena **básica**, donde la barra invertida escapa. En
/// Windows `env_file = "C:\Users\..."` no es una ruta, es un `\U` a medias, y
/// el fichero entero deja de parsearse —«too few unicode value digits»—. O sea:
/// la puesta en marcha escribía una configuración que el propio programa no
/// podía volver a leer, y solo allí.
///
/// Lo decide `toml` y no un escapado a mano: la biblioteca es la autoridad
/// sobre su formato y elige la forma literal `'...'` cuando hace falta. Aquí
/// escaparlo a ojo sería volver a apostar contra el mismo problema.
fn env_file_toml() -> String {
    env_file_toml_de(&ruta_env())
}

/// Lo mismo, sobre una ruta dada, para poder probarlo con una de Windows sin
/// estar en Windows. `ruta_env()` mira la configuración de la máquina, y en
/// Linux nunca trae una barra invertida: una prueba que la use no puede
/// cazar este fallo.
fn env_file_toml_de(ruta: &Path) -> String {
    format!(
        "env_file = {}",
        toml::Value::String(ruta.display().to_string())
    )
}

/// Compone el `proveedores.toml` con lo elegido.
///
/// Se genera entero en vez de editarse: en la primera puesta en marcha no hay
/// nada que conservar, y un generador que además tuviera que respetar lo
/// escrito a mano sería un editor de TOML, que es mucho más de lo que hace
/// falta aquí.
pub fn componer_toml(elegidos: &[usize], por_defecto: Option<usize>) -> String {
    let mut t = String::from(
        "# Proveedores de vsioncode.\n\
         #\n\
         # Escrito por el asistente de puesta en marcha. Se puede editar a mano:\n\
         # cada [[proveedor]] es un servidor y cada [[proveedor.modelos]] uno de\n\
         # sus modelos, que aparecerán en /modelo.\n\
         #\n\
         # Las claves NO se guardan aquí, sino en el fichero de abajo, para que\n\
         # este pueda copiarse o enseñarse sin exponer nada.\n\
         #\n\
         # «esfuerzo» y «pensar» apagan el razonamiento del modelo: en una\n\
         # terminal es lo que más se nota. Cada familia entiende uno de los dos.\n\n",
    );

    t.push_str(&format!("{}\n", env_file_toml()));

    if let Some(i) = por_defecto {
        t.push_str(&format!(
            "\n# A quién se pregunta al arrancar.\npor_defecto = \"{}\"\n",
            SERVICIOS[i].id
        ));
    }

    for &i in elegidos {
        t.push_str(&bloque_de(&SERVICIOS[i]));
    }

    t
}

/// Añade a un TOML que ya existe lo que le falte, sin tocar lo demás.
///
/// Reescribir el fichero entero al volver por `/configurar` se llevaría por
/// delante lo que hubiera añadido a mano —otro servidor, otro modelo, una nota
/// suya—, y perder la configuración de alguien por abrir una ventana es
/// exactamente lo que no puede pasar. Así que se edita el texto: se añaden los
/// bloques que faltan al final, y `por_defecto` y `env_file` se actualizan en
/// su sitio o se crean si no estaban.
pub fn fusionar_toml(previo: &str, elegidos: &[usize], por_defecto: Option<usize>) -> String {
    let mut lineas: Vec<String> = previo.lines().map(str::to_string).collect();

    // `por_defecto` y `env_file` son claves de nivel superior: tienen que
    // quedar ANTES del primer [[proveedor]], o pasarían a ser campos suyos.
    let primer_bloque = lineas
        .iter()
        .position(|l| l.trim_start().starts_with("[["))
        .unwrap_or(lineas.len());

    let fijar = |clave: &str, valor: String, lineas: &mut Vec<String>, tope: &mut usize| {
        let prefijo = format!("{clave} =");
        match lineas
            .iter()
            .position(|l| l.trim_start().starts_with(&prefijo))
        {
            Some(i) => lineas[i] = valor,
            None => {
                // Con una línea en blanco detrás: pegada al primer
                // [[proveedor]] el fichero se lee como si la clave fuera suya,
                // aunque el analizador no se confunda.
                lineas.insert(*tope, String::new());
                lineas.insert(*tope, valor);
                *tope += 2;
            }
        }
    };

    let mut tope = primer_bloque;
    fijar("env_file", env_file_toml(), &mut lineas, &mut tope);
    if let Some(i) = por_defecto {
        fijar(
            "por_defecto",
            format!("por_defecto = \"{}\"", SERVICIOS[i].id),
            &mut lineas,
            &mut tope,
        );
    }

    let mut t = lineas.join("\n");
    if !t.ends_with('\n') {
        t.push('\n');
    }

    for &i in elegidos {
        let s = &SERVICIOS[i];
        // Ya declarado: no se duplica ni se pisa. Cambiar de clave se hace en
        // el fichero de claves, que es donde vive.
        if previo.contains(&format!("id = \"{}\"", s.id)) {
            continue;
        }
        t.push_str(&bloque_de(s));
    }
    t
}

/// El bloque `[[proveedor]]` de un servicio, con sus modelos.
fn bloque_de(s: &Servicio) -> String {
    let mut t = format!(
        "\n# {} — {}\n[[proveedor]]\nid = \"{}\"\n",
        s.nombre, s.url, s.id
    );
    if s.es_daemon() {
        t.push_str("tipo = \"daemon\"\n");
        return t;
    }
    t.push_str("tipo = \"openai\"\n");
    t.push_str(&format!("base_url = \"{}\"\n", s.base_url));
    if !s.variable.is_empty() {
        t.push_str(&format!("api_key = \"env:{}\"\n", s.variable));
    }
    t.push_str(&format!("modelo = \"{}\"\n", s.modelo));
    t.push_str(s.modelos);
    t
}

/// Escribe la configuración. Devuelve dónde ha quedado.
///
/// Si no había fichero, se genera entero; si lo había, se fusiona.
pub fn guardar_toml(elegidos: &[usize], por_defecto: Option<usize>) -> std::io::Result<PathBuf> {
    let ruta = crate::proveedores::ruta();
    if let Some(d) = ruta.parent() {
        // La misma carpeta donde vive el `.env`: si la crea este camino en vez
        // del otro, tiene que quedar igual de cerrada.
        crate::proveedores::crear_carpeta_privada(d)?;
    }
    let texto = match std::fs::read_to_string(&ruta) {
        Ok(previo) if !previo.trim().is_empty() => fusionar_toml(&previo, elegidos, por_defecto),
        _ => componer_toml(elegidos, por_defecto),
    };
    std::fs::write(&ruta, texto)?;
    Ok(ruta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_clave_pegada_mal_se_reconoce() {
        assert!(clave_plausible("").is_err());
        assert!(clave_plausible("   ").is_err());
        assert!(clave_plausible("https://console.groq.com/keys").is_err());
        assert!(clave_plausible("gsk_ abcdefghijklmnop").is_err());
        assert!(clave_plausible("corta").is_err());
        assert!(clave_plausible("gsk_0123456789abcdefghij").is_ok());
    }

    /// La clave no se enseña entera ni en la pantalla que la pide.
    #[test]
    fn la_clave_se_tapa_menos_el_final() {
        assert_eq!(tapada("gsk_0123456789"), "••••••••••6789");
        assert_eq!(tapada("abc"), "•••");
    }

    /// El TOML generado tiene que poder releerlo el propio lector.
    #[test]
    fn lo_que_escribe_se_puede_volver_a_leer() {
        let groq = SERVICIOS.iter().position(|s| s.id == "groq").unwrap();
        let claude = SERVICIOS.iter().position(|s| s.id == "claude").unwrap();
        let toml = componer_toml(&[claude, groq], Some(groq));

        let cfg: crate::proveedores::Config =
            toml::from_str(&toml).expect("el TOML generado debe ser válido");

        assert_eq!(cfg.por_defecto.as_deref(), Some("groq"));
        assert_eq!(cfg.proveedor.len(), 2);
        let p = cfg.proveedor.iter().find(|p| p.id == "groq").unwrap();
        assert_eq!(p.api_key, "env:GROQ_API_KEY");
        assert!(!p.modelos.is_empty(), "los modelos van al selector");
        assert!(cfg.proveedor.iter().any(|p| p.es_daemon()));
    }

    /// La clave nunca acaba en el fichero que se comparte.
    #[test]
    fn el_toml_no_contiene_ninguna_clave() {
        let todos: Vec<usize> = (0..SERVICIOS.len()).collect();
        let toml = componer_toml(&todos, Some(0));
        assert!(toml.contains("env:GROQ_API_KEY"));
        for s in SERVICIOS {
            if !s.variable.is_empty() {
                assert!(
                    !toml.contains(&format!("api_key = \"{}", s.variable)),
                    "la clave literal no puede aparecer"
                );
            }
        }
    }

    /// El fallo que solo se veía en Windows: en TOML lo que va entre comillas
    /// es una cadena **básica**, y ahí la barra invertida escapa. Con
    /// `env_file = "C:\Users\..."` el `\U` deja el fichero entero sin
    /// parsear, así que la puesta en marcha escribía una configuración que el
    /// propio programa no podía volver a leer.
    ///
    /// Se prueba desde aquí porque no hace falta Windows para escribir una
    /// ruta de Windows: hace falta no dar por hecho que todas las rutas se
    /// parecen a las de casa.
    #[test]
    fn una_ruta_de_windows_no_rompe_el_toml() {
        let ruta = Path::new(r"C:\Users\runneradmin\AppData\Roaming\vsioncode\.env");
        let linea = env_file_toml_de(ruta);

        let leido: toml::Table = linea
            .parse()
            .unwrap_or_else(|e| panic!("el TOML no se puede volver a leer: {e}\n{linea}"));
        assert_eq!(
            leido["env_file"].as_str().unwrap(),
            r"C:\Users\runneradmin\AppData\Roaming\vsioncode\.env",
            "la ruta tiene que volver entera"
        );
    }

    /// Y la de Linux sigue saliendo con comillas normales, que es lo que
    /// cualquiera espera ver al abrir el fichero.
    #[test]
    fn la_ruta_de_siempre_no_cambia_de_aspecto() {
        assert_eq!(
            env_file_toml_de(Path::new("/home/juan/.config/vsioncode/.env")),
            r#"env_file = "/home/juan/.config/vsioncode/.env""#
        );
    }

    /// Una carpeta propia por prueba: las que tocan disco corren en paralelo
    /// con las demás y no pueden pisarse el `.env`.
    fn carpeta_de_prueba(nombre: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vsion-clave-{}-{nombre}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// La clave no puede quedar en disco legible por nadie más: en Unix, con
    /// los permisos de la umask; en Windows, con la ACL que baja heredada de
    /// la carpeta de arriba. Es la misma garantía escrita de dos formas, así
    /// que la prueba es una y se lee del sistema, no de lo que creemos haber
    /// pedido.
    #[test]
    fn la_clave_se_guarda_cerrada_a_su_dueno() {
        let d = carpeta_de_prueba("nueva");
        let ruta = d.join(".env");
        guardar_clave_en(&ruta, "GROQ_API_KEY", "gsk_0123456789abcdefghij").unwrap();

        let cerrado = crate::privado::esta_cerrado(&ruta).unwrap();
        // En Unix además se sabe el número exacto, y conviene fijarlo: «nadie
        // más» se cumple igual con 0400, pero entonces guardar la clave
        // siguiente fallaría.
        #[cfg(unix)]
        let modo = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&ruta).unwrap().permissions().mode() & 0o777
        };
        let texto = std::fs::read_to_string(&ruta).unwrap();
        let _ = std::fs::remove_dir_all(&d);

        assert!(cerrado, "el .env solo lo lee su dueño");
        #[cfg(unix)]
        assert_eq!(modo, 0o600);
        assert!(texto.contains("GROQ_API_KEY=gsk_0123456789abcdefghij"));
    }

    /// Y la carpeta que lo guarda, igual: con ella abierta se ve qué
    /// proveedores hay configurados y cuándo se tocaron las claves.
    #[test]
    fn la_carpeta_de_la_configuracion_tambien_queda_cerrada() {
        let d = carpeta_de_prueba("carpeta").join("vsioncode");
        crate::proveedores::crear_carpeta_privada(&d).unwrap();

        let cerrada = crate::privado::esta_cerrado(&d).unwrap();
        let _ = std::fs::remove_dir_all(d.parent().unwrap());

        assert!(cerrada, "la carpeta de la configuración es de su dueño");
    }

    /// Un `.env` que ya estaba abierto queda cerrado al guardar en él. El
    /// caso importa porque es el común: la primera clave crea el fichero y la
    /// segunda lo encuentra hecho.
    #[test]
    fn un_env_que_ya_estaba_queda_cerrado() {
        let d = carpeta_de_prueba("reabierto");
        let ruta = d.join(".env");
        std::fs::write(&ruta, "# claves\nOTRA_CLAVE=valor_de_antes\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ruta, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        guardar_clave_en(&ruta, "GROQ_API_KEY", "gsk_0123456789abcdefghij").unwrap();

        let cerrado = crate::privado::esta_cerrado(&ruta).unwrap();
        let texto = std::fs::read_to_string(&ruta).unwrap();
        let _ = std::fs::remove_dir_all(&d);

        assert!(
            cerrado,
            "un .env que ya estaba abierto se cierra al tocarlo"
        );
        assert!(texto.contains("OTRA_CLAVE=valor_de_antes"));
        assert!(texto.contains("GROQ_API_KEY=gsk_0123456789abcdefghij"));
    }

    /// Con un `.env` que ya estaba, los permisos se aprietan **antes** de
    /// escribir. Si se arreglan después, la clave nueva ya ha pasado por disco
    /// con los permisos viejos — y si el fichero estaba en solo lectura, ni
    /// siquiera se llega a guardar.
    #[test]
    #[cfg(unix)]
    fn a_un_env_ya_existente_se_le_aprietan_los_permisos_antes_de_escribir() {
        use std::os::unix::fs::PermissionsExt;

        let d = carpeta_de_prueba("existente");
        let ruta = d.join(".env");
        std::fs::write(&ruta, "# claves\nOTRA_CLAVE=valor_de_antes\n").unwrap();
        std::fs::set_permissions(&ruta, std::fs::Permissions::from_mode(0o400)).unwrap();

        let r = guardar_clave_en(&ruta, "GROQ_API_KEY", "gsk_0123456789abcdefghij");

        let modo = std::fs::metadata(&ruta).unwrap().permissions().mode() & 0o777;
        let texto = std::fs::read_to_string(&ruta).unwrap_or_default();
        let _ = std::fs::set_permissions(&ruta, std::fs::Permissions::from_mode(0o600));
        let _ = std::fs::remove_dir_all(&d);

        assert!(r.is_ok(), "no se pudo guardar: {r:?}");
        assert_eq!(modo, 0o600);
        assert!(texto.contains("GROQ_API_KEY=gsk_0123456789abcdefghij"));
        assert!(
            texto.contains("OTRA_CLAVE=valor_de_antes"),
            "las demás claves siguen ahí"
        );
    }

    /// El clic tiene que caer donde se ve el enlace, no una línea al lado.
    #[test]
    fn el_clic_cae_donde_esta_lo_que_se_ve() {
        // Con el primero señalado: filas 2 y 3 son su nombre y su nota, y la 4
        // su dirección.
        assert_eq!(clic_en(0, 0), Clic::Nada);
        assert_eq!(clic_en(0, 2), Clic::Elegir(0));
        assert_eq!(clic_en(0, 3), Clic::Elegir(0));
        assert_eq!(clic_en(0, 4), Clic::Abrir);
        // Debajo del enlace todo va corrido una fila.
        assert_eq!(clic_en(0, 5), Clic::Elegir(1));
        assert_eq!(clic_en(0, 6), Clic::Elegir(1));
        assert_eq!(clic_en(0, 7), Clic::Elegir(2));
        // Y con otro señalado, su enlace se mueve con él.
        assert_eq!(clic_en(2, fila_url(2)), Clic::Abrir);
        assert_eq!(clic_en(2, 2), Clic::Elegir(0));
    }

    /// Volver a la ventana con algo ya configurado no puede borrarlo.
    #[test]
    fn fusionar_conserva_lo_que_ya_habia() {
        let claude = SERVICIOS.iter().position(|s| s.id == "claude").unwrap();
        let groq = SERVICIOS.iter().position(|s| s.id == "groq").unwrap();

        let previo = componer_toml(&[claude], Some(claude));
        let fusionado = fusionar_toml(&previo, &[claude, groq], Some(groq));

        let cfg: crate::proveedores::Config = toml::from_str(&fusionado).unwrap();
        assert_eq!(cfg.proveedor.len(), 2, "se añade sin duplicar");
        assert!(cfg.proveedor.iter().any(|p| p.id == "claude"));
        assert_eq!(
            cfg.por_defecto.as_deref(),
            Some("groq"),
            "se actualiza en su sitio"
        );
        assert_eq!(fusionado.matches("por_defecto =").count(), 1);
    }

    /// Un servidor añadido a mano sobrevive a pasar por la ventana.
    #[test]
    fn fusionar_respeta_lo_escrito_a_mano() {
        let previo = "por_defecto = \"mio\"\n\n[[proveedor]]\nid = \"mio\"\ntipo = \"openai\"\n\
                      base_url = \"http://localhost:8080/v1\"\nmodelo = \"lo-que-sea\"\n";
        let groq = SERVICIOS.iter().position(|s| s.id == "groq").unwrap();
        let fusionado = fusionar_toml(previo, &[groq], Some(groq));

        let cfg: crate::proveedores::Config = toml::from_str(&fusionado).unwrap();
        assert!(cfg.proveedor.iter().any(|p| p.id == "mio"), "no se pierde");
        assert!(cfg.proveedor.iter().any(|p| p.id == "groq"));
        assert!(fusionado.contains("env_file ="), "se añade lo que faltaba");
    }

    #[test]
    fn la_lista_se_recorre_en_circulo() {
        let mut b = Bienvenida::nueva();
        b.sel = 0;
        b.mover(-1);
        assert_eq!(b.sel, SERVICIOS.len(), "hacia atrás cae en «terminar»");
        b.mover(1);
        assert_eq!(b.sel, 0);
    }

    /// El proceso local es el único sin clave, y hay que poder distinguirlo.
    #[test]
    fn solo_claude_es_daemon() {
        for s in SERVICIOS {
            assert_eq!(s.es_daemon(), s.id == "claude", "{}", s.id);
        }
    }
}
