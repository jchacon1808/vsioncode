//! De dónde salen los modelos.
//!
//! Un fichero TOML declara a quién se le puede preguntar. Lo importante del
//! diseño está en dos decisiones:
//!
//! **Las claves no se guardan aquí.** Se escribe `api_key = "env:GROQ_API_KEY"`
//! y se lee de donde estén. Un fichero de configuración acaba en una copia de
//! seguridad, en un repositorio o en una captura de pantalla; una clave dentro
//! de él, también.
//!
//! **Casi todos hablan el mismo dialecto.** Groq, NVIDIA, Gemini, OpenRouter,
//! Cerebras, DeepSeek, Ollama y llama.cpp exponen `/v1/chat/completions` de
//! OpenAI. Cambia la URL y el modelo; el código es el mismo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ai::http::Proveedor;

/// El fichero, tal y como se lee.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// A quién se pregunta si no se dice otra cosa.
    #[serde(default)]
    pub por_defecto: Option<String>,
    /// Fichero de variables de entorno del que sacar las claves.
    ///
    /// Existe porque las claves suelen vivir en un `.env` que nadie exporta a
    /// la sesión, y obligar a `export` antes de arrancar es una fricción que
    /// se paga cada día.
    #[serde(default)]
    pub env_file: Option<String>,
    #[serde(default)]
    pub proveedor: Vec<ProveedorToml>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProveedorToml {
    pub id: String,
    /// `daemon` (el proceso `claude`) u `openai` (cualquier HTTP compatible).
    #[serde(default = "tipo_openai")]
    pub tipo: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    /// Con cuál se arranca. Si está vacío, el primero de `modelos`.
    #[serde(default)]
    pub modelo: String,
    /// Los que se ofrecen en el menú.
    ///
    /// Un proveedor **no es un modelo**: Groq sirve cinco y NVIDIA más de
    /// cien. Mientras la configuración solo dejaba declarar uno por servidor,
    /// tener varios obligaba a repetir el bloque entero —con su URL y su
    /// clave— por cada modelo que quisieras probar.
    #[serde(default)]
    pub modelos: Vec<ModeloToml>,
}

/// Un modelo concreto de un proveedor.
#[derive(Debug, Clone, Deserialize)]
pub struct ModeloToml {
    /// Tal y como lo espera el proveedor: `openai/gpt-oss-120b`.
    pub id: String,
    /// Para qué va bien, en pocas palabras. Se enseña al elegir.
    #[serde(default)]
    pub nota: String,
    /// `reasoning_effort`, si este modelo lo admite. Ver [`Proveedor::esfuerzo`].
    #[serde(default)]
    pub esfuerzo: Option<String>,
    /// `enable_thinking`, el interruptor de los Nemotron.
    #[serde(default)]
    pub pensar: Option<bool>,
    /// Si sabe mirar imágenes.
    ///
    /// Se declara y no se adivina: `qwen3.6-27b` las ve y `gpt-oss-120b` no, y
    /// nada en sus identificadores lo dice. Suponerlo daría avisos falsos, que
    /// estorban más que no avisar.
    #[serde(default)]
    pub imagenes: bool,
}

/// Modelos que miran imágenes aunque el fichero no lo diga.
///
/// Existe porque la configuración del usuario **envejece en silencio**: se
/// escribe una vez y ahí se queda, así que cada campo nuevo nace apagado en
/// los ficheros que ya existían. Pasó con este: `imagenes` llegó después de
/// que el fichero estuviera escrito, y el resultado fue que pegar una imagen
/// no funcionaba con ningún modelo y no había forma de saber por qué.
///
/// Por familia y no por identificador exacto, que cambian cada pocos meses.
/// El TOML sigue mandando: esto solo cubre lo que no declara.
fn mira_imagenes_por_defecto(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    const FAMILIAS: &[&str] = &[
        "gemini",
        "qwen3.6",
        "qwen3-vl",
        "qwen2.5-vl",
        "gpt-4o",
        "gpt-5",
        "claude",
        "sonnet",
        "opus",
        "haiku",
        "pixtral",
        "-vl",
        "vision",
        "llava",
    ];
    FAMILIAS.iter().any(|f| id.contains(f))
}

impl ModeloToml {
    /// ¿Sabe mirar una imagen?
    ///
    /// Lo que diga el fichero manda; si no dice nada, se mira la familia. Así
    /// una configuración escrita hace meses no deja la función apagada sin
    /// que nadie se entere.
    pub fn ve_imagenes(&self) -> bool {
        self.imagenes || mira_imagenes_por_defecto(&self.id)
    }

    /// Para qué va bien, en pocas palabras.
    ///
    /// Lo que diga el fichero manda. Si no dice nada y el identificador es de
    /// la casa, la descripción de serie: una configuración que solo declara
    /// `id = "claude-haiku-4-5-20251001"` dejaría la columna de notas en
    /// blanco, y una lista sin notas vuelve a ser una lista de nombres que hay
    /// que saberse.
    pub fn nota_util(&self) -> String {
        if !self.nota.is_empty() {
            return self.nota.clone();
        }
        crate::agente::reconocer(&self.id)
            .and_then(|alias| crate::slash::MODELOS.iter().find(|(n, _)| *n == alias))
            .map(|(_, d)| (*d).to_string())
            .unwrap_or_default()
    }

    /// Cómo se le llama en la interfaz: sin el prefijo del fabricante.
    ///
    /// `openai/gpt-oss-120b` se enseña como `gpt-oss-120b`. El prefijo lo
    /// necesita la API, no la persona que elige, y en una lista agrupada por
    /// servidor solo añade ruido repetido.
    pub fn corto(&self) -> &str {
        self.id.rsplit('/').next().unwrap_or(&self.id)
    }
}

fn tipo_openai() -> String {
    "openai".into()
}

impl ProveedorToml {
    pub fn es_daemon(&self) -> bool {
        self.tipo == "daemon"
    }

    /// Resuelve la clave: literal, o de una variable de entorno con `env:`.
    pub fn clave(&self, entorno: &HashMap<String, String>) -> String {
        match self.api_key.strip_prefix("env:") {
            Some(nombre) => entorno
                .get(nombre)
                .cloned()
                .or_else(|| std::env::var(nombre).ok())
                .unwrap_or_default(),
            None => self.api_key.clone(),
        }
    }

    /// Con qué modelo se arranca este proveedor.
    ///
    /// El campo `modelo` manda; si no lo hay, el primero de la lista. Así una
    /// configuración que solo declara `modelos` funciona sin repetir el
    /// preferido en dos sitios.
    pub fn por_defecto(&self) -> Option<&ModeloToml> {
        if !self.modelo.is_empty()
            && let Some(m) = self.modelos.iter().find(|m| m.id == self.modelo)
        {
            return Some(m);
        }
        self.modelos.first()
    }

    /// Todos los que ofrece, ya sea por lista o por el campo suelto.
    ///
    /// Devuelve algo prestado cuando hay lista y algo construido cuando solo
    /// hay `modelo`, para que una configuración antigua —un servidor, un
    /// modelo— siga funcionando sin tocarla.
    pub fn ofrecidos(&self) -> Vec<ModeloToml> {
        if !self.modelos.is_empty() {
            return self.modelos.clone();
        }
        if self.modelo.is_empty() {
            return Vec::new();
        }
        vec![ModeloToml {
            id: self.modelo.clone(),
            nota: String::new(),
            esfuerzo: None,
            pensar: None,
            imagenes: false,
        }]
    }

    /// Los que se enseñan al elegir: lo declarado y, para el daemon, los alias
    /// de serie si no ha declarado ninguno.
    ///
    /// El respaldo es solo de `claude` y solo cuando el fichero calla: esos
    /// tres alias los entiende el propio proceso, así que valen siempre, y sin
    /// ellos la primera vez —cuando aún no hay TOML— no habría nada que elegir.
    /// En cuanto el usuario escribe sus modelos de claude, mandan los suyos.
    ///
    /// Antes no: eran siempre los tres del binario. Declarar un modelo de
    /// claude en el fichero no cambiaba nada de lo que se ve, que es el fallo
    /// que ya se pagó con los proveedores —una opción que la interfaz no
    /// enseña no existe para quien la usa—, repetido un nivel más abajo.
    pub fn a_elegir(&self) -> Vec<ModeloToml> {
        let v = self.ofrecidos();
        if !v.is_empty() || !self.es_daemon() {
            return v;
        }
        crate::slash::MODELOS
            .iter()
            .map(|(alias, nota)| ModeloToml {
                id: (*alias).to_string(),
                nota: (*nota).to_string(),
                esfuerzo: None,
                pensar: None,
                imagenes: false,
            })
            .collect()
    }

    pub fn a_proveedor(&self, entorno: &HashMap<String, String>) -> Proveedor {
        let m = self.por_defecto().cloned();
        Proveedor {
            id: self.id.clone(),
            base_url: self.base_url.trim_end_matches('/').to_string(),
            api_key: self.clave(entorno),
            modelo: m
                .as_ref()
                .map(|m| m.id.clone())
                .unwrap_or_else(|| self.modelo.clone()),
            esfuerzo: m.as_ref().and_then(|m| m.esfuerzo.clone()),
            pensar: m.as_ref().and_then(|m| m.pensar),
        }
    }

    /// El mismo proveedor pero apuntando a uno de sus modelos.
    pub fn con_modelo(&self, entorno: &HashMap<String, String>, m: &ModeloToml) -> Proveedor {
        Proveedor {
            id: self.id.clone(),
            base_url: self.base_url.trim_end_matches('/').to_string(),
            api_key: self.clave(entorno),
            modelo: m.id.clone(),
            esfuerzo: m.esfuerzo.clone(),
            pensar: m.pensar,
        }
    }
}

/// Dónde vive la configuración.
pub fn ruta() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("vsioncode/proveedores.toml")
}

/// Lee un fichero de variables de entorno.
///
/// Deliberadamente simple: `CLAVE=valor` por línea, comillas quitadas,
/// comentarios ignorados. No es un intérprete de shell y no debe serlo — un
/// fichero de claves no es sitio para ejecutar nada.
pub fn leer_env(ruta: &Path) -> HashMap<String, String> {
    let Ok(texto) = std::fs::read_to_string(ruta) else {
        return HashMap::new();
    };
    texto
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=')?;
            Some((
                k.trim().to_string(),
                v.trim().trim_matches(['"', '\'']).to_string(),
            ))
        })
        .collect()
}

/// Carga la configuración, o devuelve una vacía si no existe.
pub fn cargar() -> (Config, HashMap<String, String>) {
    let cfg: Config = std::fs::read_to_string(ruta())
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();

    let entorno = cfg
        .env_file
        .as_ref()
        .map(|f| leer_env(&expandir(f)))
        .unwrap_or_default();

    (cfg, entorno)
}

/// Deja la configuración del cerebro apuntando a `por_defecto`.
///
/// Existe porque el campo se leía, se documentaba en el fichero de ejemplo
/// —«a quién se pregunta al arrancar»— y no lo miraba nadie: escribieras lo
/// que escribieras, vsioncode arrancaba con `claude`. Configurar un proveedor
/// no cambiaba nada hasta que lo pedías a mano con `/modelo`.
///
/// Devuelve un aviso cuando hay algo que contar. No se escribe en `stderr`:
/// para cuando esto corre, la pantalla ya es de la TUI y ahí un `eprintln!`
/// se pierde sin que nadie lo lea.
pub fn aplicar_por_defecto(
    cfg: &mut crate::ai::BrainConfig,
    catalogo: &Config,
    entorno: &HashMap<String, String>,
) -> Option<String> {
    let id = catalogo.por_defecto.as_deref()?.trim();
    if id.is_empty() {
        return None;
    }

    let Some(p) = catalogo
        .proveedor
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
    else {
        return Some(format!(
            "Tu proveedor por defecto es «{id}», pero no está declarado en {}. \
             Arranco con claude.",
            ruta().display()
        ));
    };

    if p.es_daemon() {
        cfg.proveedor = None;
        return None;
    }

    let prov = p.a_proveedor(entorno);
    // Un modelo local no necesita clave, y exigírsela sería negarse a arrancar
    // contra algo que funciona.
    if prov.api_key.is_empty() && !prov.base_url.contains("localhost") {
        return Some(format!(
            "«{id}» es tu proveedor por defecto, pero no encuentro su clave en {}. \
             Arranco con claude; /modelo {id} cuando la tengas.",
            catalogo.env_file.as_deref().unwrap_or("el entorno")
        ));
    }

    let aviso = format!("Preguntando a «{}» con {}.", prov.id, prov.modelo);
    cfg.proveedor = Some(prov);
    Some(aviso)
}

/// Expande `~` al directorio del usuario.
fn expandir(ruta: &str) -> PathBuf {
    match ruta.strip_prefix("~/") {
        Some(resto) => dirs::home_dir().unwrap_or_default().join(resto),
        None => PathBuf::from(ruta),
    }
}

/// Escribe un fichero de ejemplo si no hay ninguno.
///
/// Con los proveedores reales dentro y comentados, porque una plantilla vacía
/// obliga a buscar en internet las URL base — que es justo el trabajo que esto
/// debería ahorrar.
pub fn crear_ejemplo() -> std::io::Result<PathBuf> {
    let p = ruta();
    if p.exists() {
        return Ok(p);
    }
    if let Some(d) = p.parent() {
        crear_carpeta_privada(d)?;
    }
    std::fs::write(&p, EJEMPLO)?;
    Ok(p)
}

/// Crea la carpeta de la configuración cerrada a su dueño.
///
/// Vive aquí, y no donde se escriben las claves, porque la carpeta es de este
/// módulo: `ruta()` y el `.env` que declara `env_file` caen los dos dentro, y
/// hay tres caminos que pueden crearla —la puesta en marcha, guardar una
/// clave y este fichero de ejemplo—. Con la creación repartida, los permisos
/// acababan dependiendo de quién llegara primero, y una garantía que depende
/// del orden no es una garantía.
///
/// El `.env` está a 600, pero con la carpeta abierta se sigue viendo qué
/// proveedores hay configurados y cuándo se tocaron las claves. Cerrarla no
/// cuesta nada y el dueño no pierde nada: el `proveedores.toml` sigue siendo
/// suyo para copiarlo o enseñarlo.
///
/// Solo afecta a lo que se crea aquí; una carpeta que ya existía se respeta
/// tal cual, que sus permisos son decisión de quien la hizo.
pub(crate) fn crear_carpeta_privada(d: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(d)
    }
    // En Windows los permisos no se pueden dar al crear: la carpeta nace con
    // lo que herede de la de arriba y hay que cerrarla justo después. La
    // ventana existe, y no se puede cerrar más —no hay un `mode()` que valga—;
    // lo que sí se puede es que dentro de ella todavía no haya nada.
    #[cfg(windows)]
    {
        let nueva = !d.exists();
        std::fs::create_dir_all(d)?;
        if nueva {
            crate::privado::cerrar(d)?;
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::create_dir_all(d)
    }
}

pub const EJEMPLO: &str = r#"# Proveedores de vsioncode.
#
# Las claves NO se ponen aquí. Se escribe env:NOMBRE y se leen de donde estén:
# de la sesión, o del fichero que indique env_file. Un fichero de configuración
# acaba en una copia de seguridad o en una captura de pantalla; una clave
# dentro de él, también.
#
# Los tiempos que van en cada «nota» son el tiempo hasta la primera palabra,
# medido en esta máquina con este mismo cliente. En una terminal es lo que más
# se nota: un modelo que piensa diez segundos antes de escribir estorba por
# listo que sea.
#
# «imagenes = true» marca los que saben mirar una imagen que sueltes en la
# conversación. Se declara y no se adivina: qwen3.6 las ve y gpt-oss no, y nada
# en sus nombres lo dice.
#
# «esfuerzo» y «pensar» son los dos interruptores del razonamiento. Casi todos
# los modelos nuevos razonan por defecto y eso se paga en cada turno: apagarlo
# bajó qwen3.6 de 972 ms a 165, y gemini-3.6-flash de 3,7 s a 1,0 s. Cada
# familia entiende uno solo de los dos, así que no se mezclan.

# Fichero de donde sacar las claves (opcional).
env_file = "~/.coupset/.env"

# A quién se pregunta al arrancar.
por_defecto = "claude"

# El proceso `claude` local. Usa tu suscripción, sin clave de API.
[[proveedor]]
id = "claude"
tipo = "daemon"

# Groq. El más rápido con diferencia. https://console.groq.com
# Gratis: 30 peticiones/minuto, 1000 al día.
[[proveedor]]
id = "groq"
tipo = "openai"
base_url = "https://api.groq.com/openai/v1"
api_key = "env:GROQ_API_KEY"
modelo = "openai/gpt-oss-120b"

  [[proveedor.modelos]]
  id = "openai/gpt-oss-120b"
  nota = "equilibrado — 642 ms"
  esfuerzo = "low"          # los gpt-oss no saben apagarlo del todo

  [[proveedor.modelos]]
  id = "openai/gpt-oss-20b"
  nota = "más pequeño y suelto — 1,0 s"
  esfuerzo = "low"

  [[proveedor.modelos]]
  id = "qwen/qwen3.6-27b"
  nota = "el más rápido de todos, y programa bien — 165 ms"
  esfuerzo = "none"
  imagenes = true

  [[proveedor.modelos]]
  id = "groq/compound"
  nota = "busca en internet por su cuenta — 1,6 s"

  [[proveedor.modelos]]
  id = "groq/compound-mini"
  nota = "lo mismo, más ligero — 1,3 s"

# NVIDIA NIM. Catálogo enorme. https://build.nvidia.com
# Gratis: ~40 peticiones/minuto. Duerme los modelos poco usados, así que la
# primera pregunta a uno frío puede tardar bastante más de lo que dice la nota.
[[proveedor]]
id = "nvidia"
tipo = "openai"
base_url = "https://integrate.api.nvidia.com/v1"
api_key = "env:NVIDIA_API_KEY"
modelo = "nvidia/nemotron-3-super-120b-a12b"

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
  pensar = false            # los Nemotron usan este interruptor, no «esfuerzo»

  [[proveedor.modelos]]
  id = "nvidia/nemotron-3-ultra-550b-a55b"
  nota = "el más capaz del catálogo — 1,8 s"
  esfuerzo = "none"

  [[proveedor.modelos]]
  id = "nvidia/nemotron-3-nano-30b-a3b"
  nota = "el más pequeño — 5,6 s"
  pensar = false

  [[proveedor.modelos]]
  id = "moonshotai/kimi-k3"
  nota = "contexto largo, tarda en arrancar — 7,7 s"

  [[proveedor.modelos]]
  id = "meta/llama-3.3-70b-instruct"
  nota = "conocido y estable, pero lento aquí — 15,3 s"

# Google AI Studio. https://aistudio.google.com
# El «esfuerzo» es aquí especialmente importante: sin él, estos modelos razonan
# a nivel medio en cada turno y tardan segundos en abrir la boca.
[[proveedor]]
id = "gemini"
tipo = "openai"
base_url = "https://generativelanguage.googleapis.com/v1beta/openai"
api_key = "env:GEMINI_API_KEY"
modelo = "gemini-3.5-flash-lite"

  [[proveedor.modelos]]
  id = "gemini-3.5-flash-lite"
  nota = "el más rápido de Google — 626 ms"
  esfuerzo = "minimal"
  imagenes = true

  [[proveedor.modelos]]
  id = "gemini-3.6-flash"
  nota = "el más capaz de los rápidos — 1,0 s"
  esfuerzo = "minimal"
  imagenes = true

  [[proveedor.modelos]]
  id = "gemini-3.5-flash"
  nota = "la generación anterior — 1,1 s"
  esfuerzo = "minimal"
  imagenes = true

# Un modelo local con ollama. Sin clave y sin red.
# [[proveedor]]
# id = "local"
# tipo = "openai"
# base_url = "http://localhost:11434/v1"
# api_key = ""
# modelo = "qwen2.5-coder:7b"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(t: &str) -> Config {
        toml::from_str(t).expect("toml válido")
    }

    /// El fallo que costó que pegar imágenes no funcionara con nada: el campo
    /// llegó después de que los ficheros estuvieran escritos, y nació apagado
    /// en todos ellos sin que nadie pudiera saberlo.
    #[test]
    fn un_modelo_conocido_ve_imagenes_aunque_el_fichero_no_lo_diga() {
        let m = |id: &str| ModeloToml {
            id: id.into(),
            nota: String::new(),
            esfuerzo: None,
            pensar: None,
            imagenes: false,
        };
        for si in [
            "gemini-3.5-flash-lite",
            "qwen/qwen3.6-27b",
            "claude-sonnet-5",
        ] {
            assert!(m(si).ve_imagenes(), "«{si}» debería mirar imágenes");
        }
        for no in ["openai/gpt-oss-120b", "meta/llama-3.3-70b-instruct"] {
            assert!(!m(no).ve_imagenes(), "«{no}» no mira imágenes");
        }
    }

    /// Y lo que diga el fichero manda sobre la lista.
    #[test]
    fn el_fichero_manda_sobre_lo_conocido() {
        let m = ModeloToml {
            id: "modelo-raro-que-si-ve".into(),
            nota: String::new(),
            esfuerzo: None,
            pensar: None,
            imagenes: true,
        };
        assert!(m.ve_imagenes());
    }

    #[test]
    fn lee_un_proveedor() {
        let c = cfg(r#"
            por_defecto = "groq"
            [[proveedor]]
            id = "groq"
            base_url = "https://api.groq.com/openai/v1"
            api_key = "env:GROQ_API_KEY"
            modelo = "openai/gpt-oss-120b"
        "#);
        assert_eq!(c.por_defecto.as_deref(), Some("groq"));
        assert_eq!(c.proveedor.len(), 1);
        assert_eq!(c.proveedor[0].tipo, "openai", "el tipo por defecto");
    }

    /// El respaldo de serie es solo del daemon, y solo cuando el fichero calla.
    ///
    /// Lo que arreglaba: los tres alias del binario se enseñaban siempre, así
    /// que escribir tus modelos de claude en el TOML no cambiaba ni el menú ni
    /// la ventana. Y al revés importa igual: un servidor HTTP sin modelos no
    /// puede heredar los alias de claude, que ahí no significan nada.
    #[test]
    fn los_alias_de_serie_solo_entran_si_claude_no_declara_nada() {
        let daemon = |modelos: Vec<ModeloToml>| ProveedorToml {
            id: "claude".into(),
            tipo: "daemon".into(),
            base_url: String::new(),
            api_key: String::new(),
            modelo: String::new(),
            modelos,
        };

        assert_eq!(
            daemon(Vec::new()).a_elegir().len(),
            crate::slash::MODELOS.len(),
            "sin declarar nada, los de serie"
        );

        let mio = daemon(vec![ModeloToml {
            id: "claude-haiku-4-5-20251001".into(),
            nota: "el que uso al probar".into(),
            esfuerzo: None,
            pensar: None,
            imagenes: false,
        }]);
        let suyos = mio.a_elegir();
        assert_eq!(suyos.len(), 1, "lo declarado manda: {suyos:?}");
        assert_eq!(suyos[0].id, "claude-haiku-4-5-20251001");

        let http = ProveedorToml {
            id: "groq".into(),
            tipo: "openai".into(),
            base_url: "x".into(),
            api_key: String::new(),
            modelo: String::new(),
            modelos: Vec::new(),
        };
        assert!(http.a_elegir().is_empty(), "un HTTP no hereda a claude");
    }

    /// La clave nunca se guarda en la configuración: se dice dónde está.
    #[test]
    fn saca_la_clave_del_entorno() {
        let p = ProveedorToml {
            id: "groq".into(),
            tipo: "openai".into(),
            base_url: "x".into(),
            api_key: "env:MI_CLAVE".into(),
            modelo: "m".into(),
            modelos: Vec::new(),
        };
        let env = HashMap::from([("MI_CLAVE".to_string(), "secreta".to_string())]);
        assert_eq!(p.clave(&env), "secreta");
    }

    #[test]
    fn una_clave_literal_tambien_vale() {
        let p = ProveedorToml {
            id: "x".into(),
            tipo: "openai".into(),
            base_url: "x".into(),
            api_key: "directa".into(),
            modelo: "m".into(),
            modelos: Vec::new(),
        };
        assert_eq!(p.clave(&HashMap::new()), "directa");
    }

    #[test]
    fn si_falta_la_variable_la_clave_queda_vacia() {
        let p = ProveedorToml {
            id: "x".into(),
            tipo: "openai".into(),
            base_url: "x".into(),
            api_key: "env:NO_EXISTE_ESTA_VARIABLE_12345".into(),
            modelo: "m".into(),
            modelos: Vec::new(),
        };
        assert!(p.clave(&HashMap::new()).is_empty());
    }

    #[test]
    fn la_barra_final_de_la_url_no_duplica_la_ruta() {
        let p = ProveedorToml {
            id: "x".into(),
            tipo: "openai".into(),
            base_url: "https://api.groq.com/openai/v1/".into(),
            api_key: String::new(),
            modelo: "m".into(),
            modelos: Vec::new(),
        };
        assert_eq!(
            p.a_proveedor(&HashMap::new()).base_url,
            "https://api.groq.com/openai/v1"
        );
    }

    #[test]
    fn lee_un_fichero_de_variables() {
        let d = std::env::temp_dir().join(format!("vsion-env-{}", std::process::id()));
        std::fs::write(
            &d,
            "# comentario\nGROQ_API_KEY=gsk_abc\nexport OTRA=\"con comillas\"\n\nVACIA=\n",
        )
        .unwrap();
        let e = leer_env(&d);
        assert_eq!(e.get("GROQ_API_KEY").unwrap(), "gsk_abc");
        assert_eq!(e.get("OTRA").unwrap(), "con comillas", "quita las comillas");
        assert_eq!(e.get("VACIA").unwrap(), "");
        let _ = std::fs::remove_file(&d);
    }

    #[test]
    fn un_fichero_que_no_existe_no_revienta() {
        assert!(leer_env(Path::new("/no/existe/jamas")).is_empty());
    }

    fn brain() -> crate::ai::BrainConfig {
        crate::ai::BrainConfig::default()
    }

    /// El campo existía y no lo leía nadie: se configuraba un proveedor y
    /// vsioncode seguía arrancando con claude.
    #[test]
    fn el_proveedor_por_defecto_se_aplica_al_arrancar() {
        let c = cfg(r#"
            por_defecto = "groq"
            [[proveedor]]
            id = "groq"
            base_url = "https://api.groq.com/openai/v1"
            api_key = "env:MI_CLAVE"
            modelo = "openai/gpt-oss-120b"
        "#);
        let env = HashMap::from([("MI_CLAVE".to_string(), "secreta".to_string())]);
        let mut b = brain();
        aplicar_por_defecto(&mut b, &c, &env);
        let p = b.proveedor.expect("debería quedar apuntando a groq");
        assert_eq!(p.id, "groq");
        assert_eq!(p.modelo, "openai/gpt-oss-120b");
    }

    /// El daemon es la ausencia de proveedor HTTP, no un proveedor más.
    #[test]
    fn por_defecto_claude_deja_el_daemon() {
        let c = cfg(r#"
            por_defecto = "claude"
            [[proveedor]]
            id = "claude"
            tipo = "daemon"
        "#);
        let mut b = brain();
        b.proveedor = Some(Proveedor {
            id: "otro".into(),
            base_url: "x".into(),
            api_key: "k".into(),
            modelo: "m".into(),
            esfuerzo: None,
            pensar: None,
        });
        aplicar_por_defecto(&mut b, &c, &HashMap::new());
        assert!(b.proveedor.is_none());
    }

    /// Sin clave no se arranca contra él: fallaría en el primer turno, que es
    /// mucho peor que decirlo ahora.
    #[test]
    fn sin_clave_no_se_arranca_con_el_y_se_avisa() {
        let c = cfg(r#"
            por_defecto = "groq"
            [[proveedor]]
            id = "groq"
            base_url = "https://api.groq.com/openai/v1"
            api_key = "env:NO_EXISTE_ESTA_VARIABLE_12345"
            modelo = "m"
        "#);
        let mut b = brain();
        let aviso = aplicar_por_defecto(&mut b, &c, &HashMap::new());
        assert!(b.proveedor.is_none(), "no debe arrancar sin clave");
        assert!(aviso.unwrap().contains("clave"));
    }

    /// Un local sí arranca sin clave: no la necesita.
    #[test]
    fn un_modelo_local_no_necesita_clave() {
        let c = cfg(r#"
            por_defecto = "local"
            [[proveedor]]
            id = "local"
            base_url = "http://localhost:11434/v1"
            api_key = ""
            modelo = "qwen2.5-coder:7b"
        "#);
        let mut b = brain();
        aplicar_por_defecto(&mut b, &c, &HashMap::new());
        assert!(b.proveedor.is_some());
    }

    #[test]
    fn un_por_defecto_que_no_existe_se_dice_en_vez_de_callarse() {
        let c = cfg(r#"por_defecto = "fantasma""#);
        let mut b = brain();
        let aviso = aplicar_por_defecto(&mut b, &c, &HashMap::new());
        assert!(b.proveedor.is_none());
        assert!(aviso.unwrap().contains("fantasma"));
    }

    #[test]
    fn sin_por_defecto_no_se_toca_nada() {
        let c = cfg("");
        let mut b = brain();
        assert!(aplicar_por_defecto(&mut b, &c, &HashMap::new()).is_none());
        assert!(b.proveedor.is_none());
    }

    /// El ejemplo que se le escribe al usuario tiene que ser TOML válido: si
    /// no, la primera experiencia con la configuración es un error de sintaxis
    /// que él no ha escrito.
    #[test]
    fn el_ejemplo_es_valido_y_tiene_lo_que_promete() {
        let c: Config = toml::from_str(EJEMPLO).expect("el ejemplo debe parsear");
        let ids: Vec<&str> = c.proveedor.iter().map(|p| p.id.as_str()).collect();
        for esperado in ["claude", "groq", "nvidia", "gemini"] {
            assert!(ids.contains(&esperado), "falta {esperado} en el ejemplo");
        }
        assert!(c.proveedor.iter().any(|p| p.es_daemon()));
        // Y ninguna clave literal: todas por variable de entorno.
        for p in &c.proveedor {
            assert!(
                p.api_key.is_empty() || p.api_key.starts_with("env:"),
                "«{}» trae una clave escrita en el fichero",
                p.id
            );
        }
    }

    /// La carpeta de la configuración se crea cerrada, venga por donde venga.
    ///
    /// Hay tres caminos que pueden crearla y antes uno de ellos —el fichero de
    /// ejemplo— la dejaba abierta: los permisos acababan dependiendo de quién
    /// llegara primero.
    #[test]
    #[cfg(unix)]
    fn la_carpeta_de_configuracion_nace_cerrada() {
        use std::os::unix::fs::PermissionsExt;

        let d = std::env::temp_dir().join(format!("vsion-conf-{}/anidada", std::process::id()));
        let _ = std::fs::remove_dir_all(d.parent().unwrap());

        crear_carpeta_privada(&d).unwrap();

        let modo = std::fs::metadata(&d).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_dir_all(d.parent().unwrap());
        assert_eq!(modo, 0o700, "solo su dueño");
    }
}
