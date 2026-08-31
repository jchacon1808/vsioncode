//! Prueba de extremo a extremo del circuito que da sentido al proyecto.
//!
//! Arranca un `bash` de verdad bajo un pseudoterminal, le escribe comandos
//! como si fuera el usuario, y comprueba que al otro lado el registro sabe
//! qué se ejecutó, qué salió y con qué código — sin que nadie haya copiado ni
//! pegado nada.
//!
//! Si esta prueba pasa, la tesis del proyecto se sostiene.

use std::time::{Duration, Instant};

use vsioncode::ledger::Ledger;
use vsioncode::marker::Marker;
use vsioncode::pty::{PtyMsg, Shell};

/// Arranca un shell, ejecuta los comandos dados y devuelve el registro
/// resultante. Espera hasta ver `esperados` comandos cerrados o agotar el
/// tiempo — nada de `sleep` a ciegas.
fn ejecutar(comandos: &[&str], esperados: usize) -> Ledger {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let shell = Shell::spawn(24, 80, tx).expect("no pude arrancar el shell");

    let mut ledger = Ledger::new();
    let limite = Instant::now() + Duration::from_secs(25);

    // Damos tiempo al shell a pintar su primer prompt antes de escribirle:
    // los marcadores de comando anteriores al primer prompt se descartan a
    // propósito, y sin esta espera la prueba sería una carrera.
    esperar_prompt(&mut rx, &mut ledger, limite);
    // Ver el prompt no significa que readline ya esté leyendo: hay una
    // ventana entre que bash lo pinta y que se pone a escuchar. Escribir
    // dentro de esa ventana pierde la línea, igual que si tecleas antes de
    // tiempo en una terminal de verdad.
    std::thread::sleep(Duration::from_millis(250));

    for c in comandos {
        // Los saltos de línea se mandan como retorno de carro, igual que hace
        // la aplicación: es lo que permite que un heredoc funcione.
        let cuerpo = c.lines().collect::<Vec<_>>().join("\r");
        shell.write_bytes(cuerpo.as_bytes()).unwrap();
        shell.write_bytes(b"\r").unwrap();
    }

    let mut cerrados = 0;
    let mut abierto = false;
    let mut traza: Vec<String> = Vec::new();
    let mut crudo = String::new();
    while Instant::now() < limite && cerrados < esperados {
        match rx.try_recv() {
            Ok(PtyMsg::Output(b)) => {
                crudo.push_str(&String::from_utf8_lossy(&b));
                ledger.feed_output(&b);
            }
            Ok(PtyMsg::Mark(Marker::CmdStart(cmd))) => {
                traza.push(format!("inicio<{cmd}>"));
                ledger.begin(cmd);
                abierto = true;
            }
            Ok(PtyMsg::Mark(Marker::CmdEnd { exit, micros })) => {
                traza.push(format!("fin<{exit}>"));
                if abierto {
                    ledger.end(Some(exit), Duration::from_micros(micros));
                    cerrados += 1;
                    abierto = false;
                }
            }
            Ok(PtyMsg::Mark(Marker::Cwd(p))) => ledger.set_cwd(p),
            Ok(PtyMsg::Mark(_)) => {}
            Ok(PtyMsg::Closed) => break,
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    assert!(
        cerrados >= esperados,
        "solo se cerraron {cerrados} de {esperados} comandos.\nTraza: {traza:?}\nPantalla: {:?}",
        &crudo[crudo.len().saturating_sub(600)..]
    );
    ledger
}

fn esperar_prompt(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<PtyMsg>,
    ledger: &mut Ledger,
    limite: Instant,
) {
    while Instant::now() < limite {
        match rx.try_recv() {
            Ok(PtyMsg::Mark(Marker::PromptEnd)) => return,
            Ok(PtyMsg::Mark(Marker::Cwd(p))) => ledger.set_cwd(p),
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    panic!("el shell no llegó a pintar un prompt");
}

#[test]
fn ve_un_comando_correcto_sin_que_nadie_pegue_nada() {
    let l = ejecutar(&["echo hola-desde-vsioncode"], 1);
    let ev = l.last().expect("el registro está vacío");

    assert_eq!(ev.cmd, "echo hola-desde-vsioncode");
    assert_eq!(ev.exit, Some(0));
    assert!(
        ev.output.contains("hola-desde-vsioncode"),
        "la salida no llegó al registro: {:?}",
        ev.output
    );
    assert!(!ev.failed());
}

#[test]
fn ve_el_fallo_y_su_codigo() {
    // El flujo estrella: algo falla y el agente ya tiene todo lo que necesita.
    let l = ejecutar(&["ls /ruta-que-no-existe-en-ningun-sitio"], 1);
    let ev = l.last_failure().expect("no registró ningún fallo");

    assert!(ev.cmd.starts_with("ls /ruta-que-no-existe"));
    assert_eq!(ev.exit, Some(2), "ls devuelve 2 cuando la ruta no existe");
    assert!(ev.failed());
    assert!(
        !ev.output.trim().is_empty(),
        "un fallo sin salida no le sirve de nada al agente"
    );
}

#[test]
fn no_mezcla_la_salida_de_dos_comandos() {
    // Este es el caso que se rompe si el orden entre bytes y marcadores no se
    // respeta dentro de una misma lectura del pseudoterminal.
    let l = ejecutar(&["echo primero", "echo segundo"], 2);
    let evs: Vec<_> = l.recent(2).collect();
    assert_eq!(evs.len(), 2);

    assert!(evs[0].output.contains("primero"));
    assert!(
        !evs[0].output.contains("segundo"),
        "se coló la salida del siguiente"
    );
    assert!(evs[1].output.contains("segundo"));
    assert!(
        !evs[1].output.contains("primero"),
        "arrastró la salida del anterior"
    );
}

#[test]
fn sigue_el_directorio_de_trabajo() {
    let l = ejecutar(&["cd /tmp", "pwd"], 2);
    assert_eq!(
        l.cwd(),
        "/tmp",
        "el registro perdió el cambio de directorio"
    );
}

#[test]
fn los_marcadores_no_se_ven_en_pantalla() {
    // Si la instrumentación se colara en el emulador, el usuario vería basura
    // como `]7331;cmd;ZWNobw==` en su terminal.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let shell = Shell::spawn(24, 80, tx).unwrap();

    let limite = Instant::now() + Duration::from_secs(15);
    let mut visto = String::new();
    shell.write_bytes(b"echo marcador-invisible\r").unwrap();

    while Instant::now() < limite {
        match rx.try_recv() {
            Ok(PtyMsg::Output(b)) => {
                visto.push_str(&String::from_utf8_lossy(&b));
                if visto.contains("marcador-invisible") && visto.matches('\n').count() > 2 {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    for rastro in ["7331", "133;A", "133;B", "cmd;", ";end;"] {
        assert!(
            !visto.contains(rastro),
            "«{rastro}» llegó a la pantalla; el escáner debería habérselo comido.\nVisto: {visto:?}"
        );
    }
}

#[test]
fn una_salida_enorme_no_se_come_la_memoria() {
    // 3 MB por el pseudoterminal. El registro tiene que quedarse acotado.
    let l = ejecutar(&["head -c 3000000 /dev/zero | tr '\\0' 'x'"], 1);
    let ev = l.last().unwrap();
    assert!(ev.truncated, "debería haber recortado");
    assert!(
        ev.output.len() < 200_000,
        "el registro creció hasta {} bytes",
        ev.output.len()
    );
    assert!(ev.output.contains("del centro"));
}

/// Regresión de un fallo que costó encontrar.
///
/// `EPOCHREALTIME` usa el separador decimal de la configuración regional. En
/// español es una coma, y en aritmética de bash la coma es el operador coma:
/// el cálculo de la duración no explotaba, devolvía la parte decimal — y
/// cuando esa parte empezaba por cero y tenía un 8 o un 9, bash la leía como
/// octal y la función moría **antes de emitir el marcador de fin**.
///
/// Resultado: la captura se rompía de forma intermitente según los dígitos
/// del reloj, en cualquier máquina con locale español, francés o alemán. Sin
/// error visible: el comando simplemente se quedaba abierto para siempre.
#[test]
fn la_coma_decimal_del_locale_no_rompe_la_captura() {
    for locale in ["es_ES.UTF-8", "de_DE.UTF-8", "C"] {
        unsafe {
            std::env::set_var("LC_ALL", locale);
            std::env::set_var("LC_NUMERIC", locale);
        }
        let l = ejecutar(&["echo locale-ok"], 1);
        let ev = l
            .last()
            .unwrap_or_else(|| panic!("con LC_ALL={locale} no se registró nada"));
        assert_eq!(
            ev.exit,
            Some(0),
            "con LC_ALL={locale} se perdió el código de salida"
        );
        assert!(
            ev.output.contains("locale-ok"),
            "con LC_ALL={locale} se perdió la salida"
        );
    }
    unsafe {
        std::env::remove_var("LC_ALL");
        std::env::remove_var("LC_NUMERIC");
    }
}

/// La duración medida tiene que ser plausible. Con el bug del separador
/// decimal salían números absurdos cuando no fallaba del todo.
#[test]
fn la_duracion_medida_es_creible() {
    let l = ejecutar(&["sleep 0.3"], 1);
    let ev = l.last().unwrap();
    let s = ev.duration.as_secs_f32();
    assert!(
        (0.2..3.0).contains(&s),
        "un `sleep 0.3` no puede durar {s} s"
    );
}

/// Regresión de un fallo que colgaba la terminal entera.
///
/// Para escribir un fichero, un agente usa un heredoc. Al lanzarlo se
/// aplastaban los saltos de línea a espacios, así que bash se quedaba
/// esperando eternamente un `EOF` que nunca llegaba solo — en el prompt de
/// continuación, tragándose todos los comandos siguientes.
///
/// Desde fuera no parecía roto: parecía que la terminal iba lenta, mientras la
/// conversación seguía como si nada.
#[test]
fn un_heredoc_escribe_el_fichero_en_vez_de_colgar_el_shell() {
    let dir = std::env::temp_dir().join(format!("vsion-heredoc-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let fichero = dir.join("notas.md");
    let _ = std::fs::remove_file(&fichero);

    // La ruta se le escribe **al shell**, así que va en la forma del shell.
    // Con la nativa, en Windows las barras invertidas de `C:\Users\...` se las
    // come bash como escapes y el fichero acaba en otro sitio, o en ninguno.
    let heredoc = format!(
        "cat > {} <<'FIN'\n# Título\ncon **dos** líneas\nFIN",
        vsioncode::rutas::a_posix(&fichero.display().to_string())
    );

    // Tal y como lo manda el agente: con los saltos de línea intactos.
    let l = ejecutar(&[&heredoc, "echo terminado"], 2);

    let escrito = std::fs::read_to_string(&fichero).unwrap_or_default();
    assert!(
        escrito.contains("# Título"),
        "no se escribió el fichero: {escrito:?}"
    );
    assert!(
        escrito.contains("con **dos** líneas"),
        "se perdió una línea: {escrito:?}"
    );

    // Y lo que de verdad importa: el shell siguió aceptando comandos después.
    let ultimo = l.last().unwrap();
    assert!(
        ultimo.output.contains("terminado"),
        "el shell se quedó atascado tras el heredoc: {:?}",
        ultimo.output
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Los ficheros de arranque de instancias muertas no pueden acumularse.
///
/// Cada shell borra el suyo al cerrar bien, pero un `kill -9` o un cuelgue lo
/// deja ahí para siempre. Se comprueba que al arrancar se limpian los ajenos
/// **sin tocar los de instancias que siguen vivas**, que sería peor: dejaría a
/// otra ventana sin su fichero.
#[test]
fn los_ficheros_de_arranque_huerfanos_se_limpian() {
    let dir = dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("vsioncode");
    let _ = std::fs::create_dir_all(&dir);

    // Uno de un proceso que no existe, y uno del diseño viejo.
    let huerfano = dir.join("init.4294967290.7.bash");
    let viejo = dir.join("init.bash");
    std::fs::write(&huerfano, "# de una instancia muerta").unwrap();
    std::fs::write(&viejo, "# del diseño anterior").unwrap();

    // Y uno de un proceso vivo: el de esta misma prueba.
    let mio = dir.join(format!("init.{}.999.bash", std::process::id()));
    std::fs::write(&mio, "# vivo").unwrap();

    // Arrancar un shell dispara la limpieza.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let shell = Shell::spawn(24, 80, tx).expect("arrancar");

    assert!(!huerfano.exists(), "no se limpió el de un proceso muerto");
    assert!(!viejo.exists(), "no se limpió el del diseño anterior");
    assert!(mio.exists(), "se cargó el de una instancia viva");

    drop(shell);
    let _ = std::fs::remove_file(&mio);
}
