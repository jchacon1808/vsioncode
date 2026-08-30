//! Escáner de marcadores OSC.
//!
//! Es la primera cosa que tocan los bytes que salen del PTY. Su trabajo:
//! sacar del flujo las secuencias de instrumentación que emite
//! `shell/vsion-init.bash`, y dejar pasar intacto absolutamente todo lo demás
//! para que el emulador vt100 lo pinte.
//!
//! Tres exigencias que condicionan el diseño:
//!
//! 1. **Streaming de verdad.** Un `read()` del PTY puede cortar una secuencia
//!    OSC por la mitad. El escáner guarda su estado entre llamadas, así que
//!    `ESC ] 7331 ; cmd ;` en un chunk y el base64 en el siguiente funciona
//!    igual que si hubieran llegado juntos.
//! 2. **No romper lo ajeno.** OSC 0 (título), OSC 8 (hyperlinks), OSC 52
//!    (portapapeles) y cualquier otro deben salir por el otro lado byte a
//!    byte. Solo interceptamos 133 y 7331.
//! 3. **Coste ~cero.** Es la ruta caliente: por aquí pasa cada byte de
//!    `cat archivo_gigante`. Sin asignaciones en el camino normal.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

/// Tope del buffer OSC. Una secuencia legítima no pasa de unos pocos KB
/// (OSC 52 con un portapapeles grande es el peor caso realista). Si algo
/// supera esto es basura binaria o un flujo corrupto: soltamos lo acumulado
/// tal cual y volvemos a modo normal en vez de crecer sin límite.
const MAX_OSC: usize = 64 * 1024;

/// Un trozo del flujo, **en su sitio**.
///
/// Devolver los bytes por un lado y los marcadores por otro parece
/// equivalente y no lo es: si en una misma lectura llega
/// `«hola\n» + fin-de-comando`, hay que meter «hola» en el registro *antes*
/// de cerrar el evento. Perder ese orden hace que la salida de un comando
/// aparezca en el siguiente, o que se pierda del todo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Bytes para el emulador y para el registro.
    Bytes(Vec<u8>),
    /// Un marcador del shell, exactamente donde estaba.
    Mark(Marker),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    /// OSC 133;A — el shell empieza a dibujar el prompt.
    PromptStart,
    /// OSC 133;B — fin del prompt, a partir de aquí escribe el usuario.
    PromptEnd,
    /// OSC 7331;cmd — arranca un comando; trae la línea tal cual se ejecutó.
    CmdStart(String),
    /// OSC 7331;end — terminó: código de salida y duración.
    CmdEnd { exit: i32, micros: u64 },
    /// OSC 7331;cwd — directorio de trabajo tras el comando.
    Cwd(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Copiando bytes tal cual.
    Normal,
    /// Vimos ESC; falta saber si viene un `]`.
    Esc,
    /// Dentro de una OSC, acumulando hasta BEL o ST.
    Osc,
    /// Dentro de una OSC y acabamos de ver ESC: si sigue `\` es el fin (ST).
    OscEsc,
}

pub struct Scanner {
    state: State,
    /// Payload de la OSC en curso (sin el `ESC ]` inicial ni el terminador).
    osc: Vec<u8>,
    /// Ya hemos visto un prompt. Antes del primero, los marcadores `cmd` son
    /// ruido del propio arranque del shell y se descartan.
    seen_prompt: bool,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            state: State::Normal,
            osc: Vec::with_capacity(256),
            seen_prompt: false,
        }
    }

    /// Procesa un trozo del flujo del PTY.
    ///
    /// Devuelve los bytes y los marcadores **intercalados en el orden real**
    /// en que aparecían. Esa fidelidad es lo que hace que la salida de cada
    /// comando acabe en su propio evento y no en el de al lado.
    pub fn feed(&mut self, input: &[u8]) -> Vec<Piece> {
        let mut pieces = Vec::new();
        let mut buf: Vec<u8> = Vec::new();

        for &b in input {
            match self.state {
                State::Normal => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    } else {
                        buf.push(b);
                    }
                }
                State::Esc => {
                    if b == b']' {
                        self.state = State::Osc;
                        self.osc.clear();
                    } else {
                        // Falsa alarma: no era una OSC. Devolvemos el ESC que
                        // habíamos retenido y seguimos.
                        buf.push(0x1b);
                        if b == 0x1b {
                            self.state = State::Esc; // ESC ESC: retenemos el segundo
                        } else {
                            buf.push(b);
                            self.state = State::Normal;
                        }
                    }
                }
                State::Osc => match b {
                    0x07 => self.finish_osc(&mut pieces, &mut buf, false), // BEL
                    0x1b => self.state = State::OscEsc,
                    _ => {
                        self.osc.push(b);
                        if self.osc.len() > MAX_OSC {
                            // Algo va mal: no era una OSC de verdad. Soltamos
                            // lo retenido sin interpretarlo, en vez de crecer
                            // sin límite con basura binaria.
                            self.flush_raw(&mut buf, None);
                        }
                    }
                },
                State::OscEsc => {
                    if b == b'\\' {
                        self.finish_osc(&mut pieces, &mut buf, true); // ST (ESC \)
                    } else {
                        // ESC suelto dentro de la OSC: no es un terminador.
                        self.osc.push(0x1b);
                        self.osc.push(b);
                        self.state = State::Osc;
                    }
                }
            }
        }

        if !buf.is_empty() {
            pieces.push(Piece::Bytes(buf));
        }
        pieces
    }

    /// Reemite la OSC acumulada sin interpretarla (no es nuestra, o es basura).
    fn flush_raw(&mut self, buf: &mut Vec<u8>, terminator: Option<&[u8]>) {
        buf.push(0x1b);
        buf.push(b']');
        buf.extend_from_slice(&self.osc);
        if let Some(t) = terminator {
            buf.extend_from_slice(t);
        }
        self.osc.clear();
        self.state = State::Normal;
    }

    fn finish_osc(&mut self, pieces: &mut Vec<Piece>, buf: &mut Vec<u8>, st: bool) {
        match self.parse_osc() {
            Some(m) => {
                // Es nuestra: no llega a la pantalla, y se coloca justo aquí,
                // detrás de los bytes que la precedían.
                if let Marker::PromptStart = m {
                    self.seen_prompt = true;
                }
                let keep = !matches!(m, Marker::CmdStart(_)) || self.seen_prompt;
                if keep {
                    if !buf.is_empty() {
                        pieces.push(Piece::Bytes(std::mem::take(buf)));
                    }
                    pieces.push(Piece::Mark(m));
                }
                self.osc.clear();
                self.state = State::Normal;
            }
            None => {
                // De otro (título, hyperlink, portapapeles): pasa entera, con
                // su terminador original.
                let t: &[u8] = if st { b"\x1b\\" } else { b"\x07" };
                self.flush_raw(buf, Some(t));
            }
        }
    }

    fn parse_osc(&self) -> Option<Marker> {
        let s = &self.osc[..];

        if let Some(rest) = s.strip_prefix(b"133;") {
            return match rest.first()? {
                b'A' => Some(Marker::PromptStart),
                b'B' => Some(Marker::PromptEnd),
                // 133;C y 133;D los emitimos por compatibilidad con otras
                // terminales, pero para nosotros mandan los 7331 (llevan
                // payload). Los tragamos igual para que no se pinten.
                b'C' | b'D' => Some(Marker::PromptEnd),
                _ => None,
            };
        }

        let rest = s.strip_prefix(b"7331;")?;
        let text = std::str::from_utf8(rest).ok()?;
        let (kind, payload) = text.split_once(';')?;

        match kind {
            "cmd" => {
                let raw = B64.decode(payload).ok()?;
                Some(Marker::CmdStart(String::from_utf8_lossy(&raw).into_owned()))
            }
            "cwd" => {
                let raw = B64.decode(payload).ok()?;
                Some(Marker::Cwd(String::from_utf8_lossy(&raw).into_owned()))
            }
            "end" => {
                let (ec, us) = payload.split_once(';')?;
                Some(Marker::CmdEnd {
                    exit: ec.parse().ok()?,
                    micros: us.parse().unwrap_or(0),
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aplana el resultado a (bytes, marcadores) para las comprobaciones
    /// sencillas. El orden se prueba aparte, con `piezas`.
    fn scan(chunks: &[&[u8]]) -> (Vec<u8>, Vec<Marker>) {
        let mut out = Vec::new();
        let mut ms = Vec::new();
        for p in piezas(chunks) {
            match p {
                Piece::Bytes(b) => out.extend(b),
                Piece::Mark(m) => ms.push(m),
            }
        }
        (out, ms)
    }

    fn piezas(chunks: &[&[u8]]) -> Vec<Piece> {
        let mut sc = Scanner::new();
        let mut v = Vec::new();
        for c in chunks {
            v.extend(sc.feed(c));
        }
        v
    }

    #[test]
    fn texto_normal_pasa_intacto() {
        let (out, ms) = scan(&[b"hola mundo\n"]);
        assert_eq!(out, b"hola mundo\n");
        assert!(ms.is_empty());
    }

    #[test]
    fn extrae_marcadores_y_no_los_pinta() {
        // Secuencia real: prompt, comando `echo hola`, salida, fin con exit 0.
        let cmd = B64.encode("echo hola");
        let input = format!(
            "\x1b]133;A\x07$ \x1b]133;B\x07\x1b]7331;cmd;{cmd}\x07hola\n\x1b]7331;end;0;8335\x07"
        );
        let (out, ms) = scan(&[input.as_bytes()]);
        assert_eq!(out, b"$ hola\n");
        assert_eq!(ms[0], Marker::PromptStart);
        assert_eq!(ms[1], Marker::PromptEnd);
        assert_eq!(ms[2], Marker::CmdStart("echo hola".into()));
        assert_eq!(
            ms[3],
            Marker::CmdEnd {
                exit: 0,
                micros: 8335
            }
        );
    }

    #[test]
    fn sobrevive_a_cortes_entre_lecturas() {
        // El mismo flujo, partido byte a byte: no puede cambiar el resultado.
        let cmd = B64.encode("ls -la");
        let input =
            format!("\x1b]133;A\x07>\x1b]133;B\x07\x1b]7331;cmd;{cmd}\x07x\x1b]7331;end;2;1\x07");
        let bytes = input.into_bytes();
        let chunks: Vec<&[u8]> = bytes.chunks(1).collect();
        let (out, ms) = scan(&chunks);
        assert_eq!(out, b">x");
        assert_eq!(ms[2], Marker::CmdStart("ls -la".into()));
        assert_eq!(ms[3], Marker::CmdEnd { exit: 2, micros: 1 });
    }

    #[test]
    fn respeta_osc_de_terceros() {
        // OSC 0 (título) y OSC 8 (hyperlink) tienen que llegar al emulador.
        let (out, ms) = scan(&[b"\x1b]0;titulo\x07texto\x1b]8;;http://x\x1b\\link"]);
        assert_eq!(out, b"\x1b]0;titulo\x07texto\x1b]8;;http://x\x1b\\link");
        assert!(ms.is_empty());
    }

    #[test]
    fn acepta_terminador_st() {
        let (out, ms) = scan(&[b"\x1b]133;A\x1b\\ok"]);
        assert_eq!(out, b"ok");
        assert_eq!(ms, vec![Marker::PromptStart]);
    }

    #[test]
    fn esc_suelto_no_se_traga_bytes() {
        let (out, ms) = scan(&[b"\x1b[31mrojo\x1b[0m"]);
        assert_eq!(out, b"\x1b[31mrojo\x1b[0m");
        assert!(ms.is_empty());
    }

    #[test]
    fn ignora_comandos_previos_al_primer_prompt() {
        // Ruido de arranque del propio vsion-init.bash: no debe entrar al registro.
        let cmd = B64.encode("[[ arranque ]]");
        let input = format!("\x1b]7331;cmd;{cmd}\x07");
        let (_, ms) = scan(&[input.as_bytes()]);
        assert!(ms.is_empty());
    }

    /// La prueba que justifica que exista `Piece`.
    ///
    /// Si los bytes salieran todos juntos y los marcadores después, «hola»
    /// llegaría al registro antes de que el evento estuviera abierto y se
    /// perdería; o peor, se le achacaría al comando siguiente.
    #[test]
    fn los_bytes_salen_antes_del_marcador_que_les_seguia() {
        let cmd = B64.encode("echo hola");
        let input = format!(
            "\x1b]133;A\x07$ \x1b]133;B\x07\x1b]7331;cmd;{cmd}\x07hola\n\x1b]7331;end;0;5\x07"
        );
        let ps = piezas(&[input.as_bytes()]);

        let pos_inicio = ps
            .iter()
            .position(|p| matches!(p, Piece::Mark(Marker::CmdStart(_))))
            .unwrap();
        let pos_fin = ps
            .iter()
            .position(|p| matches!(p, Piece::Mark(Marker::CmdEnd { .. })))
            .unwrap();
        let pos_hola = ps
            .iter()
            .position(|p| matches!(p, Piece::Bytes(b) if b.windows(4).any(|w| w == b"hola")))
            .expect("«hola» no salió por ningún lado");

        assert!(
            pos_inicio < pos_hola,
            "«hola» salió antes de abrirse el comando"
        );
        assert!(
            pos_hola < pos_fin,
            "«hola» salió después de cerrarse el comando"
        );
    }

    #[test]
    fn el_orden_aguanta_aunque_se_parta_la_lectura() {
        let cmd = B64.encode("echo x");
        let input = format!("\x1b]133;A\x07\x1b]7331;cmd;{cmd}\x07salida\x1b]7331;end;0;1\x07");
        let bytes = input.into_bytes();
        let trozos: Vec<&[u8]> = bytes.chunks(3).collect();
        let ps = piezas(&trozos);

        let pos_inicio = ps
            .iter()
            .position(|p| matches!(p, Piece::Mark(Marker::CmdStart(_))))
            .unwrap();
        let pos_fin = ps
            .iter()
            .position(|p| matches!(p, Piece::Mark(Marker::CmdEnd { .. })))
            .unwrap();
        let recogido: Vec<u8> = ps[pos_inicio..pos_fin]
            .iter()
            .filter_map(|p| match p {
                Piece::Bytes(b) => Some(b.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(recogido, b"salida");
    }

    #[test]
    fn payload_con_saltos_y_puntoycoma() {
        // El base64 existe precisamente para esto.
        let raw = "for i in 1 2; do\n  echo \"$i;\"\ndone";
        let cmd = B64.encode(raw);
        let input = format!("\x1b]133;A\x07\x1b]7331;cmd;{cmd}\x07");
        let (out, ms) = scan(&[input.as_bytes()]);
        assert!(out.is_empty());
        assert_eq!(ms[1], Marker::CmdStart(raw.into()));
    }
}
