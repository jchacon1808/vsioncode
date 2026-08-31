//! Clasificación de riesgo de un comando propuesto.
//!
//! ## Qué es esto y qué NO es
//!
//! Esto es un **cinturón, no el freno**. Hay evidencia sólida de que las
//! listas negras de comandos son estructuralmente frágiles: en un estudio
//! sobre 1.709 denylists reales de proyectos con agentes de código, entre el
//! 69 % y el 98,6 % **no bloqueaban del todo** la operación que pretendían
//! bloquear, y cubrir bien una sola operación exigía cientos de reglas.
//!
//! El freno de verdad en vsioncode es de diseño, no de listas: **escribir un
//! comando en tu línea no lo ejecuta**. Esta clasificación decide cuánta
//! información se te da antes de que pulses Enter, no si se te deja pulsar.
//!
//! ## Insertar siempre, avisar según el caso
//!
//! Una versión anterior se negaba a insertar los comandos marcados como
//! peligrosos. Era incoherente —si el gatillo es del humano, negarse a
//! *escribir* el texto no protege de nada— y en la práctica solo conseguía
//! que el usuario le pidiera lo mismo al agente tres veces seguidas hasta
//! copiarlo a mano. Un aviso que estorba se aprende a ignorar, y entonces ya
//! no avisa de nada.
//!
//! Ahora se inserta siempre. El nivel de riesgo pinta el aviso, y solo manda
//! de verdad en el modo de ejecución automática, donde sí hay que parar.
//!
//! ## `sudo` no es sinónimo de peligro
//!
//! Trabajar de administrador es trabajo normal: `sudo nmap -sV`, `sudo
//! systemctl status`, `sudo apt update`. Lo peligroso no es el `sudo`, es lo
//! que va detrás. Así que se clasifica **el comando de dentro** y `sudo` solo
//! sube un escalón desde «seguro»: informa de que corre como root, sin
//! declarar destructivo lo que no lo es.
//!
//! ## La regla que de verdad importa
//!
//! Un comando compuesto **hereda el nivel de su parte más peligrosa**. Es
//! justo ahí donde se rompen las listas negras: `ls && rm -rf ~` pasa el
//! filtro de «empieza por ls» de casi todas ellas.
//!
//! Un comando se encadena con `;`, `&&` o `|`, pero también se **anida**: lo
//! que va detrás de `find -exec` o de `xargs` es otro comando de verdad, y
//! hereda su nivel igual que las partes de uno compuesto. Y una bandera puede
//! sacar de la lista blanca al comando que la lleva: `sed -i` reescribe el
//! fichero que `sed` a secas solo leía.

/// Cuánta ceremonia merece un comando antes de tocarlo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    /// Solo lee. Se inserta sin ruido.
    Safe,
    /// Modifica algo. Se inserta y se marca; tú lees y pulsas.
    Caution,
    /// Destruye, escala privilegios o ejecuta código remoto. Exige
    /// confirmación escrita, no una tecla.
    Danger,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Safe => "seguro",
            Risk::Caution => "modifica",
            Risk::Danger => "peligroso",
        }
    }
}

/// Comandos que solo leen. Todo lo que no esté aquí es al menos `Caution`:
/// lista blanca, no negra, que es la única dirección que envejece bien.
const READ_ONLY: &[&str] = &[
    "ls",
    "ll",
    "la",
    "cat",
    "bat",
    "head",
    "tail",
    "less",
    "more",
    "wc",
    "file",
    "stat",
    "du",
    "df",
    "pwd",
    "whoami",
    "id",
    "date",
    "uptime",
    "uname",
    "hostname",
    "env",
    "printenv",
    "which",
    "type",
    "whereis",
    "echo",
    "printf",
    "grep",
    "rg",
    "egrep",
    "fgrep",
    "find",
    "fd",
    "locate",
    "ps",
    "top",
    "htop",
    "free",
    "vmstat",
    "iostat",
    "lsblk",
    "lsusb",
    "lspci",
    "ss",
    "netstat",
    "ip",
    "ifconfig",
    "ping",
    "dig",
    "host",
    "nslookup",
    "tree",
    "diff",
    "cmp",
    "sort",
    "uniq",
    "cut",
    "awk",
    "sed",
    "jq",
    "column",
    "man",
    "info",
    "help",
    "history",
    "sleep",
    "true",
    "false",
    "test",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "md5sum",
    "sha256sum",
];

/// Subcomandos de `git` que no cambian nada.
///
/// `config` y `stash` estuvieron en esta lista y no les correspondía: `git
/// config` escribe configuración salvo que solo pregunte, y `git stash` se
/// lleva el árbol de trabajo —con `drop` y `clear`, sin vuelta atrás—. Los dos
/// dependen de lo que venga detrás, así que se miran en [`clasifica_git`].
const GIT_READ_ONLY: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "branch",
    "remote",
    "blame",
    "describe",
    "rev-parse",
    "ls-files",
    "shortlog",
    "reflog",
];

/// Banderas con las que `git config` solo pregunta.
const GIT_CONFIG_PREGUNTA: &[&str] = &[
    "--get",
    "--get-all",
    "--get-regexp",
    "--get-urlmatch",
    "--list",
    "-l",
];

/// Subcomandos de `cargo` que no salen del proyecto.
///
/// `cargo` entero no cabe en [`READ_ONLY`] —`cargo install` mete software en
/// la máquina—, pero compilar y probar es el trabajo de todos los días:
/// pintarlo de amarillo veinte veces al día es justo el aviso que se aprende a
/// ignorar. Lo que escriben cae dentro de `target/`, que es desechable por
/// definición; lo que toca las fuentes (`fmt`, `fix`, `--fix`) no está aquí.
const CARGO_SOLO_LEE: &[&str] = &[
    "test", "check", "build", "bench", "clippy", "doc", "tree", "metadata", "search", "version",
];

/// Verbos con los que `ip` cambia la red.
///
/// Se mira lo que escribe y no lo que lee, al revés que en el resto del
/// módulo, porque `ip` acepta **prefijos**: `ip a s` es `ip addr show`, y así
/// es como se teclea de verdad. Con una lista blanca de verbos completos,
/// `ip a s` y `ip r l` —de los comandos más escritos mientras se depura una
/// red— salían marcados, que es exactamente el aviso que se aprende a
/// ignorar.
///
/// El precio está medido y asumido: un verbo de escritura abreviado a una o
/// dos letras (`ip a d 10.0.0.1/24 dev eth0`) no se reconoce y pasa como
/// seguro. Se acepta porque quien abrevia hasta ahí sabe lo que hace, y
/// porque esto es un cinturón y no el freno: lo que de verdad protege es que
/// escribir un comando en tu línea no lo ejecuta.
const IP_ESCRIBE: &[&str] = &[
    "add", "append", "change", "chg", "replace", "del", "delete", "set", "flush", "restore",
];

/// Comandos que nunca deberían ejecutarse de un tirón sin leerlos dos veces.
const DANGEROUS: &[&str] = &[
    "rm",
    "rmdir",
    "dd",
    "mkfs",
    "mkfs.ext4",
    "mkfs.xfs",
    "fdisk",
    "parted",
    "shred",
    "wipefs",
    "userdel",
    "groupdel",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
];

/// Intérpretes que reciben el comando de verdad dentro de una cadena.
///
/// `bash -c "rm -rf ~"` salía «modifica»: la cabecera es `bash`, un comando
/// desconocido, y lo que iba entre comillas no lo miraba nadie. Era el
/// agujero más grande del módulo, y el más fácil de encontrar para quien
/// quisiera esquivarlo.
///
/// No entra `ssh`, y es deliberado: `ssh host 'rm -rf /'` borra en **otra**
/// máquina. Avisar ahí exigiría decidir qué significa el riesgo cuando el
/// sistema afectado no es este, y esa es una pregunta distinta de la que
/// responde este módulo.
const INTERPRETES: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish"];

/// Programas que no hacen nada por sí mismos: lanzan otro.
///
/// Es la misma anidación que `sudo`, sin los privilegios. Sin reconocerlos,
/// `nohup rm -rf ~` se quedaba en «modifica» porque la cabecera era `nohup`,
/// un comando desconocido — y el `rm` de detrás no lo miraba nadie. A
/// diferencia de `sudo`, estos **no** suben el nivel por sí solos: envolver
/// un `ls` en `nice` no lo hace más peligroso, así que el veredicto del
/// comando de dentro pasa tal cual.
///
/// Cada uno con sus banderas de valor, porque el valor se confundiría con la
/// cabecera del comando envuelto.
const ENVOLTORIOS: &[(&str, &[&str])] = &[
    ("nohup", &[]),
    ("setsid", &[]),
    (
        "stdbuf",
        &["-i", "-o", "-e", "--input", "--output", "--error"],
    ),
    ("nice", &["-n", "--adjustment"]),
    (
        "ionice",
        &["-c", "--class", "-n", "--classdata", "-p", "--pid"],
    ),
    ("time", &["-f", "--format", "-o", "--output"]),
    ("watch", &["-n", "--interval"]),
    ("timeout", &["-s", "--signal", "-k", "--kill-after"]),
];

/// Banderas de `sudo` y compañía que llevan un valor detrás.
///
/// Sin esta lista, el valor se toma por la cabecera del comando anidado y todo
/// lo que va después deja de mirarse.
const PRIVILEGED_CON_VALOR: &[&str] = &[
    "-u",
    "--user",
    "-g",
    "--group",
    "-U",
    "-C",
    "--close-from",
    "-p",
    "--prompt",
    "-r",
    "--role",
    "-t",
    "--type",
    "-D",
    "--chdir",
    "-R",
    "--chroot",
];

/// Elevan privilegios. No son peligrosos por sí mismos —administrar es
/// trabajo normal—, pero conviene que se vea que la orden corre como root.
const PRIVILEGED: &[&str] = &["sudo", "doas", "su", "pkexec", "runuser"];

/// Cambian el sistema sin destruir nada por sí solos. Merecen «modifica», no
/// «peligroso»: marcarlos de rojo todos los días entrena a ignorar el rojo.
const SYSTEM_CHANGING: &[&str] = &[
    "systemctl",
    "service",
    "mount",
    "umount",
    "iptables",
    "nft",
    "ufw",
    "crontab",
    "usermod",
    "chown",
    "chgrp",
    "passwd",
    "chpasswd",
    "swapoff",
    "kill",
    "killall",
    "pkill",
    "modprobe",
];

/// Gestores de paquetes, y los verbos con los que instalan o quitan cosas.
///
/// No suben el riesgo —un `npm install` no es peligroso, se hace veinte veces
/// al día— pero **no los lanza el agente por su cuenta**: instalar mete código
/// de fuera en la máquina, y esa decisión es del dueño de la máquina. Ver
/// [`instala_software`].
///
/// El verbo importa tanto como el programa: `apt list`, `pip show` y
/// `cargo build` no instalan nada, y frenarlos por empezar por la misma
/// palabra sería exactamente el aviso que sobra.
const GESTORES: &[&str] = &[
    "apt", "apt-get", "aptitude", "dpkg", "snap", "flatpak", "dnf", "yum", "zypper", "pacman",
    "apk", "brew", "port", "npm", "pnpm", "yarn", "bun", "pip", "pip3", "pipx", "uv", "gem",
    "cargo", "go", "rustup", "nix-env", "asdf", "conda", "composer",
];

/// Verbos con los que un gestor mete o quita software.
const VERBOS_INSTALAN: &[&str] = &[
    "install",
    "uninstall",
    "reinstall",
    "remove",
    "erase",
    "purge",
    "add",
    "del",
    "delete",
    "upgrade",
    "dist-upgrade",
    "full-upgrade",
    "-s", // pacman -S, apk -s
    "-r",
    "-u",
];

/// ¿Este comando instala o quita software? Devuelve el motivo, para poder
/// decírtelo con palabras en vez de con una alarma.
///
/// Se mira parte por parte, igual que el riesgo: un `ls && pip install x`
/// instala tanto como el `pip install x` a secas.
pub fn instala_software(cmd: &str) -> Option<String> {
    for part in split_parts(cmd) {
        let mut words: Vec<&str> = part.split_whitespace().collect();
        while let Some(w) = words.first() {
            if w.contains('=') && !w.starts_with('-') {
                words.remove(0);
            } else {
                break;
            }
        }
        // `sudo apt install` es lo mismo que `apt install` a estos efectos.
        if let Some(w) = words.first()
            && PRIVILEGED.contains(&nombre(w))
        {
            words.remove(0);
            while let Some(w) = words.first() {
                if w.starts_with('-') {
                    words.remove(0);
                } else {
                    break;
                }
            }
        }
        let Some(head) = words.first().copied() else {
            continue;
        };
        let gestor = nombre(head).to_ascii_lowercase();
        if !GESTORES.contains(&gestor.as_str()) {
            continue;
        }
        if words
            .iter()
            .skip(1)
            .any(|w| VERBOS_INSTALAN.contains(&w.to_ascii_lowercase().as_str()))
        {
            return Some(format!("«{gestor}» instala o quita software"));
        }
    }
    None
}

/// Patrones que hacen peligroso cualquier comando que los contenga.
///
/// **Van en minúsculas obligatoriamente**: se comparan contra el comando ya
/// pasado a minúsculas, así que un patrón con una mayúscula no casaría nunca
/// y fallaría en silencio, que es la peor forma de fallar en una lista de
/// seguridad. Hay un test que lo verifica.
const DANGEROUS_PATTERNS: &[(&str, &str)] = &[
    ("rm -rf /", "borrado recursivo desde la raíz"),
    ("rm -fr /", "borrado recursivo desde la raíz"),
    (":(){", "fork bomb"),
    ("> /dev/sd", "escritura directa a un disco"),
    ("of=/dev/", "escritura directa a un dispositivo"),
    (
        "git reset --hard",
        "descarta cambios sin posibilidad de recuperarlos",
    ),
    ("git push --force", "reescribe la historia remota"),
    ("git push -f ", "reescribe la historia remota"),
    ("git clean -fd", "borra ficheros sin seguimiento"),
    ("chmod 777 /", "permisos abiertos en la raíz del sistema"),
];

/// Patrones que suben a «modifica» sin llegar a peligrosos.
///
/// Aquí estuvo `-f `, y hacía más daño que bien: en `tar -xzf`, `grep -f
/// patrones.txt` o `sudo -f` esa letra no fuerza nada, y el aviso salía
/// siempre. `--force` sí es inequívoco y se queda.
const CAUTION_PATTERNS: &[(&str, &str)] = &[
    ("--force", "fuerza una operación que la herramienta rechazó"),
    ("chmod", "cambia permisos"),
    ("/etc/", "toca configuración del sistema"),
];

/// Motivo por el que un comando subió de nivel. Sirve para explicárselo al
/// usuario: una advertencia sin razón se ignora a la tercera.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub risk: Risk,
    pub reason: Option<String>,
}

/// Clasifica un comando completo, tal y como se escribiría en el shell.
pub fn classify(cmd: &str) -> Verdict {
    let lower = cmd.to_ascii_lowercase();

    // 1. Patrones que condenan el comando entero, aparezcan donde aparezcan.
    for (pat, why) in DANGEROUS_PATTERNS {
        if lower.contains(pat) {
            return Verdict {
                risk: Risk::Danger,
                reason: Some((*why).to_string()),
            };
        }
    }

    // 2. Descarga ejecutada directamente. El clásico `curl … | sh`: instala
    //    código que nadie ha leído, y ni siquiera queda registro de qué era.
    if (lower.contains("curl ") || lower.contains("wget ") || lower.contains("fetch "))
        && lower.contains('|')
        && [
            "| sh", "|sh", "| bash", "|bash", "| zsh", "|zsh", "| python", "|python",
        ]
        .iter()
        .any(|p| lower.contains(p))
    {
        return Verdict {
            risk: Risk::Danger,
            reason: Some("ejecuta directamente código descargado de internet".into()),
        };
    }

    // 3. Patrones que merecen un aviso pero no una alarma.
    let mut piso = Risk::Safe;
    let mut motivo_piso = None;
    for (pat, why) in CAUTION_PATTERNS {
        if lower.contains(pat) {
            piso = Risk::Caution;
            motivo_piso = Some((*why).to_string());
            break;
        }
    }

    // 4. Cada parte del comando por separado; gana la peor.
    //    Aquí es donde se cazan los `ls && rm -rf ~` que las listas negras
    //    ingenuas dejan pasar por mirar solo la primera palabra.
    let mut worst = Verdict {
        risk: piso,
        reason: motivo_piso,
    };
    for part in split_parts(cmd) {
        let v = classify_simple(part);
        if v.risk > worst.risk {
            worst = v;
        }
    }
    worst
}

/// Parte por los separadores que encadenan comandos: `;`, `&&`, `||`, `|`, y
/// el contenido de las sustituciones `$(...)` y de los backticks.
fn split_parts(cmd: &str) -> Vec<&str> {
    // Lo que va entre comillas es texto, no estructura.
    //
    // Sin esto, `awk '{print; x = 1}'` se partía en dos y la cabecera `awk`
    // se quedaba en el primer trozo: el segundo era un comando desconocido
    // —«modifica»— y el programa de awk no lo leía nadie. Lo mismo con
    // cualquier paréntesis, que es como se escribe un `if`. Y es peor de lo
    // que parece: cuando el que pierde la cabecera es `bash -c 'algo (x)'`,
    // lo que se cae es justo el reconocimiento del intérprete.
    //
    // Es el mismo fallo que el `&` de `2>&1`, con otra cara: cortar por un
    // símbolo sin mirar si el shell lo vería como tal.
    let estructura = {
        let mut e = vec![false; cmd.len()];
        for i in indices_sin_comillas(cmd) {
            e[i] = true;
        }
        e
    };

    let mut parts = Vec::new();
    let bytes = cmd.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if !estructura[i] {
            i += 1;
            continue;
        }
        let (is_sep, width) = match bytes[i] {
            // El `&` de `2>&1` no encadena nada: es parte de la redirección.
            //
            // Partir ahí dejaba `1` como si fuera un comando suelto, y un
            // comando desconocido cuenta «modifica» — así que `ls -la 2>&1`
            // salía marcado. Es la coletilla más tecleada de cualquier
            // terminal, y aparece en el propio `CLAUDE.md` del proyecto: el
            // aviso que sobra, en su forma más pura.
            b'&' if i > 0 && bytes[i - 1] == b'>' => (false, 0),
            b';' | b'|' | b'&' | b'\n' => {
                // `&&` y `||` cuentan igual que uno solo.
                let w = if i + 1 < bytes.len() && bytes[i + 1] == bytes[i] {
                    2
                } else {
                    1
                };
                (true, w)
            }
            // Abre sustitución: `$(` o backtick. Tratamos lo de dentro como un
            // comando más, que es exactamente lo que es.
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => (true, 2),
            b'(' | b')' | b'`' => (true, 1),
            _ => (false, 0),
        };
        if is_sep {
            if i > start {
                parts.push(cmd[start..i].trim());
            }
            i += width;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < cmd.len() {
        parts.push(cmd[start..].trim());
    }
    parts.retain(|p| !p.is_empty());
    if parts.is_empty() {
        parts.push(cmd.trim());
    }
    parts
}

/// Clasifica un comando simple (sin encadenamientos).
fn classify_simple(part: &str) -> Verdict {
    let mut words: Vec<&str> = part.split_whitespace().collect();

    // Saltamos asignaciones de entorno del principio (`FOO=1 comando`).
    while let Some(w) = words.first() {
        if w.contains('=') && !w.starts_with('-') {
            words.remove(0);
        } else {
            break;
        }
    }

    let Some(head) = words.first().copied() else {
        return Verdict {
            risk: Risk::Safe,
            reason: None,
        };
    };
    let name = nombre(head);

    // `sudo`: lo que importa es lo que va detrás.
    //
    // Clasificar `sudo` como destructivo de plano convertía en «peligroso»
    // cualquier tarea de administración normal. Aquí se clasifica el comando
    // real y `sudo` solo sube desde «seguro»: se informa de que corre como
    // root sin declarar destructivo lo que no lo es.
    if PRIVILEGED.contains(&name) {
        // `salta_banderas` y no un `skip_while`: `-u` lleva un usuario detrás,
        // y saltando solo lo que empieza por guion la cabecera acababa siendo
        // el nombre del usuario. `sudo -u pepe rm -rf /tmp/x` se quedaba en
        // «modifica» porque «pepe» es un comando desconocido.
        let resto = salta_banderas(&words[1..], PRIVILEGED_CON_VALOR);

        if resto.is_empty() {
            return Verdict {
                risk: Risk::Caution,
                reason: Some("abre una sesión como root".into()),
            };
        }

        let dentro = classify_simple(&resto.join(" "));
        let risk = dentro.risk.max(Risk::Caution);
        let reason = dentro
            .reason
            .or_else(|| Some(format!("«{}» se ejecuta como root", nombre(resto[0]))));
        return Verdict { risk, reason };
    }

    // `bash -c "…"`: lo que cuenta es lo que va dentro de la cadena.
    if INTERPRETES.contains(&name)
        && let Some(guion) = tras_la_c(&words[1..])
    {
        return classify(&guion);
    }

    // Envoltorios: lo que importa es el comando que lanzan, igual que con
    // `sudo`, pero sin subir el nivel por el hecho de envolver.
    if let Some((_, con_valor)) = ENVOLTORIOS.iter().find(|(n, _)| *n == name) {
        let mut resto = salta_banderas(&words[1..], con_valor);
        // `timeout` lleva la duración como primer argumento suelto, y sin
        // saltarla la cabecera del comando envuelto sería «5».
        if name == "timeout" && resto.first().is_some_and(|w| es_duracion(w)) {
            resto.remove(0);
        }
        if !resto.is_empty() {
            return classify_simple(&resto.join(" "));
        }
    }

    if DANGEROUS.contains(&name) {
        return Verdict {
            risk: Risk::Danger,
            reason: Some(format!("«{name}» puede destruir datos sin vuelta atrás")),
        };
    }

    if SYSTEM_CHANGING.contains(&name) {
        return Verdict {
            risk: Risk::Caution,
            reason: Some(format!("«{name}» cambia el estado del sistema")),
        };
    }

    // Antes que la lista blanca: una bandera puede haber sacado al comando de
    // ella, y lo que va detrás de un `-exec` o de un `xargs` es otro comando
    // con su propio nivel. La redirección que pisa un fichero compite con
    // ellos y gana la peor, que es la misma regla de los comandos encadenados:
    // `find . -exec rm {} \; > log` es el `rm`, no el `>`.
    let por_dentro = por_sus_banderas(name, &words);
    let redirige =
        trunca_un_fichero(part).then(|| veredicto(Risk::Caution, "sobrescribe un fichero"));
    let peor = match (por_dentro, redirige) {
        (Some(a), Some(b)) => Some(if a.risk >= b.risk { a } else { b }),
        (a, b) => a.or(b),
    };
    if let Some(v) = peor {
        return v;
    }

    if name == "git" {
        return clasifica_git(&words);
    }

    if name == "cargo"
        && CARGO_SOLO_LEE.contains(&words.get(1).copied().unwrap_or(""))
        // `cargo clippy --fix` reescribe las fuentes; ya no es una consulta.
        && !words.contains(&"--fix")
    {
        return Verdict {
            risk: Risk::Safe,
            reason: None,
        };
    }

    if READ_ONLY.contains(&name) {
        return Verdict {
            risk: Risk::Safe,
            reason: None,
        };
    }

    Verdict {
        risk: Risk::Caution,
        reason: None,
    }
}

/// El nombre del ejecutable, sin la ruta. Que esté en `/bin` no lo disfraza.
fn nombre(palabra: &str) -> &str {
    palabra.rsplit('/').next().unwrap_or(palabra)
}

/// Atajo para los veredictos con motivo, que son casi todos.
fn veredicto(risk: Risk, por_que: impl Into<String>) -> Verdict {
    Verdict {
        risk,
        reason: Some(por_que.into()),
    }
}

/// `git` no es un comando, son cincuenta. El nivel lo decide el subcomando, y
/// en dos de ellos hace falta mirar una palabra más.
fn clasifica_git(words: &[&str]) -> Verdict {
    let sub = words.get(1).copied().unwrap_or("");
    let resto = words.get(2..).unwrap_or(&[]);
    let siguiente = resto.first().copied().unwrap_or("");

    let solo_lee = match sub {
        // Guardar el trabajo y limpiar el árbol no es leer; tirar lo guardado
        // no tiene deshacer.
        "stash" => {
            if matches!(siguiente, "drop" | "clear") {
                return veredicto(
                    Risk::Danger,
                    format!("«git stash {siguiente}» tira trabajo guardado sin vuelta atrás"),
                );
            }
            matches!(siguiente, "list" | "show")
        }
        // Escribe la configuración salvo que solo pregunte por ella.
        "config" => resto.iter().any(|w| {
            GIT_CONFIG_PREGUNTA.contains(w) || GIT_CONFIG_PREGUNTA.iter().any(|g| w.starts_with(g))
        }),
        _ => GIT_READ_ONLY.contains(&sub),
    };

    if solo_lee {
        Verdict {
            risk: Risk::Safe,
            reason: None,
        }
    } else {
        veredicto(
            Risk::Caution,
            format!("«git {sub}» modifica el repositorio"),
        )
    }
}

/// Lo que la primera palabra no cuenta: banderas que sacan al comando de la
/// lista blanca y banderas que esconden otro comando detrás.
///
/// Mirar solo la cabecera daba por lectura `sed -i` —que reescribe el
/// fichero—, `find -delete` —que borra— y `find -exec rm -rf {}` —que borra
/// recursivamente—. Lo anidado se clasifica como lo que es y hereda su nivel;
/// por eso el motivo que sale de `-exec rm` habla de `rm` y no de `find`.
///
/// Devuelve `None` cuando no hay nada que objetar, para que el comando siga el
/// camino de siempre. Cuando lo que hay dentro es el comando entero —`xargs
/// rm`, `env ls`— devuelve su veredicto tal cual, «seguro» incluido: quien
/// manda es el de dentro.
fn por_sus_banderas(name: &str, words: &[&str]) -> Option<Verdict> {
    let args = words.get(1..).unwrap_or(&[]);

    match name {
        // Reescribe en el sitio el fichero que solo leía.
        "sed" => args
            .iter()
            .any(|w| bandera_corta(w, b'i') || w.starts_with("--in-place"))
            .then(|| veredicto(Risk::Caution, "«sed -i» reescribe el fichero en su sitio")),

        // `awk` no es un comando con banderas: es un lenguaje, y lo que hace
        // está escrito dentro de las comillas.
        "awk" | "gawk" => lo_que_hace_awk(args),

        "find" | "fd" | "fdfind" => lo_que_hace_find(name, args),

        // Su razón de ser es ejecutar lo que le llega por la tubería.
        "xargs" => {
            let resto = salta_banderas(args, XARGS_CON_VALOR);
            (!resto.is_empty()).then(|| classify_simple(&resto.join(" ")))
        }

        // `env` también arranca un comando: `env FOO=1 rm -rf x` borra igual.
        "env" => {
            let resto = salta_banderas(args, &["-u", "--unset"]);
            (!resto.is_empty()).then(|| classify_simple(&resto.join(" ")))
        }

        // Las puertas que sacan de POSIX, que es el único mundo que este
        // módulo sabe leer. Desde Git Bash se llama a Windows con estas tres.
        //
        // **No suben el nivel**: un comando desconocido ya cae en «modifica»,
        // y `powershell -c "Get-Date"` es una consulta —marcarlo de rojo sería
        // el aviso que sobra—. Lo que se añade es el motivo, que es lo que
        // aquí de verdad falta: que lo de dentro **no se ha clasificado**.
        // Es lo contrario de `bash -c`, donde sí se mira dentro.
        "cmd" | "cmd.exe" | "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" => {
            (!args.is_empty()).then(|| {
                veredicto(
                    Risk::Caution,
                    format!("«{name}» sale del shell, y lo que hace dentro no se puede leer"),
                )
            })
        }

        // El registro de Windows es `/etc` y media configuración del sistema
        // en un solo sitio, y `reg delete` no tiene papelera ni confirmación.
        // Manda el verbo, como en `ip`: `reg query` lee y no molesta.
        "reg" | "reg.exe" => match args.first().map(|v| v.to_ascii_lowercase()).as_deref() {
            Some("delete") => Some(veredicto(
                Risk::Danger,
                "«reg delete» borra del registro de Windows, y de ahí no se vuelve",
            )),
            Some("add" | "copy" | "import" | "restore" | "load" | "unload") => Some(veredicto(
                Risk::Caution,
                "«reg» cambia el registro de Windows",
            )),
            _ => None,
        },

        // `ip addr` lista y `ip addr add` cambia la red: manda el verbo.
        //
        // El verbo cuenta desde tres letras: `add`, `del`, `set`. Menos que
        // eso es la abreviatura de una consulta mucho más a menudo que la de
        // un cambio —`ip a s`, `ip r l`— y marcarlas era el aviso que sobra.
        "ip" => {
            let objeto = salta_banderas(args, &[]);
            match (objeto.first(), objeto.get(1)) {
                // `ip netns exec vpn <cmd>` no toca la red: lanza otro comando
                // dentro de ella. Manda el de dentro, como en `nice` o `nohup`
                // — así `ip netns exec vpn rm -rf /` sale por lo que es y no
                // se queda en el «modifica» de un cambio de red cualquiera.
                (Some(obj), Some(verbo)) if es_netns_exec(obj, verbo) => {
                    let dentro = objeto.get(3..).unwrap_or(&[]);
                    Some(if dentro.is_empty() {
                        veredicto(
                            Risk::Caution,
                            "«ip netns exec» entra en otro espacio de red",
                        )
                    } else {
                        classify_simple(&dentro.join(" "))
                    })
                }
                (Some(obj), Some(verbo)) if ip_escribe(verbo) => Some(veredicto(
                    Risk::Caution,
                    format!("«ip {obj} {verbo}» cambia la configuración de red"),
                )),
                _ => None,
            }
        }

        // El `ip` de antes: `ifconfig eth0` informa, `ifconfig eth0 down` no.
        "ifconfig" => (args.iter().filter(|w| !w.starts_with('-')).count() >= 2).then(|| {
            veredicto(
                Risk::Caution,
                "«ifconfig» con argumentos reconfigura la interfaz",
            )
        }),

        // Un argumento suelto no es una consulta: es el nombre nuevo.
        "hostname" => args
            .iter()
            .any(|w| !w.starts_with('-'))
            .then(|| veredicto(Risk::Caution, "«hostname» con un nombre detrás lo cambia")),

        // Preguntar la hora es leer; ponerla, no. Solo la bandera exacta:
        // `date -Is` también lleva una ese y solo imprime.
        "date" => args
            .iter()
            .any(|w| *w == "-s" || w.starts_with("--set"))
            .then(|| veredicto(Risk::Caution, "«date -s» cambia la hora del sistema")),

        // Ordenar es leer, pero `-o` escribe —y puede ser el mismo fichero—.
        "sort" => args
            .iter()
            .any(|w| *w == "-o" || w.starts_with("--output"))
            .then(|| {
                veredicto(
                    Risk::Caution,
                    "«sort -o» escribe el resultado en un fichero",
                )
            }),

        "tree" => args
            .iter()
            .any(|w| *w == "-o" || w.starts_with("--output"))
            .then(|| {
                veredicto(
                    Risk::Caution,
                    "«tree -o» escribe el resultado en un fichero",
                )
            }),

        _ => None,
    }
}

/// Banderas de `xargs` que se llevan por delante la palabra siguiente: sin
/// esto, el comando anidado de `xargs -n 1 rm` parecería llamarse «1».
const XARGS_CON_VALOR: &[&str] = &[
    "-n", "-I", "-i", "-L", "-l", "-P", "-s", "-d", "-E", "-e", "-a",
];

/// `find` y `fd`: el buscador que además borra o ejecuta.
fn lo_que_hace_find(name: &str, args: &[&str]) -> Option<Verdict> {
    let ejecuta: &[&str] = if name == "find" {
        &["-exec", "-execdir", "-ok", "-okdir"]
    } else {
        &["-x", "--exec", "-X", "--exec-batch"]
    };

    let mut peor: Option<Verdict> = None;
    for (i, w) in args.iter().enumerate() {
        let v = if name == "find" && *w == "-delete" {
            Some(veredicto(
                Risk::Danger,
                "«find -delete» borra todo lo que encuentre",
            ))
        } else if name == "find" && matches!(*w, "-fprint" | "-fprintf" | "-fls") {
            Some(veredicto(
                Risk::Caution,
                format!("«find {w}» escribe un fichero"),
            ))
        } else if ejecuta.contains(w) {
            let anidado = hasta_el_cierre(&args[i + 1..]);
            (!anidado.is_empty()).then(|| classify_simple(&anidado.join(" ")))
        } else {
            None
        };
        if let Some(v) = v
            && peor.as_ref().is_none_or(|p| v.risk > p.risk)
        {
            peor = Some(v);
        }
    }
    peor
}

/// El comando que va detrás de un `-exec`, sin lo que lo cierra.
///
/// El `\;` no llega entero: `split_parts` ya ha partido por ese `;` y deja una
/// barra suelta al final. Si el cierre acabara dentro del comando anidado, la
/// clasificación miraría un comando que nadie escribió.
fn hasta_el_cierre<'a>(args: &[&'a str]) -> Vec<&'a str> {
    args.iter()
        .take_while(|w| !matches!(**w, ";" | "\\;" | "\\" | "+"))
        .filter(|w| **w != "{}")
        .copied()
        .collect()
}

/// Se salta las banderas de cabecera y devuelve lo que viene detrás, que es
/// donde empieza el comando anidado.
fn salta_banderas<'a>(args: &[&'a str], con_valor: &[&str]) -> Vec<&'a str> {
    let mut i = 0;
    while let Some(w) = args.get(i) {
        if w.len() > 1 && w.starts_with('-') {
            if con_valor.contains(w) {
                i += 1;
            }
            i += 1;
        } else {
            break;
        }
    }
    args[i.min(args.len())..].to_vec()
}

/// ¿La palabra es un grupo de banderas cortas que incluye `letra`?
///
/// `-i`, `-ni` y `-i.bak` sí; `--include` no, y un argumento suelto tampoco.
/// Se para en el primer carácter que no es una letra porque ahí acaba el
/// grupo y empieza el valor pegado (`-i.bak`).
fn bandera_corta(palabra: &str, letra: u8) -> bool {
    let Some(resto) = palabra.strip_prefix('-') else {
        return false;
    };
    if resto.starts_with('-') {
        return false;
    }
    resto
        .bytes()
        .take_while(|b| b.is_ascii_alphabetic())
        .any(|b| b == letra)
}

/// Índices de byte que quedan **fuera** de comillas simples y dobles.
///
/// Se escribe a mano y en pequeño a propósito. Un troceador de shell completo
/// resolvería más casos, pero sería mucha más superficie donde equivocarse
/// justo en el módulo que decide la seguridad, y equivocarse aquí no se ve:
/// solo se nota cuando el aviso falta.
fn indices_sin_comillas(texto: &str) -> Vec<usize> {
    let bytes = texto.as_bytes();
    let mut fuera = Vec::new();
    let mut comilla: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match comilla {
            None => match b {
                b'\'' | b'"' => comilla = Some(b),
                // Una barra invertida anula el carácter siguiente: en
                // `echo a \> b` ese `>` es texto.
                b'\\' => i += 1,
                _ => fuera.push(i),
            },
            Some(q) => {
                if b == q {
                    comilla = None;
                } else if q == b'"' && b == b'\\' {
                    i += 1;
                }
            }
        }
        i += 1;
    }
    fuera
}

/// ¿Hay una redirección que **pise** un fichero?
///
/// La comprobación anterior era `part.contains('>')`, y avisaba de `echo 'a >
/// b'`, de `awk '$1 > 3'` y de `cargo test 2>/dev/null`: tres avisos que
/// sobran, de los que enseñan a ignorar el que sí importa. Cuentan como
/// redirección que trunca solo los `>` de verdad:
///
/// - `>>` añade y `>&` reencamina un descriptor: ninguno pisa nada.
/// - `>=`, `->` y `=>` no son redirecciones, son comparaciones y flechas.
/// - `/dev/null` y compañía no guardan nada, venga el descriptor que venga.
///   `2>errores.log` sí trunca `errores.log`, y por eso sí avisa: lo que
///   decide no es el descriptor, es el destino.
/// - Lo que está entre comillas es texto.
/// - Un `>` sin nada detrás no trunca nada, porque no hay fichero que
///   truncar. Parece un caso imposible —el shell ni lo aceptaría— y es el
///   más frecuente de todos: [`split_parts`] corta por `&`, así que el
///   `2>&1` que lleva pegado media terminal llega hasta aquí convertido en
///   un `>` huérfano. Sin esta condición, `ls -la 2>&1` avisaba de que
///   sobrescribe un fichero.
fn trunca_un_fichero(part: &str) -> bool {
    let bytes = part.as_bytes();
    indices_sin_comillas(part).into_iter().any(|i| {
        bytes[i] == b'>'
            && !matches!(bytes.get(i + 1), Some(b'>') | Some(b'&') | Some(b'='))
            && !(i > 0 && matches!(bytes[i - 1], b'-' | b'=' | b'>'))
            && hay_destino(&part[i + 1..])
            && !destino_que_no_guarda(&part[i + 1..])
    })
}

/// ¿Hay un fichero detrás del `>`?
fn hay_destino(resto: &str) -> bool {
    !resto.trim_start_matches('|').trim().is_empty()
}

/// El guion que va detrás de `-c`, ya sin las comillas que lo envuelven.
///
/// Se devuelve para pasarlo por [`classify`] entero y no por `classify_simple`:
/// lo de dentro puede ser una línea compuesta —`cd /tmp && rm -rf x`— y ahí
/// vuelve a valer la regla de que hereda el nivel de su parte más peligrosa.
fn tras_la_c(args: &[&str]) -> Option<String> {
    let i = args.iter().position(|w| *w == "-c")?;
    // Todo lo que sigue, junto: la cadena ya venía partida por espacios, así
    // que quedarse con la palabra siguiente daría `"rm` en vez de `rm -rf ~`.
    let guion = args.get(i + 1..)?.join(" ");
    let recortado = guion.trim();
    let sin_comillas = recortado
        .strip_prefix('"')
        .and_then(|g| g.strip_suffix('"'))
        .or_else(|| {
            recortado
                .strip_prefix('\'')
                .and_then(|g| g.strip_suffix('\''))
        })
        .unwrap_or(recortado);
    (!sin_comillas.trim().is_empty()).then(|| sin_comillas.to_string())
}

/// `5`, `5s`, `1.5m`: el primer argumento de `timeout`.
fn es_duracion(palabra: &str) -> bool {
    let numero = palabra.trim_end_matches(['s', 'm', 'h', 'd']);
    !numero.is_empty() && numero.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Lo que hace un programa de `awk`, que va escrito **dentro de las comillas**.
///
/// Es el único comando del módulo donde hay que leer un lenguaje entero, y por
/// eso vive aparte. Tres cosas distintas sacan a `awk` de la lectura, y solo
/// una de ellas se ve desde fuera:
///
/// - `-i inplace` reescribe el fichero que lee. Es una bandera, se ve.
/// - `print > "salida.txt"` escribe un fichero nuevo. **No** se ve:
///   [`redirige_pisando`] no mira dentro de las comillas, y hace bien —ahí un
///   `>` es la comparación de `$1 > 3` mucho más a menudo que una
///   redirección—. Lo que distingue una de otra es de qué lado del `print`
///   cae.
/// - `system("…")` ejecuta un comando: aquí `awk` es un intérprete como los
///   de [`INTERPRETES`], y manda lo que lleva dentro.
///
/// Queda fuera `print | "sh"`, la tubería a un comando. Es la misma ejecución
/// que `system`, pero reconocerla obliga a distinguir el destino de una
/// tubería del de un fichero dentro del programa, y equivocarse ahí devuelve
/// justo el aviso que sobra. La puerta que de verdad se usa es `system`.
fn lo_que_hace_awk(args: &[&str]) -> Option<Verdict> {
    let inplace = args
        .windows(2)
        .any(|p| matches!(p[0], "-i" | "--include" | "--load" | "-l") && p[1] == "inplace")
        || args
            .iter()
            .any(|w| matches!(*w, "-iinplace" | "--include=inplace" | "--load=inplace"));
    if inplace {
        return Some(veredicto(
            Risk::Caution,
            "«awk -i inplace» reescribe el fichero en su sitio",
        ));
    }

    // El programa llega troceado por espacios como todo lo demás, así que se
    // vuelve a juntar: `'{print > "x"}'` son tres palabras sueltas.
    let texto = args.join(" ");

    if let Some(dentro) = entre_comillas_de(&texto, "system(") {
        return Some(classify_simple(&dentro));
    }

    awk_escribe(&texto)
        .then(|| veredicto(Risk::Caution, "«awk» escribe en un fichero con «print >»"))
}

/// ¿Hay un `print` con un destino detrás?
///
/// El destino es lo que va **después** del `print` y antes de que acabe su
/// sentencia. `$1 > 3 {print}` compara y no escribe; `{print > "x"}` escribe.
/// La sentencia acaba en `;`, en `}` o en el final del programa.
fn awk_escribe(programa: &str) -> bool {
    let mut resto = programa;
    while let Some(p) = resto.find("print") {
        let tras_print = &resto[p + "print".len()..];
        let fin = tras_print
            .find([';', '}', '\n'])
            .unwrap_or(tras_print.len());
        if tras_print[..fin].contains('>') {
            return true;
        }
        resto = tras_print;
    }
    false
}

/// El argumento de una llamada como `system("rm -rf x")`.
///
/// Se busca la comilla que abre justo después del nombre y su pareja, sin
/// intentar entender el resto del lenguaje: lo que se quiere es el comando,
/// no analizar `awk`.
fn entre_comillas_de(texto: &str, llamada: &str) -> Option<String> {
    let tras = &texto[texto.find(llamada)? + llamada.len()..];
    let comilla = tras.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let cuerpo = &tras[comilla.len_utf8()..];
    let dentro = &cuerpo[..cuerpo.find(comilla)?];
    (!dentro.trim().is_empty()).then(|| dentro.to_string())
}

/// ¿Este verbo de `ip` cambia algo?
///
/// Vale la palabra entera o un prefijo suyo de tres letras o más, que es hasta
/// donde se abrevia sin ambigüedad: `ip link set` y `ip link se` cambian la
/// red, y `ip l s` es casi siempre `show`.
fn ip_escribe(verbo: &str) -> bool {
    verbo.len() >= 3 && IP_ESCRIBE.iter().any(|v| v.starts_with(verbo))
}

/// ¿Es `ip netns exec`, la puerta a un espacio de red aparte?
///
/// El objeto cuenta desde tres letras, como el resto de `ip`; el verbo desde
/// una, porque dentro de `netns` no hay otro subcomando que empiece por `e` y
/// aquí equivocarse por exceso solo cuesta mirar el comando de dentro.
fn es_netns_exec(objeto: &str, verbo: &str) -> bool {
    objeto.len() >= 3
        && "netns".starts_with(objeto)
        && !verbo.is_empty()
        && "exec".starts_with(verbo)
}

/// Los destinos donde escribir no deja rastro. `>|` es la misma redirección
/// con el clobber forzado, así que el destino se lee igual.
fn destino_que_no_guarda(resto: &str) -> bool {
    let destino = resto
        .trim_start_matches('|')
        .split_whitespace()
        .next()
        .unwrap_or("");
    matches!(
        destino,
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(c: &str) -> Risk {
        classify(c).risk
    }

    /// Instalar no lo decide el agente, así que hay que reconocerlo bien: ni
    /// dejar pasar lo que instala, ni frenar lo que solo mira.
    #[test]
    fn distingue_instalar_de_consultar() {
        for c in [
            "sudo apt install ripgrep",
            "apt-get remove nginx",
            "pip install requests",
            "npm install",
            "cargo install ripgrep",
            "ls -la && pip3 install flask",
            "pacman -S neovim",
        ] {
            assert!(instala_software(c).is_some(), "no vio la instalación: {c}");
        }
        for c in [
            "apt list --upgradable",
            "pip show requests",
            "cargo build --release",
            "npm run test",
            "dpkg -l | grep vim",
            "ls -la",
            "git status",
        ] {
            assert!(instala_software(c).is_none(), "aviso que sobra: {c}");
        }
    }

    #[test]
    fn lo_que_solo_lee_es_seguro() {
        assert_eq!(r("ls -la"), Risk::Safe);
        assert_eq!(r("git status"), Risk::Safe);
        assert_eq!(r("grep -rn foo src/"), Risk::Safe);
        assert_eq!(r("ps aux | grep node"), Risk::Safe);
        assert_eq!(r("cat /var/log/syslog | tail -50"), Risk::Safe);
    }

    /// El caso que motivó rehacer este módulo.
    ///
    /// Escanear la red con `sudo nmap` es trabajo normal de administración.
    /// Marcarlo «peligroso» hacía que la interfaz se negara a escribirlo, y el
    /// usuario acababa pidiéndolo tres veces seguidas hasta copiarlo a mano.
    #[test]
    fn administrar_no_es_destruir() {
        assert_eq!(r("sudo nmap -sV -O -Pn 192.168.1.130"), Risk::Caution);
        assert_eq!(r("sudo systemctl status nginx"), Risk::Caution);
        assert_eq!(r("sudo apt update"), Risk::Caution);
        assert_eq!(r("sudo -u www-data ls /var/www"), Risk::Caution);
        assert_eq!(r("sudo tcpdump -i wlan0"), Risk::Caution);
    }

    #[test]
    fn sudo_no_disfraza_lo_que_si_destruye() {
        // Pero delante de algo irreversible sigue siendo irreversible.
        assert_eq!(r("sudo rm -rf /"), Risk::Danger);
        assert_eq!(r("sudo mkfs.ext4 /dev/sda1"), Risk::Danger);
        assert_eq!(r("sudo dd if=x of=/dev/sda"), Risk::Danger);
    }

    /// Salir de POSIX no sube el nivel —ya era «modifica» por desconocido—,
    /// pero sí tiene que decir por qué: que lo de dentro no se ha mirado.
    #[test]
    fn las_puertas_a_windows_dicen_que_no_se_leen() {
        for c in [
            "powershell -c Get-Date",
            "powershell.exe -Command Get-Process",
            "pwsh -c ls",
            "cmd.exe /c dir",
            "cmd /c echo hola",
        ] {
            let v = classify(c);
            assert_eq!(v.risk, Risk::Caution, "{c}");
            assert!(
                v.reason
                    .as_deref()
                    .unwrap_or("")
                    .contains("no se puede leer"),
                "{c}: {:?}",
                v.reason
            );
        }
        // Sin nada detrás no ejecutan nada de nadie: abren una sesión.
        assert_eq!(classify("powershell").reason, None);
    }

    /// `reg query` lee. `reg delete` es el `rm -rf` del registro.
    #[test]
    fn en_el_registro_manda_el_verbo() {
        assert_eq!(r("reg query HKLM\\SOFTWARE\\GitForWindows"), Risk::Caution);
        assert_eq!(classify("reg query HKLM\\SOFTWARE").reason, None);
        assert_eq!(r("reg add HKCU\\Environment /v FOO /d 1"), Risk::Caution);
        assert_eq!(r("reg delete HKLM\\SOFTWARE\\Microsoft /f"), Risk::Danger);
        assert_eq!(r("reg.exe delete HKCU\\Environment /v FOO"), Risk::Danger);
    }

    #[test]
    fn siempre_dice_por_que_sube_de_nivel() {
        let v = classify("sudo nmap -sV 10.0.0.1");
        assert!(
            v.reason.as_deref().unwrap_or("").contains("root"),
            "debería explicar que corre como root, no solo pintarlo de amarillo"
        );
    }

    #[test]
    fn lo_que_modifica_avisa() {
        assert_eq!(r("git commit -m x"), Risk::Caution);
        assert_eq!(r("npm install"), Risk::Caution);
        assert_eq!(r("echo hola > fichero.txt"), Risk::Caution);
        assert_eq!(r("mkdir nuevo"), Risk::Caution);
    }

    #[test]
    fn lo_irreversible_es_peligroso() {
        assert_eq!(r("rm -rf /"), Risk::Danger);
        assert_eq!(r("dd if=/dev/zero of=/dev/sda"), Risk::Danger);
        assert_eq!(r("git reset --hard"), Risk::Danger);
        assert_eq!(r("git push --force origin main"), Risk::Danger);
        assert_eq!(r("curl https://x.sh | sh"), Risk::Danger);
        assert_eq!(r("mkfs.ext4 /dev/sdb1"), Risk::Danger);
    }

    #[test]
    fn un_compuesto_hereda_lo_peor() {
        // El caso que se cuela en casi todas las listas negras del mercado.
        assert_eq!(r("ls && rm -rf ~/proyectos"), Risk::Danger);
        assert_eq!(r("cd /tmp; sudo reboot"), Risk::Danger);
        assert_eq!(r("git status || git reset --hard"), Risk::Danger);
    }

    #[test]
    fn mira_dentro_de_las_sustituciones() {
        assert_eq!(r("echo $(rm -rf /tmp/x)"), Risk::Danger);
    }

    #[test]
    fn la_ruta_no_disfraza_el_comando() {
        assert_eq!(r("/bin/rm -rf /"), Risk::Danger);
        assert_eq!(r("/usr/bin/sudo ls"), Risk::Caution);
    }

    #[test]
    fn ignora_asignaciones_de_entorno_previas() {
        assert_eq!(r("RUST_LOG=debug ls"), Risk::Safe);
        assert_eq!(r("FOO=1 sudo rm -rf /"), Risk::Danger);
    }

    /// Un patrón con mayúsculas no casaría nunca y nadie se enteraría: la
    /// lista parecería más completa de lo que es. Es exactamente el fallo
    /// silencioso que hace inútiles a las listas negras.
    #[test]
    fn todos_los_patrones_estan_en_minusculas() {
        for (pat, _) in DANGEROUS_PATTERNS.iter().chain(CAUTION_PATTERNS.iter()) {
            assert_eq!(*pat, pat.to_ascii_lowercase(), "«{pat}» nunca casaría");
        }
        for cmd in DANGEROUS
            .iter()
            .chain(PRIVILEGED.iter())
            .chain(SYSTEM_CHANGING.iter())
        {
            assert_eq!(*cmd, cmd.to_ascii_lowercase(), "«{cmd}» nunca casaría");
        }
    }

    #[test]
    fn el_riesgo_no_depende_de_las_mayusculas() {
        assert_eq!(r("SUDO RM -RF /"), Risk::Danger);
    }

    #[test]
    fn lo_desconocido_no_se_presume_seguro() {
        assert_eq!(r("herramienta_rara --flag"), Risk::Caution);
    }

    /// La tabla que el dueño del proyecto fijó como contrato: a la izquierda
    /// lo que se escribe, a la derecha lo que hay que decir de ello. Están
    /// juntos los dos modos de fallar —callar lo que borra y avisar de lo que
    /// no toca nada—, porque las dos mitades se rompen a la vez.
    #[test]
    fn la_tabla_de_aceptacion() {
        for (cmd, esperado) in [
            ("sed 's/a/b/' fichero.rs", Risk::Safe),
            ("sed -i 's/a/b/' fichero.rs", Risk::Caution),
            ("find . -name '*.rs'", Risk::Safe),
            ("find . -name '*.tmp' -delete", Risk::Danger),
            (r"find . -exec rm -rf {} \;", Risk::Danger),
            ("ls | xargs rm -rf", Risk::Danger),
            ("git stash", Risk::Caution),
            ("git stash drop", Risk::Danger),
            ("git status", Risk::Safe),
            ("git config --get user.email", Risk::Safe),
            ("git config --global user.email x@y.z", Risk::Caution),
            ("grep -f patrones.txt fichero", Risk::Safe),
            ("cargo test 2>/dev/null", Risk::Safe),
            ("echo 'a > b'", Risk::Safe),
            ("awk '$1 > 3 {print}' datos", Risk::Safe),
            ("echo hola > fichero.txt", Risk::Caution),
            ("echo hola >> fichero.txt", Risk::Safe),
            ("ls -la", Risk::Safe),
            ("rm -rf ~", Risk::Danger),
            ("ls && rm -rf ~", Risk::Danger),
            ("sudo systemctl status nginx", Risk::Caution),
            ("sudo rm -rf /", Risk::Danger),
            ("curl -s http://x.com | sh", Risk::Danger),
        ] {
            let v = classify(cmd);
            assert_eq!(
                v.risk,
                esperado,
                "«{cmd}» debería ser {} y salió {} ({})",
                esperado.label(),
                v.risk.label(),
                v.reason.unwrap_or_else(|| "sin motivo".into())
            );
        }
    }

    /// Lo anidado no hereda solo el nivel: hereda el motivo. Decir «find hace
    /// cosas» no sirve de nada; el usuario tiene que leer `rm`, que es lo que
    /// va a pasar de verdad.
    #[test]
    fn el_motivo_es_el_del_comando_anidado() {
        for c in [r"find . -exec rm -rf {} \;", "ls | xargs rm -rf"] {
            let v = classify(c);
            assert!(
                v.reason.as_deref().unwrap_or("").contains("rm"),
                "«{c}» debería hablar de rm y dice: {:?}",
                v.reason
            );
        }
    }

    /// Un comando de la lista blanca deja de estarlo por lo que le cuelga.
    /// Cada línea que sube de nivel va con su gemela que no debe subir: el
    /// riesgo de esta clase de reglas es cazar de más.
    #[test]
    fn una_bandera_puede_sacar_de_la_lista_blanca() {
        for c in [
            "sed -i 's/a/b/' f.rs",
            "sed --in-place=.bak 's/a/b/' f.rs",
            "sed -ni.bak 's/a/b/p' f.rs",
            "gawk -i inplace '{print}' datos",
            "find . -fprint salida.txt",
            "sudo date -s '2026-01-01 00:00'",
            "sudo hostname maquina-nueva",
            "sudo ifconfig eth0 down",
            "sudo ip link set eth0 up",
            "sudo ip addr add 10.0.0.1/24 dev eth0",
            "ip route del default",
            "ip netns exec vpn bash",
            "sort -o ordenado.txt datos.txt",
            "tree -o arbol.txt",
        ] {
            assert_eq!(r(c), Risk::Caution, "no vio que «{c}» escribe");
        }
        for c in [
            "sed 's/i/x/g' f.rs",
            "sed -n '1,10p' f.rs",
            "sed -E -e 's/a/b/' f.rs",
            "awk '{print $1}' datos",
            "awk -f script.awk datos",
            "find . -name '*.rs' -type f",
            "date -Is",
            "date +%s",
            "hostname -f",
            "ifconfig -a",
            "ifconfig eth0",
            "ip addr show dev eth0",
            "ip -br link",
            "ip route",
            "ip route get 8.8.8.8",
            "sort -n datos.txt",
            "tree -L 2 src",
        ] {
            assert_eq!(r(c), Risk::Safe, "aviso que sobra en «{c}»");
        }
    }

    /// `find -exec` y `xargs` son otra forma de encadenar comandos, y la regla
    /// es la misma que con `&&`: manda lo que hay dentro, en los dos sentidos.
    #[test]
    fn lo_anidado_se_clasifica_como_lo_que_es() {
        assert_eq!(r(r"find . -name '*.o' -exec rm {} \;"), Risk::Danger);
        assert_eq!(r("find . -type f -execdir shred {} +"), Risk::Danger);
        assert_eq!(r("ls | xargs -n 1 -I {} rm -rf {}"), Risk::Danger);
        assert_eq!(r("cat lista | xargs -0 chmod 600"), Risk::Caution);
        assert_eq!(r("fd -e tmp -x rm"), Risk::Danger);
        assert_eq!(r("env FOO=1 rm -rf /tmp/x"), Risk::Danger);
        assert_eq!(r("ip netns exec vpn rm -rf /tmp/x"), Risk::Danger);

        // Y lo de dentro inofensivo no contagia nada.
        assert_eq!(r(r"find . -name '*.rs' -exec cat {} \;"), Risk::Safe);
        assert_eq!(r("ls | xargs echo"), Risk::Safe);
        assert_eq!(r("env RUST_LOG=info ls -la"), Risk::Safe);
        assert_eq!(r("ip netns exec vpn ip -br addr show"), Risk::Safe);
    }

    /// El `\;` que cierra un `-exec` llega partido en dos —`split_parts` se
    /// queda con el `;`—, así que la barra suelta también cierra.
    #[test]
    fn el_cierre_del_exec_no_entra_en_el_comando() {
        assert_eq!(hasta_el_cierre(&["rm", "-rf", "{}", "\\"]), ["rm", "-rf"]);
        assert_eq!(hasta_el_cierre(&["rm", "{}", "+"]), ["rm"]);
        assert_eq!(hasta_el_cierre(&["rm", "{}", ";"]), ["rm"]);
        assert!(hasta_el_cierre(&[r"\;", "rm"]).is_empty());
    }

    /// Un `>` solo redirige si está donde el shell lo mira: fuera de comillas
    /// y sin nada pegado que lo convierta en otra cosa.
    #[test]
    fn distingue_la_redireccion_del_simbolo_mayor_que() {
        for c in [
            "echo hola > fichero.txt",
            "cat a >fichero",
            "echo x >| forzado.txt",
            "cargo test 2>errores.log",
            "diff a b > parche.diff",
        ] {
            assert!(trunca_un_fichero(c), "no vio la redirección en «{c}»");
        }
        for c in [
            "echo 'a > b'",
            "echo \"a > b\"",
            "awk '$1 > 3 {print}' datos",
            "echo hola >> fichero.txt",
            "cargo test 2>&1",
            "cargo test 2>/dev/null",
            "ls > /dev/null",
            "jq '.a >= 3' datos.json",
            "echo 'x -> y'",
            "grep '=>' src/",
            "echo a \\> b",
        ] {
            assert!(!trunca_un_fichero(c), "redirección que no existe en «{c}»");
        }
    }

    /// Saber qué queda fuera de comillas es lo que sostiene lo anterior, así
    /// que se prueba aparte y en pequeño.
    #[test]
    fn las_comillas_tapan_lo_que_hay_dentro() {
        assert_eq!(indices_sin_comillas("a'b'c"), [0, 4]);
        assert_eq!(indices_sin_comillas("a\"b\"c"), [0, 4]);
        // Dentro de comillas simples no hay escapes: la comilla doble es una
        // más, y la barra invertida también.
        assert_eq!(indices_sin_comillas("'\"\\'x"), [4]);
        // Fuera, la barra se lleva por delante el carácter siguiente.
        assert_eq!(indices_sin_comillas("\\'a"), [2]);
    }

    /// Un separador entre comillas es texto, no estructura.
    ///
    /// Es el fallo del `2>&1` con otra cara: cortar por un símbolo sin mirar
    /// si el shell lo vería como tal. Al partir, la cabecera se quedaba en el
    /// primer trozo y el resto pasaba por comando desconocido.
    #[test]
    fn un_separador_entre_comillas_no_parte_el_comando() {
        assert_eq!(split_parts("echo 'hola; adios'"), ["echo 'hola; adios'"]);
        assert_eq!(split_parts("grep 'a|b' f.txt"), ["grep 'a|b' f.txt"]);
        assert_eq!(split_parts("echo 'a (b) c'"), ["echo 'a (b) c'"]);
        assert_eq!(r("echo 'hola; adios'"), Risk::Safe);
        assert_eq!(r("grep 'a|b' fichero.txt"), Risk::Safe);

        // Y lo que sí es estructura se sigue partiendo.
        assert_eq!(split_parts("ls; rm -rf x"), ["ls", "rm -rf x"]);
        assert_eq!(r("ls; rm -rf ~"), Risk::Danger);
        assert_eq!(r("ls && rm -rf ~"), Risk::Danger);
        assert_eq!(r("echo $(rm -rf ~)"), Risk::Danger);
        assert_eq!(r("(cd /tmp; rm -rf x)"), Risk::Danger);
        assert_eq!(r("ls -la 2>&1"), Risk::Safe);
    }

    /// `awk` es un lenguaje: lo que hace está dentro de las comillas, que es
    /// justo donde no se mira. Cada caso que escribe va con su gemelo donde
    /// el mismo símbolo solo compara.
    #[test]
    fn awk_se_lee_por_dentro() {
        assert_eq!(r(r#"awk '{print > "salida.txt"}' datos"#), Risk::Caution);
        assert_eq!(r(r#"awk '{print $1 >> "log.txt"}' datos"#), Risk::Caution);
        assert_eq!(
            r(r#"awk '$1 > 3 {print > "altos.txt"}' datos"#),
            Risk::Caution
        );
        assert_eq!(r("gawk -i inplace '{print}' datos"), Risk::Caution);

        // `system` es ejecutar: manda el comando de dentro, para bien y para
        // mal.
        assert_eq!(r(r#"awk 'BEGIN{system("rm -rf ~")}'"#), Risk::Danger);
        assert_eq!(r(r#"awk 'BEGIN{system("ls -la")}'"#), Risk::Safe);

        // El `>` que solo compara, en sus cuatro formas.
        assert_eq!(r("awk '$1 > 3 {print}' datos"), Risk::Safe);
        assert_eq!(r("awk '{if (a > b) print}' datos"), Risk::Safe);
        assert_eq!(r("awk '{print; x = 1}' datos"), Risk::Safe);
        assert_eq!(r("awk -F: '{print $1}' datos"), Risk::Safe);
        assert_eq!(r("awk -f script.awk datos"), Risk::Safe);
    }

    /// `git stash` y `git config` estaban dados por lectura y no lo son. El
    /// arreglo tiene que distinguir, no barrer: `git config --get` pregunta y
    /// `git stash list` enseña.
    #[test]
    fn git_no_es_de_lectura_solo_por_el_subcomando() {
        assert_eq!(r("git stash"), Risk::Caution);
        assert_eq!(r("git stash push -m x"), Risk::Caution);
        assert_eq!(r("git stash drop"), Risk::Danger);
        assert_eq!(r("git stash clear"), Risk::Danger);
        assert_eq!(r("git stash list"), Risk::Safe);
        assert_eq!(r("git stash show -p"), Risk::Safe);

        assert_eq!(r("git config --global user.email x@y.z"), Risk::Caution);
        assert_eq!(r("git config user.name Juan"), Risk::Caution);
        assert_eq!(r("git config --get user.email"), Risk::Safe);
        assert_eq!(r("git config --get-regexp '^alias'"), Risk::Safe);
        assert_eq!(r("git config --list"), Risk::Safe);
    }

    /// Compilar y probar es el trabajo de todos los días; instalar no lo es, y
    /// además lo frena [`instala_software`] por su cuenta.
    #[test]
    fn cargo_de_diario_no_merece_aviso() {
        assert_eq!(r("cargo test --release"), Risk::Safe);
        assert_eq!(r("cargo clippy --all-targets"), Risk::Safe);
        assert_eq!(r("cargo build"), Risk::Safe);
        assert_eq!(r("cargo clippy --fix"), Risk::Caution);
        assert_eq!(r("cargo install ripgrep"), Risk::Caution);
        assert_eq!(r("cargo fmt"), Risk::Caution);
    }

    /// El aviso que se quitó: `-f` no significa «forzar» en casi ningún sitio.
    #[test]
    fn la_efe_suelta_ya_no_asusta() {
        assert_eq!(r("grep -f patrones.txt fichero"), Risk::Safe);
        assert_eq!(r("sudo -f /etc/sudoers.d/x ls"), Risk::Caution);
        // `--force` sí es inequívoco y sigue avisando.
        assert_eq!(r("cp --force a b"), Risk::Caution);
    }

    #[test]
    fn agrupa_las_banderas_cortas_sin_confundir_el_valor_pegado() {
        assert!(bandera_corta("-i", b'i'));
        assert!(bandera_corta("-ni", b'i'));
        assert!(bandera_corta("-i.bak", b'i'));
        assert!(!bandera_corta("--include", b'i'));
        assert!(!bandera_corta("-n", b'i'));
        assert!(!bandera_corta("fichero-i", b'i'));
        // La ese está en el valor pegado, no en el grupo de banderas.
        assert!(!bandera_corta("-i.sed", b's'));
    }

    /// La coletilla más tecleada de cualquier terminal no puede avisar.
    ///
    /// `2>&1` juntaba dos fallos: `split_parts` partía por el `&` y dejaba
    /// `1` suelto —un comando desconocido, o sea «modifica»— y el `>` que
    /// quedaba huérfano contaba como truncado. `ls -la 2>&1` salía marcado.
    /// Se prueba por `classify`, que es el camino que recorre el programa:
    /// probar el ayudante en aislamiento era lo que dejaba pasar esto.
    #[test]
    fn una_redireccion_de_descriptor_no_avisa() {
        for c in [
            "cargo test 2>&1",
            "ls -la 2>&1",
            "cargo build 2>&1 | tail -20",
            "cargo clippy --all-targets -- -D warnings 2>&1 | tail",
            "grep -r cosa . 2>/dev/null",
            "diff a b 1>&2",
        ] {
            assert_eq!(classify(c).risk, Risk::Safe, "«{c}» no toca nada");
        }
    }

    /// Pero un descriptor a un fichero de verdad sí trunca, y sí avisa.
    #[test]
    fn un_descriptor_a_un_fichero_si_avisa() {
        assert_eq!(classify("cargo test 2>errores.log").risk, Risk::Caution);
        assert_eq!(classify("echo hola > fichero.txt").risk, Risk::Caution);
    }

    /// Y no encadenar por `>&` no puede haber roto el encadenamiento normal:
    /// la regla de que un comando compuesto hereda su parte más peligrosa es
    /// lo único que este módulo garantiza de verdad.
    #[test]
    fn un_solo_ampersand_sigue_encadenando() {
        assert_eq!(classify("ls & rm -rf ~").risk, Risk::Danger);
        assert_eq!(classify("ls && rm -rf ~").risk, Risk::Danger);
        assert_eq!(classify("ls; rm -rf ~").risk, Risk::Danger);
    }
}
