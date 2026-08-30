//! Punto de entrada de vsioncode. La lógica vive en la biblioteca;
//! aquí solo se leen los argumentos, se levanta el terminal y se restaura.

use vsioncode::app::Vista;
use vsioncode::{app, ui};

use color_eyre::eyre::Result;
use tokio::sync::mpsc;

use vsioncode::ai::BrainConfig;
use vsioncode::app::App;
use vsioncode::pty::{self, Shell};

/// Lo que encendimos nosotros y solo nosotros podemos apagar.
///
/// Vive aparte porque hace falta en dos sitios que no se parecen en nada: la
/// salida normal y el gancho de pánico. `ratatui` devuelve la pantalla alterna
/// y el modo crudo, pero no sabe que aquí se capturó el ratón; y con
/// `panic = "abort"` no hay desenrollado, así que el final de `main` no llega
/// a ejecutarse nunca después de un fallo. Sin esto, el emulador sigue
/// informando del ratón a un programa que ya no está y el shell te escribe los
/// informes en la línea, sin parar, mientras muevas el puntero.
fn devolver_terminal() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste
    );
}

fn help() {
    println!(
        "\
vsioncode — terminal compartida entre tú y un agente

USO
    vsioncode [opciones]

OPCIONES
    --model <id>      modelo a usar. Por defecto se respeta el de tu
                      configuración de Claude Code (~/.claude/settings.json).
                      Para esta herramienta, «sonnet» o «haiku» suelen ir
                      mejor que «opus»: las respuestas son cortas y la
                      latencia se nota más que la profundidad.
    --configurar      abre la puesta en marcha: servicios, claves y modelo
    --sin-agente      arranca solo la terminal, sin cerebro
    --sin-web         el agente no podrá consultar internet
    --binario <ruta>  ejecutable del agente (por defecto: claude)
    -h, --help        esta ayuda
    -V, --version     versión

DENTRO
    Ctrl+cualquier cosa va al shell intacto. Los atajos van con Alt:
    Alt+A foco al agente · Alt+E explicar lo último · Alt+M cambiar de modo
    Alt+W pausar la captura · Alt+H ayuda · Alt+Q salir"
    );
}

#[tokio::main(flavor = "multi_thread", worker_threads = 3)]
async fn main() -> Result<()> {
    // color-eyre primero: así, si algo revienta, el terminal ya está
    // restaurado cuando se imprime el informe y no queda la consola muda.
    color_eyre::install()?;

    // El proveedor criptográfico se elige a mano, y a propósito.
    //
    // rustls trae `aws-lc-rs` por defecto: una biblioteca en C que, medida en
    // este proyecto, engordaba el binario de 1,8 a 5,9 MB. `ring` hace el
    // mismo trabajo por una fracción. El precio de elegirlo es esta línea: sin
    // ella, cualquier conexión TLS falla al no haber proveedor instalado.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut cfg = BrainConfig::default();
    let mut sin_agente = false;
    let mut configurar = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                help();
                return Ok(());
            }
            "-V" | "--version" => {
                println!("vsioncode {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--sin-agente" => sin_agente = true,
            "--configurar" => configurar = true,
            "--sin-web" => cfg.web = false,
            "--model" => cfg.model = args.next(),
            "--binario" => {
                if let Some(b) = args.next() {
                    cfg.binary = b;
                }
            }
            otro => {
                eprintln!("vsioncode: opción desconocida «{otro}». Prueba --help.");
                std::process::exit(2);
            }
        }
    }

    let (pty_tx, pty_rx) = mpsc::unbounded_channel();
    let (ai_tx, ai_rx) = mpsc::unbounded_channel();
    let (voz_tx, voz_rx) = mpsc::unbounded_channel();
    // Copia para poder relanzar el cerebro con otro modelo sin reiniciar.
    let ai_tx2 = ai_tx.clone();

    // El cerebro es opcional a propósito. Si `claude` no está, o no está
    // autenticado, vsioncode arranca igual: una terminal sin agente sigue
    // siendo una terminal, y es mucho mejor que un error y nada.
    // Lo que hay en el disco: proveedores y skills. Deja `cfg` apuntando al
    // proveedor por defecto, así que va antes de arrancar el cerebro — y
    // también antes de heredar el modelo de Claude Code, porque hasta aquí
    // `cfg.model` significa «lo ha pedido la línea de órdenes» y después ya no
    // se distingue de una preferencia leída de un fichero.
    let mut arranque = app::Arranque::preparar(&mut cfg);

    // Pedirla a mano vale tanto como no tener configuración: en los dos casos
    // lo que se quiere es la misma ventana.
    if configurar {
        arranque.primera_vez = true;
    }

    // Si no se pidió un modelo, se respeta el que el usuario tenga configurado
    // en Claude Code. Solo si se va a hablar con el daemon: a un proveedor
    // HTTP el modelo se lo dice su propia entrada del fichero.
    if cfg.model.is_none() && cfg.proveedor.is_none() {
        cfg.model = vsioncode::ai::preferred_model();
    }

    let brain = if sin_agente {
        None
    } else {
        app::try_brain(ai_tx, &cfg)
    };

    // `ratatui::init` instala el gancho de pánico que restaura el terminal.
    // Sin él, un fallo te deja la consola sin eco y sin cursor.
    let mut terminal = ratatui::init();

    // Y el suyo no basta: no sabe nada del ratón. Se envuelve en vez de
    // sustituirlo para que el nuestro corra primero y el informe de
    // color-eyre salga después, sobre un terminal ya devuelto.
    let anterior = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        devolver_terminal();
        anterior(info);
    }));

    // Capturar el ratón es lo que permite que la rueda mueva la pantalla en
    // vez de escribir el historial en la línea. El precio es que el emulador
    // deja de poder seleccionar texto: para eso está Mayús+arrastrar, y
    // `/raton` si eso no bastara.
    // Y el pegado con corchetes, sin el cual `Event::Paste` no llega NUNCA.
    //
    // Faltaba desde el principio y no se notó porque un pegado sin esto llega
    // igual, carácter a carácter, como si lo teclearas: en el shell funciona y
    // nadie lo echa de menos. Lo que no se puede es distinguir «he pegado
    // esto» de «he escrito esto», y de eso depende reconocer un fichero
    // soltado en la conversación. También mejora el pegado en el shell: los
    // programas de dentro reciben el pegado marcado como tal, que es lo que
    // evita que `vim` te lo autoindente.
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste
    );
    // Un terminal puede no saber su tamaño (sin terminal de control, en CI,
    // dentro de `script`). Ahí se usa el clásico 80x24 en vez de arrastrar un
    // cero hasta el emulador.
    // Desde aquí hasta `devolver_terminal()` no puede quedar ni un `?`: se
    // saldría dejando el emulador informando a nadie.
    let size = match terminal.size() {
        Ok(s) => s,
        Err(e) => {
            devolver_terminal();
            ratatui::restore();
            return Err(e.into());
        }
    };
    let (h, w) = pty::sane_size(size.height, size.width);
    let (rows, cols) = ui::shell_dims(w, h, Vista::Ambos);

    let resultado = match Shell::spawn(rows, cols, pty_tx) {
        Ok(shell) => {
            App::new(shell, brain, cfg, arranque, ai_tx2, voz_tx)
                .run(&mut terminal, pty_rx, ai_rx, voz_rx)
                .await
        }
        Err(e) => Err(e),
    };

    devolver_terminal();
    ratatui::restore();

    if let Err(e) = resultado {
        eprintln!("vsioncode: {e}");
        std::process::exit(1);
    }
    Ok(())
}
