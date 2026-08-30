//! Sonda de diagnóstico: arranca el shell y vuelca lo que llega, crudo.
use std::time::{Duration, Instant};
use vsioncode::marker::Marker;
use vsioncode::pty::{PtyMsg, Shell};

fn main() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let shell = Shell::spawn(24, 80, tx).expect("spawn");
    let fin = Instant::now() + Duration::from_secs(12);
    let mut escrito = false;
    let mut bytes = 0usize;
    while Instant::now() < fin {
        match rx.try_recv() {
            Ok(PtyMsg::Output(b)) => {
                bytes += b.len();
            }
            Ok(PtyMsg::Mark(m)) => {
                println!(
                    "MARCA {:?}",
                    match &m {
                        Marker::CmdStart(c) => format!("CmdStart({c})"),
                        Marker::CmdEnd { exit, .. } => format!("CmdEnd({exit})"),
                        Marker::Cwd(p) => format!("Cwd({p})"),
                        o => format!("{o:?}"),
                    }
                );
                if matches!(m, Marker::PromptEnd) && !escrito {
                    escrito = true;
                    println!("--- escribo: echo prueba ---");
                    shell.write_bytes(b"echo prueba\r").unwrap();
                }
            }
            Ok(PtyMsg::Closed) => {
                println!("CERRADO");
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    println!("total bytes de salida: {bytes} · ¿llegó a escribir?: {escrito}");
}
