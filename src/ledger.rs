//! El registro compartido.
//!
//! Aquí vive la idea central de vsioncode: humano y agente leen **lo mismo**.
//! Cada comando que ejecutas produce un [`CommandEvent`] y el agente lo recibe
//! sin que tengas que copiar nada.
//!
//! Tres problemas reales que resuelve este módulo, y que son la diferencia
//! entre una demo y algo que aguanta un `cargo build` de verdad:
//!
//! - **La salida puede ser enorme.** `make` suelta megas. Guardarlo todo se
//!   come la RAM (la máquina objetivo tiene 3,3 GB) y mandárselo al modelo
//!   cuesta dinero y segundos. Guardamos principio y final: el principio dice
//!   *qué* se hizo, el final dice *por qué falló*. Lo de en medio casi nunca
//!   importa.
//! - **La salida trae ANSI.** Colores y reposicionamientos de cursor son ruido
//!   puro para un LLM: gasta tokens y confunde. Se limpian al cerrar el evento.
//! - **La salida puede traer secretos.** Un `env` o un `docker login -p` mal
//!   dado no debe salir de esta máquina. Se redacta antes de que nadie lo vea.

use std::collections::VecDeque;
use std::time::Duration;

/// Cuántos eventos recordamos. Un anillo: entra el nuevo, cae el más viejo.
const MAX_EVENTS: usize = 300;

/// Bytes que conservamos del principio de la salida de un comando.
const HEAD_BYTES: usize = 24 * 1024;
/// Bytes que conservamos del final. Más grande que la cabeza a propósito: los
/// errores viven al final.
const TAIL_BYTES: usize = 40 * 1024;

/// Un comando ejecutado por el humano, tal y como lo ve el agente.
#[derive(Debug, Clone)]
pub struct CommandEvent {
    pub id: u64,
    pub cmd: String,
    pub cwd: String,
    pub exit: Option<i32>,
    pub duration: Duration,
    /// Salida ya limpia de ANSI, recortada y redactada.
    pub output: String,
    /// Si es `true`, se comió un trozo del medio.
    pub truncated: bool,
    pub omitted_bytes: usize,
}

impl CommandEvent {
    pub fn failed(&self) -> bool {
        matches!(self.exit, Some(c) if c != 0)
    }

    /// Cómo se le presenta este evento al modelo. Compacto y sin adornos:
    /// cada token de decoración es un token que no gastamos en la salida real.
    ///
    /// Sin tope: para quien ya tiene uno por fuera —[`Ledger::bloques`] lleva
    /// su presupuesto— o para lo que no va al modelo. Lo que se le manda a un
    /// modelo pasa por [`Self::para_el_modelo`].
    pub fn to_context(&self) -> String {
        self.componer(&self.output)
    }

    /// Lo mismo, pero sin pasar de `budget` bytes de salida.
    ///
    /// Existe porque el camino más caro del programa no tenía tope ninguno.
    /// El registro guarda hasta 64 KB por comando —`HEAD` más `TAIL`— y eso
    /// está bien para mirarlo en pantalla, pero al modelo se le mandaba tal
    /// cual: un `cargo build` entraba entero en la conversación, y como el
    /// daemon la guarda dentro del proceso, **se vuelve a pagar en cada turno
    /// posterior**. En una tanda larga del modo libre eso era la mayor parte
    /// de la factura.
    ///
    /// Se recorta por el medio y no por el final a propósito: de una salida
    /// larga lo que dice algo son las primeras líneas —qué se estaba
    /// haciendo— y las últimas —cómo acabó—. El centro de un `build` son mil
    /// líneas de `Compiling`.
    pub fn para_el_modelo(&self, budget: usize) -> String {
        self.componer(&recorta_por_el_medio(&self.output, budget))
    }

    fn componer(&self, salida: &str) -> String {
        let mut s = String::with_capacity(salida.len() + 128);
        s.push_str(&format!("$ {}\n", self.cmd));
        s.push_str(&format!("(cwd: {}", self.cwd));
        match self.exit {
            Some(c) => s.push_str(&format!(" · exit: {c}")),
            None => s.push_str(" · en curso"),
        }
        s.push_str(&format!(" · {:.2}s)\n", self.duration.as_secs_f32()));
        if salida.trim().is_empty() {
            s.push_str("[sin salida]\n");
        } else {
            s.push_str(salida);
            if !salida.ends_with('\n') {
                s.push('\n');
            }
        }
        s
    }
}

/// Deja `texto` en `budget` bytes quedándose con el principio y el final.
///
/// Corta en frontera de línea: partir una línea por la mitad deja al modelo
/// leyendo media ruta o medio número, que es peor que no dársela. Y en
/// frontera de carácter, que `budget` cuenta bytes y aquí hay UTF-8.
fn recorta_por_el_medio(texto: &str, budget: usize) -> String {
    if texto.len() <= budget {
        return texto.to_string();
    }
    // Dos tercios para la cabeza: el aviso de qué se intentaba pesa más que
    // el final cuando el final es un error que ya se ve en la última línea.
    let cabeza = budget * 2 / 3;
    let cola = budget.saturating_sub(cabeza);

    // La búsqueda va sobre bytes y no sobre `&str`: cortar la cadena por
    // `cabeza` para buscar dentro entra en pánico en cuanto ahí hay una tilde,
    // y una salida de terminal en español las tiene. Un `\n` es ASCII, así que
    // el índice de después de uno siempre es frontera de carácter.
    let b = texto.as_bytes();
    let corte_ini = b[..cabeza.min(b.len())]
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(0, |i| i + 1);
    let desde = b.len().saturating_sub(cola);
    let corte_fin = b[desde..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |i| desde + i + 1);

    if corte_fin <= corte_ini {
        // Una sola línea gigantesca: no hay frontera donde cortar, así que
        // se queda la cabeza y se dice que falta el resto.
        let fin = (0..=cabeza.min(texto.len()))
            .rev()
            .find(|i| texto.is_char_boundary(*i))
            .unwrap_or(0);
        debug_assert!(texto.is_char_boundary(fin));
        return format!(
            "{}\n[… vsioncode cortó aquí: faltan {} bytes del final …]\n",
            &texto[..fin],
            texto.len() - fin
        );
    }

    format!(
        "{}[… vsioncode omitió {} bytes del centro; arriba el principio, abajo el final …]\n{}",
        &texto[..corte_ini],
        corte_fin - corte_ini,
        &texto[corte_fin..]
    )
}

/// Buffer que conserva cabeza y cola descartando el centro.
///
/// Memoria acotada por construcción: nunca pasa de `HEAD + TAIL`, pase lo que
/// pase por él. Un `yes | head -c 1G` no lo mueve.
#[derive(Default)]
struct CappedBuffer {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: usize,
}

impl CappedBuffer {
    /// Traga un trozo de salida.
    ///
    /// Va por bloques y no byte a byte porque esta es la ruta caliente: por
    /// aquí pasa cada byte de un `cargo build`, y un `yes` suelta megas por
    /// segundo. Copiar tramos enteros deja el trabajo en `memcpy` en vez de en
    /// un salto condicional por byte.
    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();

        // Primero se llena la cabeza; lo que no quepa es cola.
        let hueco = (HEAD_BYTES - self.head.len()).min(bytes.len());
        let (a_cabeza, resto) = bytes.split_at(hueco);
        self.head.extend_from_slice(a_cabeza);
        if resto.is_empty() {
            return;
        }

        // Si este trozo ya llena la cola por sí solo, lo que hubiera antes
        // sobra entero: se habría descartado igual empujando de uno en uno.
        if resto.len() >= TAIL_BYTES {
            self.tail.clear();
            self.tail.extend(&resto[resto.len() - TAIL_BYTES..]);
            return;
        }

        self.tail.extend(resto);
        let sobran = self.tail.len().saturating_sub(TAIL_BYTES);
        self.tail.drain(..sobran);
    }

    fn omitted(&self) -> usize {
        self.total.saturating_sub(self.head.len() + self.tail.len())
    }

    fn finish(self) -> (String, bool, usize) {
        let omitted = self.omitted();
        let mut raw = self.head;
        if omitted > 0 {
            raw.extend_from_slice(
                format!(
                    "\n[… vsioncode omitió {omitted} bytes del centro; arriba el principio, \
                     abajo el final …]\n"
                )
                .as_bytes(),
            );
        }
        raw.extend(self.tail);
        let text = String::from_utf8_lossy(&raw);
        (redact(&strip_ansi(&text)), omitted > 0, omitted)
    }

    fn clear(&mut self) {
        self.head.clear();
        self.tail.clear();
        self.total = 0;
    }
}

/// Comando abierto: entre el marcador de inicio y el de fin.
struct InFlight {
    cmd: String,
    cwd: String,
    buf: CappedBuffer,
}

pub struct Ledger {
    events: VecDeque<CommandEvent>,
    in_flight: Option<InFlight>,
    next_id: u64,
    cwd: String,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

impl Ledger {
    pub fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(MAX_EVENTS),
            in_flight: None,
            next_id: 1,
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".into()),
        }
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn set_cwd(&mut self, cwd: String) {
        self.cwd = cwd;
    }

    pub fn busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Comando en ejecución ahora mismo, si lo hay.
    pub fn running_cmd(&self) -> Option<&str> {
        self.in_flight.as_ref().map(|f| f.cmd.as_str())
    }

    pub fn begin(&mut self, cmd: String) {
        // Si llega un inicio sin haberse cerrado el anterior (pasa si el shell
        // se reinicia o alguien manda una señal rara), cerramos el viejo sin
        // código de salida en vez de perderlo.
        if self.in_flight.is_some() {
            self.end(None, Duration::ZERO);
        }
        self.in_flight = Some(InFlight {
            cmd,
            cwd: self.cwd.clone(),
            buf: CappedBuffer::default(),
        });
    }

    /// Bytes de salida (ya sin marcadores) del comando en curso.
    ///
    /// Si no hay comando abierto se descartan: es la salida del propio prompt,
    /// no le interesa a nadie.
    pub fn feed_output(&mut self, bytes: &[u8]) {
        if let Some(f) = self.in_flight.as_mut() {
            f.buf.push(bytes);
        }
    }

    pub fn end(&mut self, exit: Option<i32>, duration: Duration) {
        let Some(f) = self.in_flight.take() else {
            return;
        };
        let (output, truncated, omitted_bytes) = f.buf.finish();
        let ev = CommandEvent {
            id: self.next_id,
            cmd: f.cmd,
            cwd: f.cwd,
            exit,
            duration,
            output,
            truncated,
            omitted_bytes,
        };
        self.next_id += 1;
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(ev);
    }

    /// Aborta el comando en curso tirando su salida (p.ej. tras `clear`).
    pub fn discard_in_flight_output(&mut self) {
        if let Some(f) = self.in_flight.as_mut() {
            f.buf.clear();
        }
    }

    pub fn last(&self) -> Option<&CommandEvent> {
        self.events.back()
    }

    pub fn last_failure(&self) -> Option<&CommandEvent> {
        self.events.iter().rev().find(|e| e.failed())
    }

    pub fn recent(&self, n: usize) -> impl Iterator<Item = &CommandEvent> {
        self.events.iter().rev().take(n).rev()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Contexto para el modelo: los últimos `n` eventos, con un presupuesto de
    /// bytes duro. Se llena desde el más reciente hacia atrás, porque lo de
    /// hace un minuto importa más que lo de hace media hora.
    pub fn context(&self, n: usize, budget: usize) -> String {
        self.bloques(self.events.iter().rev().take(n), budget)
    }

    /// Contexto de lo ocurrido **después** de `desde`, con el mismo criterio.
    ///
    /// El daemon guarda la conversación dentro del proceso, así que lo que ya
    /// se le mandó lo sigue teniendo delante. Reenviarle los seis últimos
    /// comandos en cada turno le paga cinco veces lo mismo, y en una sesión
    /// larga eso es la mayor parte del gasto evitable.
    ///
    /// El tope de `n` y el presupuesto siguen mandando: si mientras se pensaba
    /// una respuesta se ejecutaron treinta comandos, entran los más recientes
    /// que quepan, no todos.
    pub fn context_nuevos(&self, desde: u64, n: usize, budget: usize) -> String {
        self.bloques(
            self.events.iter().rev().filter(|e| e.id > desde).take(n),
            budget,
        )
    }

    /// Cuántos eventos hay posteriores a `desde`, hasta el tope de `n`.
    ///
    /// Para poder decir «le mandaría dos comandos» sin recontar el texto ya
    /// compuesto, que es la clase de cuenta que se desincroniza a la primera.
    pub fn cuantos_nuevos(&self, desde: u64, n: usize) -> usize {
        self.events
            .iter()
            .rev()
            .filter(|e| e.id > desde)
            .take(n)
            .count()
    }

    /// El id del evento más reciente, o 0 si no hay ninguno.
    ///
    /// Es la marca que guarda quien manda contexto para saber por dónde iba.
    /// Cero es «no he mandado nada», y como los ids empiezan por encima de
    /// cero, sirve igual para un registro vacío y para un cerebro recién
    /// arrancado que no ha visto nada.
    pub fn ultimo_id(&self) -> u64 {
        self.events.back().map_or(0, |e| e.id)
    }

    /// Lo común de las dos: llenar desde el más reciente con presupuesto duro.
    fn bloques<'a>(
        &self,
        eventos: impl Iterator<Item = &'a CommandEvent>,
        budget: usize,
    ) -> String {
        let mut blocks: Vec<String> = Vec::new();
        let mut used = 0;
        for ev in eventos {
            let b = ev.to_context();
            if used + b.len() > budget {
                // El más reciente entra siempre, pero **recortado**. Antes
                // entraba entero: el presupuesto no valía nada cuando el
                // último comando era justamente el que se había pasado, que
                // es el caso que había que cubrir.
                if blocks.is_empty() {
                    blocks.push(ev.para_el_modelo(budget));
                }
                break;
            }
            used += b.len();
            blocks.push(b);
        }
        blocks.reverse();
        blocks.join("\n")
    }
}

/// Quita secuencias ANSI dejando el texto.
///
/// No es un emulador: para el registro solo queremos las letras. Lo que ya
/// pintó vt100 es cosa aparte. Reconoce CSI, OSC y los escapes de dos bytes,
/// que es todo lo que suelta un programa de consola normal.
pub fn strip_ansi(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != 0x1b {
            // Reconstruimos desde bytes para no partir UTF-8 multibyte.
            let start = i;
            while i < b.len() && b[i] != 0x1b {
                i += 1;
            }
            out.push_str(&String::from_utf8_lossy(&b[start..i]));
            continue;
        }
        i += 1;
        if i >= b.len() {
            break;
        }
        match b[i] {
            b'[' => {
                // CSI: parámetros hasta un byte final en 0x40..=0x7e.
                i += 1;
                while i < b.len() && !(0x40..=0x7e).contains(&b[i]) {
                    i += 1;
                }
                i += 1;
            }
            b']' => {
                // OSC: hasta BEL o ST.
                i += 1;
                while i < b.len() {
                    if b[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if b[i] == 0x1b && i + 1 < b.len() && b[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Enmascara secretos evidentes antes de que el texto salga de la máquina.
///
/// Es una red de seguridad, no una garantía: cubre los formatos que de verdad
/// aparecen en una terminal (claves de API con prefijo reconocible, cabeceras
/// de autorización, asignaciones tipo `TOKEN=...`). Sin `regex`: recorrido
/// manual, cero dependencias y sin coste de compilación.
pub fn redact(s: &str) -> String {
    const PREFIXES: [&str; 8] = [
        "sk-",
        "sk_live_",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "AKIA",
        "AIza",
    ];
    const KEYWORDS: [&str; 7] = [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "APIKEY",
        "API_KEY",
        "ACCESS_KEY",
    ];

    let mut out = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        let upper = line.to_ascii_uppercase();

        // Cabecera de autorización: se va el valor entero.
        if let Some(p) = upper.find("AUTHORIZATION:") {
            out.push_str(&line[..p + "AUTHORIZATION:".len()]);
            out.push_str(" «redactado por vsioncode»");
            if line.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }

        let mut masked = String::with_capacity(line.len());
        for tok in split_keep(line) {
            masked.push_str(&mask_token(tok, &PREFIXES, &KEYWORDS));
        }
        out.push_str(&masked);
    }
    out
}

/// Parte en «palabras» conservando los separadores, para poder recomponer la
/// línea exactamente igual salvo los trozos enmascarados.
fn split_keep(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let bytes = line.as_bytes();
    let mut i = 0;
    let sep = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b',' | b'\'' | b'"');
    while i < bytes.len() {
        if sep(bytes[i]) {
            if i > start {
                parts.push(&line[start..i]);
            }
            parts.push(&line[i..i + 1]);
            start = i + 1;
        }
        i += 1;
    }
    if start < line.len() {
        parts.push(&line[start..]);
    }
    parts
}

fn mask_token(tok: &str, prefixes: &[&str], keywords: &[&str]) -> String {
    // `NOMBRE=valor` donde NOMBRE huele a credencial.
    if let Some((name, value)) = tok.split_once('=')
        && !value.is_empty()
    {
        let up = name.to_ascii_uppercase();
        if keywords.iter().any(|k| up.contains(k)) {
            return format!("{name}=«redactado»");
        }
    }
    // Claves con prefijo reconocible y longitud de clave de verdad.
    if tok.len() >= 16 && prefixes.iter().any(|p| tok.starts_with(p)) {
        let keep = tok.len().min(6);
        return format!("{}…«redactado»", &tok[..keep]);
    }
    tok.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mete un comando ya terminado, con su salida.
    fn ejecuta(l: &mut Ledger, cmd: &str, salida: &str) {
        l.begin(cmd.into());
        l.feed_output(salida.as_bytes());
        l.end(Some(0), Duration::from_millis(1));
    }

    /// Lo que ya se le mandó al modelo no se le vuelve a mandar.
    ///
    /// El daemon guarda la conversación dentro del proceso: repetirle los seis
    /// últimos comandos en cada turno le cobra cinco veces lo mismo. Este test
    /// es esa regla, y de paso fija que el corte es por id y no por posición
    /// —el anillo se mueve, los ids no—.
    #[test]
    fn solo_viaja_lo_que_el_modelo_no_ha_visto() {
        let mut l = Ledger::new();
        ejecuta(&mut l, "uno", "salida uno");
        ejecuta(&mut l, "dos", "salida dos");

        let visto = l.ultimo_id();
        let ctx = l.context_nuevos(0, 6, 12 * 1024);
        assert!(ctx.contains("uno") && ctx.contains("dos"), "{ctx}");

        // Nada nuevo desde entonces: no hay nada que mandar.
        assert_eq!(l.context_nuevos(visto, 6, 12 * 1024), "");

        ejecuta(&mut l, "tres", "salida tres");
        let ctx = l.context_nuevos(visto, 6, 12 * 1024);
        assert!(ctx.contains("tres"), "{ctx}");
        assert!(
            !ctx.contains("salida uno"),
            "lo ya dicho no se repite: {ctx}"
        );
    }

    /// Sin nada ejecutado, la marca es cero y no hay contexto que mandar.
    #[test]
    fn un_registro_vacio_no_tiene_ultimo_id() {
        let l = Ledger::new();
        assert_eq!(l.ultimo_id(), 0);
        assert_eq!(l.context_nuevos(0, 6, 12 * 1024), "");
    }

    /// El contexto completo sigue existiendo y sigue siendo el de siempre.
    ///
    /// Lo usa `/contexto` para enseñar la foto del registro, que es otra cosa
    /// distinta de lo que viaja en un turno.
    #[test]
    fn el_contexto_completo_no_cambia() {
        let mut l = Ledger::new();
        ejecuta(&mut l, "uno", "salida uno");
        ejecuta(&mut l, "dos", "salida dos");
        let todo = l.context(6, 12 * 1024);
        assert!(todo.contains("uno") && todo.contains("dos"));
        assert_eq!(todo, l.context_nuevos(0, 6, 12 * 1024));
    }

    #[test]
    fn limpia_ansi_sin_romper_utf8() {
        let s = "\x1b[31mError:\x1b[0m año \x1b]0;t\x07ñu";
        assert_eq!(strip_ansi(s), "Error: año ñu");
    }

    #[test]
    fn registra_un_comando_completo() {
        let mut l = Ledger::new();
        l.begin("ls /noexiste".into());
        l.feed_output(b"ls: no se puede acceder\n");
        l.end(Some(2), Duration::from_millis(15));
        let ev = l.last().unwrap();
        assert_eq!(ev.cmd, "ls /noexiste");
        assert_eq!(ev.exit, Some(2));
        assert!(ev.failed());
        assert!(ev.output.contains("no se puede acceder"));
    }

    #[test]
    fn la_salida_no_puede_crecer_sin_limite() {
        let mut l = Ledger::new();
        l.begin("yes".into());
        let chunk = vec![b'x'; 1024];
        for _ in 0..2000 {
            l.feed_output(&chunk); // 2 MB
        }
        l.end(Some(0), Duration::ZERO);
        let ev = l.last().unwrap();
        assert!(ev.truncated);
        assert!(ev.output.len() < HEAD_BYTES + TAIL_BYTES + 200);
    }

    #[test]
    fn conserva_principio_y_final() {
        let mut l = Ledger::new();
        l.begin("make".into());
        l.feed_output(b"PRIMERA LINEA\n");
        l.feed_output(&vec![b'.'; HEAD_BYTES + TAIL_BYTES + 5000]);
        l.feed_output(b"\nerror: fallo final\n");
        l.end(Some(1), Duration::ZERO);
        let ev = l.last().unwrap();
        assert!(ev.output.starts_with("PRIMERA LINEA"));
        assert!(ev.output.trim_end().ends_with("error: fallo final"));
        assert!(ev.output.contains("del centro"));
    }

    #[test]
    fn el_anillo_no_crece() {
        let mut l = Ledger::new();
        for i in 0..(MAX_EVENTS + 50) {
            l.begin(format!("cmd{i}"));
            l.end(Some(0), Duration::ZERO);
        }
        assert_eq!(l.len(), MAX_EVENTS);
        assert_eq!(l.last().unwrap().cmd, format!("cmd{}", MAX_EVENTS + 49));
    }

    #[test]
    fn redacta_secretos_evidentes() {
        let s = "export GITHUB_TOKEN=ghp_abcdefghijklmnop\nAuthorization: Bearer xyz.123\nnormal texto\n";
        let r = redact(s);
        assert!(!r.contains("ghp_abcdefghijklmnop"));
        assert!(!r.contains("xyz.123"));
        assert!(r.contains("normal texto"));
    }

    #[test]
    fn no_redacta_texto_inocente() {
        let s = "cargo build --release\nCompilando serde v1.0.229\n";
        assert_eq!(redact(s), s);
    }

    #[test]
    fn el_contexto_respeta_el_presupuesto() {
        let mut l = Ledger::new();
        for i in 0..20 {
            l.begin(format!("cmd{i}"));
            l.feed_output(&vec![b'z'; 2000]);
            l.end(Some(0), Duration::ZERO);
        }
        let ctx = l.context(10, 5000);
        assert!(ctx.len() <= 5000 + 2500);
        // Lo más reciente entra sí o sí.
        assert!(ctx.contains("cmd19"));
    }

    #[test]
    fn cierra_un_comando_huerfano_al_empezar_otro() {
        let mut l = Ledger::new();
        l.begin("uno".into());
        l.begin("dos".into());
        assert_eq!(l.len(), 1);
        assert_eq!(l.last().unwrap().cmd, "uno");
        assert_eq!(l.last().unwrap().exit, None);
    }

    /// La versión byte a byte, tal y como estuvo escrita. Es la definición de
    /// lo que el buffer tiene que hacer, y se queda aquí como referencia
    /// contra la que medir la versión por bloques.
    fn referencia(trozos: &[&[u8]]) -> (Vec<u8>, Vec<u8>, usize) {
        let (mut head, mut tail, mut total) = (Vec::new(), VecDeque::new(), 0usize);
        for t in trozos {
            for &b in t.iter() {
                total += 1;
                if head.len() < HEAD_BYTES {
                    head.push(b);
                } else {
                    tail.push_back(b);
                    if tail.len() > TAIL_BYTES {
                        tail.pop_front();
                    }
                }
            }
        }
        (head, tail.into_iter().collect(), total)
    }

    /// Que el buffer trague por bloques no puede cambiar lo que guarda.
    ///
    /// El troceado es lo que de verdad se prueba: el PTY corta donde le da la
    /// gana, y un trozo puede caer justo en el borde de la cabeza, llenar la
    /// cola de una vez, o quedarse corto. Los tamaños de abajo cubren los tres
    /// casos y los bordes exactos.
    #[test]
    fn tragar_por_bloques_guarda_lo_mismo_que_byte_a_byte() {
        let grande: Vec<u8> = (0..(HEAD_BYTES + TAIL_BYTES + 5000))
            .map(|i| (i % 251) as u8)
            .collect();

        for corte in [
            1,
            7,
            HEAD_BYTES - 1,
            HEAD_BYTES,
            HEAD_BYTES + 1,
            TAIL_BYTES,
            TAIL_BYTES + 1,
            grande.len(),
        ] {
            let trozos: Vec<&[u8]> = grande.chunks(corte).collect();
            let (head, tail, total) = referencia(&trozos);

            let mut buf = CappedBuffer::default();
            for t in &trozos {
                buf.push(t);
            }

            assert_eq!(buf.total, total, "total con trozos de {corte}");
            assert_eq!(buf.head, head, "cabeza con trozos de {corte}");
            assert_eq!(
                buf.tail.iter().copied().collect::<Vec<u8>>(),
                tail,
                "cola con trozos de {corte}"
            );
        }
    }

    /// Un trozo suelto más grande que la cola entera: la cola es su final, y
    /// lo que hubiera antes desaparece. Es el caso de `cat fichero_gigante`.
    #[test]
    fn un_trozo_mayor_que_la_cola_la_reemplaza_entera() {
        let mut buf = CappedBuffer::default();
        buf.push(&vec![b'a'; HEAD_BYTES]);
        buf.push(b"esto se pierde");
        let ultimo = vec![b'z'; TAIL_BYTES + 100];
        buf.push(&ultimo);

        assert_eq!(buf.tail.len(), TAIL_BYTES);
        assert!(buf.tail.iter().all(|&b| b == b'z'));
        assert_eq!(buf.total, HEAD_BYTES + 14 + ultimo.len());
    }

    fn evento_con(salida: String) -> CommandEvent {
        CommandEvent {
            id: 1,
            cmd: "cargo build".into(),
            cwd: "/x".into(),
            exit: Some(0),
            duration: Duration::from_secs(1),
            output: salida,
            truncated: false,
            omitted_bytes: 0,
        }
    }

    /// Lo que se le manda al modelo tiene tope; lo que se guarda, no.
    ///
    /// Es la diferencia que faltaba: el registro puede tener 64 KB de un
    /// `build` porque el usuario los mira, pero esos 64 KB dentro de la
    /// conversación del daemon se vuelven a pagar en cada turno posterior.
    #[test]
    fn la_salida_que_va_al_modelo_se_recorta() {
        let ev = evento_con((0..4000).map(|i| format!("línea {i}\n")).collect());
        let entero = ev.to_context();
        let acotado = ev.para_el_modelo(4 * 1024);
        assert!(entero.len() > 40_000, "el evento tenía que ser grande");
        assert!(acotado.len() < 5 * 1024, "no se recortó: {}", acotado.len());
        assert!(acotado.contains("del centro"));
        // Principio y final, que son los dos trozos que dicen algo.
        assert!(acotado.contains("línea 0\n"));
        assert!(acotado.contains("línea 3999\n"));
    }

    /// Recortar por el medio no puede partir una línea.
    #[test]
    fn el_recorte_cae_en_frontera_de_linea() {
        let ev = evento_con((0..4000).map(|i| format!("línea {i}\n")).collect());
        let a = ev.para_el_modelo(4 * 1024);
        for l in a.lines().filter(|l| l.starts_with("línea ")) {
            assert!(
                l.split_once(' ').unwrap().1.parse::<u32>().is_ok(),
                "línea partida: {l}"
            );
        }
    }

    /// Lo que ya cabe se queda igual: el tope no puede cambiar el caso normal.
    #[test]
    fn lo_corto_no_se_toca() {
        let ev = evento_con("dos\nlíneas\n".into());
        assert_eq!(ev.para_el_modelo(4 * 1024), ev.to_context());
    }

    /// El presupuesto de `context` no valía nada cuando el evento más
    /// reciente era justamente el que se pasaba: entraba entero.
    #[test]
    fn el_presupuesto_sujeta_tambien_al_mas_reciente() {
        let mut l = Ledger::new();
        l.begin("cargo build".into());
        for _ in 0..600 {
            l.feed_output(&[b'x'; 100]);
            l.feed_output(b"\n");
        }
        l.end(Some(0), Duration::from_millis(1));
        let c = l.context(6, 4 * 1024);
        assert!(c.len() < 5 * 1024, "sin recortar: {}", c.len());
    }
}
