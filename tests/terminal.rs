//! Que el terminal se devuelva tal y como se cogió.
//!
//! vsioncode enciende siete modos del emulador al arrancar —pantalla alterna,
//! cinco de ratón y el pegado con corchetes—. Ninguno se apaga solo: si el
//! programa se va sin apagarlos, el emulador sigue informando del ratón a un
//! programa que ya no existe y el shell escribe los informes en la línea, sin
//! parar, mientras muevas el puntero. Ya pasó, y no lo vio ningún test: los de
//! unidad no arrancan el binario, y los de circuito prueban la biblioteca.
//!
//! Por eso esta prueba arranca **el binario de verdad** bajo un pseudoterminal
//! y no mira lo que dibuja, sino lo que enciende y lo que apaga. La regla es
//! una sola y vale para los modos que hay hoy y para los que se añadan: cada
//! `CSI ? n h` tiene que tener su `CSI ? n l`.

use std::collections::BTreeSet;
use std::io::Read;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

/// Arranca el binario bajo un pseudoterminal, le manda una señal y devuelve
/// todo lo que escribió.
///
/// `--sin-agente` no es por ir rápido: arrancar el modelo gastaría cuota del
/// usuario cada vez que alguien corre las pruebas.
fn arrancar_y_señalar(senal: &str) -> Vec<u8> {
    let pty = NativePtySystem::default()
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("no pude abrir el pseudoterminal");

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_vsioncode"));
    cmd.arg("--sin-agente");
    // Sin un TERM que entienda de colores, crossterm puede no emitir lo mismo.
    cmd.env("TERM", "xterm-256color");
    let child = pty
        .slave
        .spawn_command(cmd)
        .expect("no pude arrancar vsioncode");
    let pid = child.process_id().expect("el hijo no tiene pid");

    // Se lee en un hilo porque el maestro bloquea: aquí hace falta poder
    // esperar con reloj, no quedarse colgado si el programa no muere.
    let mut lector = pty
        .master
        .try_clone_reader()
        .expect("no pude clonar el lector");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = lector.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    // Se le da tiempo a montar el terminal antes de tirarlo: mandar la señal
    // antes de que encienda los modos probaría justo lo contrario de lo que
    // se quiere probar.
    let mut salida = Vec::new();
    let hasta = Instant::now() + Duration::from_secs(3);
    while Instant::now() < hasta {
        if let Ok(t) = rx.recv_timeout(Duration::from_millis(100)) {
            salida.extend_from_slice(&t);
        }
    }

    // `kill` en vez de una llamada al sistema para no arrastrar `libc` a las
    // dependencias por una prueba.
    let _ = std::process::Command::new("kill")
        .arg(senal)
        .arg(pid.to_string())
        .status();

    // Y ahora lo que escriba al morir, que es lo que se está midiendo.
    let hasta = Instant::now() + Duration::from_secs(5);
    while Instant::now() < hasta {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(t) => salida.extend_from_slice(&t),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    salida
}

/// Los modos DEC privados que se encienden y los que se apagan.
///
/// Son las secuencias `CSI ? n h` y `CSI ? n l`. Se buscan a mano y no con una
/// expresión regular porque el módulo de riesgo ya presume de no tener
/// dependencias y esto no vale una.
fn modos(salida: &[u8]) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let (mut encendidos, mut apagados) = (BTreeSet::new(), BTreeSet::new());
    let mut i = 0;
    while i + 4 < salida.len() {
        if &salida[i..i + 3] != b"\x1b[?" {
            i += 1;
            continue;
        }
        let mut j = i + 3;
        let mut n: u32 = 0;
        let mut hay = false;
        while j < salida.len() && salida[j].is_ascii_digit() {
            n = n * 10 + u32::from(salida[j] - b'0');
            hay = true;
            j += 1;
        }
        if hay && j < salida.len() {
            match salida[j] {
                b'h' => {
                    encendidos.insert(n);
                }
                b'l' => {
                    apagados.insert(n);
                }
                _ => {}
            }
        }
        i = j.max(i + 1);
    }
    (encendidos, apagados)
}

/// Cerrar la ventana del emulador manda SIGHUP, y es como se cierra esto la
/// mitad de las veces.
#[test]
fn una_senal_no_deja_el_terminal_a_medias() {
    for senal in ["-TERM", "-HUP"] {
        let salida = arrancar_y_señalar(senal);
        let (encendidos, apagados) = modos(&salida);

        assert!(
            encendidos.contains(&1049),
            "con {senal} no llegó ni a montar el terminal: nada que probar"
        );

        let huerfanos: Vec<_> = encendidos.difference(&apagados).collect();
        assert!(
            huerfanos.is_empty(),
            "con {senal} quedan modos encendidos: {huerfanos:?}"
        );
    }
}

/// El ratón es el que se nota, porque el emulador sigue informando a nadie.
///
/// Va aparte de la comprobación general para que el fallo diga de qué va: 1003
/// informa de todo movimiento y 1006 lo hace en formato SGR, que juntos son
/// los que llenaban la línea del shell de `35;45;18M`.
#[test]
fn el_raton_se_apaga_al_salir() {
    let salida = arrancar_y_señalar("-TERM");
    let (encendidos, apagados) = modos(&salida);
    for modo in [1000, 1002, 1003, 1006, 1015] {
        if encendidos.contains(&modo) {
            assert!(
                apagados.contains(&modo),
                "el modo de ratón {modo} se quedó encendido"
            );
        }
    }
}
