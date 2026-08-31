//! El shell de verdad.
//!
//! Un `bash` interactivo bajo un pseudoterminal. No es una emulación ni una
//! herramienta «ejecutar comando»: es un TTY completo, así que `vim`, `htop`,
//! `ssh`, `git rebase -i` y cualquier programa que pida una contraseña
//! funcionan exactamente igual que en tu terminal.
//!
//! Esa es la diferencia estructural con los agentes actuales: su herramienta
//! Bash no tiene TTY, y por eso todo lo interactivo se les cuelga.
//!
//! ## Por qué un escáner propio y no `vt100::Callbacks`
//!
//! `vt100` sabe entregar las OSC desconocidas por callback, y es tentador.
//! Pero el callback te dice *qué* marcador llegó, no *dónde* estaba dentro del
//! buffer. Y eso importa: si en un mismo `read()` viene
//! `«hola\n» + fin-de-comando`, hay que meter «hola» en el registro **antes**
//! de cerrar el evento. Con el callback los marcadores llegan agrupados
//! después de procesar el trozo entero y el orden se pierde — se cuela la
//! salida de un comando en el siguiente.
//!
//! El escáner de [`crate::marker`] parte el flujo en el punto exacto, así que
//! el interleaving es correcto por construcción.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use color_eyre::eyre::{Result, eyre};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

use crate::marker::{Marker, Piece, Scanner};

/// Puente de errores.
///
/// `portable-pty` devuelve `anyhow::Error`, que no implementa
/// `std::error::Error`, así que `?` no lo convierte solo a `eyre`. Un
/// `map_err` con el mensaje resuelve el choque sin arrastrar `anyhow` como
/// dependencia directa solo para esto.
trait PtyErr<T> {
    fn pty(self, what: &str) -> Result<T>;
}

impl<T> PtyErr<T> for std::result::Result<T, anyhow::Error> {
    fn pty(self, what: &str) -> Result<T> {
        self.map_err(|e| eyre!("{what}: {e}"))
    }
}

/// Lo que el hilo lector le cuenta al resto de la aplicación, **en orden**.
#[derive(Debug)]
pub enum PtyMsg {
    /// Bytes de salida ya limpios de instrumentación.
    Output(Vec<u8>),
    /// Un marcador del shell.
    Mark(Marker),
    /// El shell terminó.
    Closed,
}

/// Filas de scrollback del emulador.
///
/// Cada celda de vt100 ocupa del orden de decenas de bytes; con 200 columnas,
/// 5000 filas rondan los 20-30 MB. En una máquina de 3,3 GB es lo que se puede
/// permitir sin que la TUI compita con el resto por memoria.
const SCROLLBACK: usize = 5_000;

/// Tamaño mínimo del pseudoterminal.
///
/// No es una preferencia estética: un emulador de una fila entra en caminos
/// degenerados y `vt100` llega a caer con un `unwrap` sobre `None`. Y llegar
/// ahí es fácil — un terminal sin tamaño conocido (`stty size` devuelve `0 0`
/// bajo `script`, en CI o sin terminal de control) da exactamente eso.
const MIN_ROWS: u16 = 4;
const MIN_COLS: u16 = 20;

/// Tamaño convencional cuando el terminal no sabe decir el suyo.
pub const FALLBACK_ROWS: u16 = 24;
pub const FALLBACK_COLS: u16 = 80;

/// Ajusta un tamaño para que el emulador nunca reciba algo degenerado.
///
/// Se aplica en el único sitio por el que pasan todos los tamaños, para que
/// ningún camino de la aplicación pueda colarle un cero.
pub fn sane_size(rows: u16, cols: u16) -> (u16, u16) {
    let rows = if rows == 0 {
        FALLBACK_ROWS
    } else {
        rows.max(MIN_ROWS)
    };
    let cols = if cols == 0 {
        FALLBACK_COLS
    } else {
        cols.max(MIN_COLS)
    };
    (rows, cols)
}

pub struct Shell {
    /// No hace falta compartirlo: solo se usa desde el bucle principal, para
    /// redimensionar. Envolverlo en `Arc` sugería que viajaba entre hilos y no
    /// viaja — y además `Arc` de algo que no es `Sync` no comparte nada.
    master: Box<dyn MasterPty + Send>,
    writer: Mutex<Box<dyn Write + Send>>,
    pub parser: Arc<RwLock<vt100::Parser>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Lo pone el hilo lector cuando hay algo nuevo que pintar. La UI lo
    /// consulta a su ritmo en vez de redibujar por cada `read()`: con
    /// `cat fichero_grande` eso serían miles de repintados por segundo.
    dirty: Arc<AtomicBool>,
    rows: u16,
    cols: u16,
    /// Fichero de arranque propio; se borra al cerrar.
    rc: std::path::PathBuf,
}

impl Shell {
    /// Arranca el shell con la instrumentación cargada.
    ///
    /// El fichero de init se escribe en disco al vuelo: hace `source` del
    /// `.bashrc` del usuario **primero** (para no perder sus alias, su prompt
    /// ni su PATH) y encima aplica la instrumentación.
    pub fn spawn(
        rows: u16,
        cols: u16,
        tx: tokio::sync::mpsc::UnboundedSender<PtyMsg>,
    ) -> Result<Self> {
        let (rows, cols) = sane_size(rows, cols);
        let rc = write_init_file()?;

        let sys = NativePtySystem::default();
        let pair = sys
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .pty("no pude abrir el pseudoterminal")?;

        let mut cmd = CommandBuilder::new(buscar_bash()?);
        cmd.arg("--rcfile");
        // Frontera de salida: `rc` es una ruta nativa —en Windows,
        // `C:\Users\...`— y quien la va a abrir es bash.
        cmd.arg(crate::rutas::a_posix(&rc.display().to_string()).as_ref());
        cmd.arg("-i");
        cmd.cwd(std::env::current_dir()?);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // Marca para que el usuario (y sus scripts) sepan dónde están.
        cmd.env("VSIONCODE", "1");

        let child = pair
            .slave
            .spawn_command(cmd)
            .pty("no pude arrancar el shell")?;
        // Sin esto el lector nunca ve EOF cuando el shell muere: el descriptor
        // del esclavo sigue abierto en nuestro proceso.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().pty("sin lector")?;
        let writer = pair.master.take_writer().pty("sin escritor")?;
        let master = pair.master;

        let parser = Arc::new(RwLock::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
        let dirty = Arc::new(AtomicBool::new(true));

        // Hilo lector. Bloqueante a propósito: `read()` sobre el PTY es la
        // única espera realmente eficiente, y así el planificador del sistema
        // hace el trabajo en vez de un bucle de sondeo.
        {
            let parser = parser.clone();
            let dirty = dirty.clone();
            std::thread::spawn(move || {
                let mut sc = Scanner::new();
                let mut buf = [0u8; 16 * 1024];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };

                    // Se reenvía pieza a pieza, respetando el orden original:
                    // los bytes que iban antes de un marcador salen antes que
                    // él. Es lo que hace que la salida de cada comando acabe
                    // en su propio evento del registro.
                    for pieza in sc.feed(&buf[..n]) {
                        let msg = match pieza {
                            Piece::Bytes(b) => {
                                // El candado se coge y se suelta aquí; jamás
                                // se mantiene mientras la interfaz dibuja.
                                if let Ok(mut p) = parser.write() {
                                    p.process(&b);
                                }
                                PtyMsg::Output(b)
                            }
                            Piece::Mark(m) => PtyMsg::Mark(m),
                        };
                        if tx.send(msg).is_err() {
                            return;
                        }
                    }
                    dirty.store(true, Ordering::Relaxed);
                }
                let _ = tx.send(PtyMsg::Closed);
            });
        }

        Ok(Self {
            master,
            writer: Mutex::new(writer),
            parser,
            child,
            dirty,
            rows,
            cols,
            rc,
        })
    }

    /// ¿Hay algo nuevo que pintar? Consultarlo limpia la marca.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// ¿Hay un programa a pantalla completa (vim, htop, less)?
    ///
    /// Mientras lo haya, la captura para el agente se pausa: lo que emite un
    /// programa así son miles de repintados por segundo, basura pura para un
    /// modelo y una vía rápida a llenar la RAM.
    pub fn alternate_screen(&self) -> bool {
        self.parser
            .read()
            .map(|p| p.screen().alternate_screen())
            .unwrap_or(false)
    }

    /// Lo que se ve en la pantalla ahora mismo, en texto plano.
    ///
    /// Es lo que hay que mandarle al agente cuando le pides que mire la
    /// pantalla. Y hace falta un camino aparte porque el registro de comandos
    /// se pausa mientras hay un programa a pantalla completa: `htop` y
    /// compañía emiten repintados a cientos por minuto y llenarían el
    /// registro de basura. Pero un diálogo de instalación esperando una
    /// respuesta también es pantalla completa, y ahí sí quieres que lo lea.
    pub fn screen_text(&self) -> String {
        let Ok(p) = self.parser.read() else {
            return String::new();
        };
        let filas: Vec<String> = p
            .screen()
            .rows(0, p.screen().size().1)
            .map(|r| r.trim_end().to_string())
            .collect();
        // Los programas a pantalla completa dejan medio lienzo en blanco; las
        // líneas vacías del final no le dicen nada a nadie.
        let fin = filas
            .iter()
            .rposition(|l| !l.is_empty())
            .map_or(0, |i| i + 1);
        filas[..fin].join("\n")
    }

    pub fn write_bytes(&self, data: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().map_err(|_| eyre!("writer envenenado"))?;
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    /// Escribe texto en la línea del shell **sin ejecutarlo**.
    ///
    /// Es el puente por el que una sugerencia del agente aterriza en tu
    /// prompt. Nunca añade `\r`: pulsar Enter sigue siendo decisión tuya, y
    /// esa separación es toda la política de seguridad de vsioncode.
    pub fn type_text(&self, text: &str) -> Result<()> {
        let one_line = text.replace('\n', " ").trim().to_string();
        self.write_bytes(one_line.as_bytes())
    }

    /// Redimensiona.
    ///
    /// Hay que hacer **las dos cosas**: el `ioctl` del PTY (que provoca el
    /// `SIGWINCH` sin el cual vim se sigue pintando con el tamaño viejo) y el
    /// tamaño del emulador. El ejemplo oficial de tui-term solo hace la
    /// segunda y por eso se ve mal.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        let (rows, cols) = sane_size(rows, cols);
        if rows == self.rows && cols == self.cols {
            return Ok(());
        }
        self.rows = rows;
        self.cols = cols;
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .pty("no pude redimensionar")?;
        if let Ok(mut p) = self.parser.write() {
            p.screen_mut().set_size(rows, cols);
        }
        self.mark_dirty();
        Ok(())
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

/// Al soltar el `Shell`, el shell se muere con él.
///
/// Sin esto, un pánico o una salida por un camino inesperado dejan un `bash`
/// vivo con el pseudoterminal abierto, invisible y para siempre. Se acumulan
/// en silencio: cada arranque fallido deja uno más, y en una máquina con poca
/// memoria eso acaba notándose sin que nadie sepa por qué.
impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.rc);
    }
}

/// Traduce una tecla de crossterm a los bytes que espera un TTY.
///
/// Los detalles que parecen menores y no lo son:
/// - Enter manda **CR** (`\r`), no LF. La disciplina de línea del TTY hace la
///   conversión; mandar LF rompe readline.
/// - Backspace manda **DEL** (0x7f), no BS (0x08).
/// - Las flechas cambian de codificación según el modo cursor de aplicación:
///   `vim` y `less` esperan SS3 (`ESC O A`) y no CSI (`ESC [ A`).
pub fn encode_key(k: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    use KeyCode::*;
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);

    let mut v = match k.code {
        Char(c) if ctrl => {
            // ^A..^Z y compañía: los 5 bits bajos del carácter en mayúscula.
            let b = (c.to_ascii_uppercase() as u8) & 0x1f;
            vec![b]
        }
        Char(c) => c.to_string().into_bytes(),
        Enter => vec![b'\r'],
        Backspace => vec![0x7f],
        Tab => vec![b'\t'],
        BackTab => b"\x1b[Z".to_vec(),
        Esc => vec![0x1b],
        Up => ss3_or_csi(app_cursor, b'A'),
        Down => ss3_or_csi(app_cursor, b'B'),
        Right => ss3_or_csi(app_cursor, b'C'),
        Left => ss3_or_csi(app_cursor, b'D'),
        Home => b"\x1b[H".to_vec(),
        End => b"\x1b[F".to_vec(),
        PageUp => b"\x1b[5~".to_vec(),
        PageDown => b"\x1b[6~".to_vec(),
        Delete => b"\x1b[3~".to_vec(),
        Insert => b"\x1b[2~".to_vec(),
        F(n @ 1..=4) => vec![0x1b, b'O', b'P' + (n - 1)],
        F(n @ 5..=12) => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            format!("\x1b[{}~", CODES[(n - 5) as usize]).into_bytes()
        }
        _ => return None,
    };

    // Alt se codifica como un ESC delante.
    if alt {
        v.insert(0, 0x1b);
    }
    Some(v)
}

fn ss3_or_csi(app_cursor: bool, letter: u8) -> Vec<u8> {
    if app_cursor {
        vec![0x1b, b'O', letter]
    } else {
        vec![0x1b, b'[', letter]
    }
}

/// Codifica un pegado respetando el modo *bracketed paste*.
///
/// Sin esto, pegar un bloque de texto en `vim` en modo inserción provoca
/// autoindentado en cascada y deja el fichero destrozado.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(text.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        text.as_bytes().to_vec()
    }
}

/// Borra los ficheros de arranque de instancias que ya no existen.
///
/// Al cerrar bien, cada shell borra el suyo. Pero si el proceso muere de golpe
/// —un `kill -9`, un cuelgue, un apagón— el fichero se queda ahí para siempre,
/// y se van acumulando sin que nadie los mire.
///
/// Se comprueba si el proceso dueño sigue vivo antes de borrar nada: con
/// varias instancias abiertas, borrar a ciegas dejaría a otra sin su fichero.
fn limpiar_huerfanos(dir: &Path) {
    let Ok(entradas) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entradas.flatten() {
        let nombre = e.file_name();
        let Some(nombre) = nombre.to_str() else {
            continue;
        };
        if !nombre.starts_with("init.") || !nombre.ends_with(".bash") {
            continue;
        }

        // `init.<pid>.<n>.bash`. Lo que no encaje con eso es del diseño
        // anterior, que compartía un único fichero: se borra sin más.
        match nombre.split('.').nth(1).and_then(|p| p.parse::<u32>().ok()) {
            Some(pid) if crate::sistema::vivo(pid) => {}
            _ => {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Qué `bash` se arranca dentro del panel.
///
/// No es el shell del usuario: es **bash**, siempre, porque la captura entera
/// —lo único que hace que vsioncode sea vsioncode— es un fichero de arranque de
/// bash. Respetar `$SHELL` cuando es `zsh` o `fish` daría un panel bonito que
/// no registra nada.
#[cfg(unix)]
fn buscar_bash() -> Result<std::path::PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    Ok(std::path::PathBuf::from(if shell.ends_with("bash") {
        shell
    } else {
        "/bin/bash".into()
    }))
}

/// En Windows el bash es el de **Git for Windows**, y hay que ir a buscarlo.
///
/// No hay `$SHELL` ni `/bin/bash`. Se prueban cuatro sitios en este orden y se
/// coge el primero que exista:
///
/// 1. `%CLAUDE_CODE_GIT_BASH_PATH%`, el mismo ajuste que ya usa Claude Code.
///    Quien lo tenga puesto no tiene que aprenderse otra variable nuestra.
/// 2. El registro, que es donde el instalador de Git deja la verdad aunque
///    alguien lo haya puesto en un disco raro.
/// 3. Los dos sitios de siempre, por si el registro no está.
/// 4. El `PATH`, el último porque es el que más miente (ver abajo).
///
/// Siempre `bin\bash.exe`, nunca `usr\bin\bash.exe` ni `git-bash.exe`: el
/// primero es el lanzador que espera un terminal por delante —que somos
/// nosotros—, y `git-bash.exe` abre su propia ventana de mintty.
///
/// Que no aparezca es fatal y se dice entero. Es el único componente
/// «opcional» que en realidad es obligatorio: sin él no hay shell que arrancar,
/// y callarlo dejaría el mismo agujero negro que arrancar sin cerebro.
#[cfg(windows)]
fn buscar_bash() -> Result<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut candidatos: Vec<PathBuf> = Vec::new();

    if let Ok(p) = std::env::var("CLAUDE_CODE_GIT_BASH_PATH") {
        candidatos.push(PathBuf::from(p));
    }
    if let Some(raiz) = git_del_registro() {
        candidatos.push(PathBuf::from(raiz).join("bin").join("bash.exe"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Ok(dir) = std::env::var(var) {
            candidatos.push(PathBuf::from(dir).join("Git").join("bin").join("bash.exe"));
        }
    }
    candidatos.extend(bash_del_path());

    for c in &candidatos {
        if c.is_file() {
            return Ok(c.clone());
        }
    }

    // Literales concatenados y no una cadena con `\` al final de cada línea:
    // la continuación se come los espacios del principio, y con ella la
    // sangría de la URL y del comando, que es justo lo que hay que copiar.
    Err(eyre!(concat!(
        "no encuentro el bash de Git for Windows, y sin él no hay nada que arrancar.\n",
        "\n",
        "La captura de vsioncode —comando, salida, código de salida— es un fichero de\n",
        "arranque de bash, así que el shell del panel tiene que ser bash. En Windows\n",
        "el que sirve viene con Git for Windows:\n",
        "\n",
        "    https://git-scm.com/downloads/win\n",
        "\n",
        "Si ya lo tienes instalado en otro sitio, dilo con:\n",
        "\n",
        "    setx CLAUDE_CODE_GIT_BASH_PATH \"C:\\ruta\\a\\Git\\bin\\bash.exe\"\n",
        "\n",
        "Y si lo que quieres es WSL2, ahí vsioncode se instala dentro de la\n",
        "distribución y no desde Windows.",
    )))
}

/// `HKLM\SOFTWARE\GitForWindows\InstallPath`, que es donde lo deja el
/// instalador.
#[cfg(windows)]
fn git_del_registro() -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};

    fn ancho(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let clave = ancho("SOFTWARE\\GitForWindows");
    let valor = ancho("InstallPath");
    // Dos llamadas: la primera pregunta el tamaño, la segunda lee. Reservar un
    // buffer «grande» y confiar es como se escriben los desbordamientos.
    let mut bytes: u32 = 0;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            clave.as_ptr(),
            valor.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if r != 0 || bytes == 0 {
        return None;
    }

    let mut buf = vec![0u16; bytes as usize / 2 + 1];
    let mut bytes2 = bytes;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            clave.as_ptr(),
            valor.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes2,
        )
    };
    if r != 0 {
        return None;
    }

    let fin = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..fin]))
}

/// Los `bash.exe` que hay en el `PATH`, saltándose el de `System32`.
///
/// Ese no es un shell: es el lanzador de WSL. Arrancarlo abriría una
/// distribución entera, con otro sistema de ficheros y otras rutas, y lo que se
/// vería es un vsioncode que edita ficheros que no son los que enseña.
#[cfg(windows)]
fn bash_del_path() -> Vec<std::path::PathBuf> {
    let Ok(path) = std::env::var("PATH") else {
        return Vec::new();
    };
    let system32 = std::env::var("SystemRoot")
        .map(|r| format!("{r}\\System32").to_ascii_lowercase())
        .unwrap_or_else(|_| "c:\\windows\\system32".into());

    std::env::split_paths(&path)
        .filter(|d| {
            !d.display()
                .to_string()
                .to_ascii_lowercase()
                .starts_with(&system32)
        })
        .map(|d| d.join("bash.exe"))
        .collect()
}

/// Escribe el fichero de arranque de **este** shell y devuelve su ruta.
///
/// Cada shell tiene el suyo, con un nombre único. La alternativa obvia —un
/// único `init.bash` compartido— parece más limpia y es una fuente constante
/// de carreras: mientras un `bash` está arrancando y leyendo el fichero, otra
/// instancia lo reescribe debajo. No falla de forma ruidosa; falla arrancando
/// un shell sin instrumentación, que simplemente no captura nada y nadie
/// entiende por qué.
///
/// Ocurre en cuanto abres dos vsioncode a la vez, así que no es un problema
/// solo de la batería de pruebas.
fn write_init_file() -> Result<std::path::PathBuf> {
    let dir = dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("vsioncode");
    std::fs::create_dir_all(&dir)?;

    let body = concat!(
        "# generado por vsioncode - no editar, se reescribe en cada arranque\n",
        "[ -r /etc/bash.bashrc ] && source /etc/bash.bashrc\n",
        "[ -r \"$HOME/.bashrc\" ] && source \"$HOME/.bashrc\"\n",
        "\n",
        include_str!("../shell/vsion-init.bash"),
    );

    limpiar_huerfanos(&dir);

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!("init.{}.{n}.bash", std::process::id()));

    std::fs::write(&path, body).map_err(|e| eyre!("no pude escribir {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: m,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn un_terminal_sin_tamano_no_hace_caer_el_emulador() {
        // `stty size` devuelve «0 0» sin terminal de control. Con eso, vt100
        // llega a caer con un unwrap: hay que sustituirlo por algo sensato.
        assert_eq!(sane_size(0, 0), (FALLBACK_ROWS, FALLBACK_COLS));
        assert_eq!(sane_size(0, 100), (FALLBACK_ROWS, 100));
        assert_eq!(sane_size(50, 0), (50, FALLBACK_COLS));
    }

    #[test]
    fn un_terminal_diminuto_se_eleva_al_minimo() {
        let (r, c) = sane_size(1, 1);
        assert!(r >= MIN_ROWS && c >= MIN_COLS, "quedó en {r}x{c}");
    }

    #[test]
    fn un_tamano_normal_se_respeta() {
        assert_eq!(sane_size(40, 160), (40, 160));
    }

    #[test]
    fn enter_manda_cr_no_lf() {
        assert_eq!(
            encode_key(key(KeyCode::Enter, KeyModifiers::NONE), false),
            Some(vec![b'\r'])
        );
    }

    #[test]
    fn backspace_manda_del() {
        assert_eq!(
            encode_key(key(KeyCode::Backspace, KeyModifiers::NONE), false),
            Some(vec![0x7f])
        );
    }

    #[test]
    fn control_c_es_etx() {
        assert_eq!(
            encode_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            Some(vec![3])
        );
        assert_eq!(
            encode_key(key(KeyCode::Char('d'), KeyModifiers::CONTROL), false),
            Some(vec![4])
        );
    }

    #[test]
    fn las_flechas_cambian_con_el_modo_aplicacion() {
        let up = key(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(encode_key(up, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_key(up, true), Some(b"\x1bOA".to_vec()));
    }

    #[test]
    fn alt_antepone_esc() {
        assert_eq!(
            encode_key(key(KeyCode::Char('b'), KeyModifiers::ALT), false),
            Some(vec![0x1b, b'b'])
        );
    }

    #[test]
    fn el_pegado_va_entre_corchetes() {
        assert_eq!(
            encode_paste("hola", true),
            b"\x1b[200~hola\x1b[201~".to_vec()
        );
        assert_eq!(encode_paste("hola", false), b"hola".to_vec());
    }
}
