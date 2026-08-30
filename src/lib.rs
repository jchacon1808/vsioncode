//! vsioncode — una terminal donde el humano y la IA miran la misma pantalla.
//!
//! El problema que resuelve cabe en una frase: **el copiar y pegar**. Hoy,
//! trabajar con un agente en la terminal significa ejecutar algo, ver que
//! falla, seleccionar la salida, pegarla en un chat, leer la respuesta,
//! copiar el comando propuesto y pegarlo de vuelta. Seis saltos de contexto
//! por iteración, y un agente que solo ve lo que le das a mano.
//!
//! Aquí el shell y el agente comparten un mismo registro de lo ejecutado. Si
//! algo falla, el agente ya lo ha visto: una tecla y te lo explica. Si no
//! sabes el comando, te lo escribe en tu línea — y ahí se queda, porque
//! pulsar Enter sigue siendo tuyo.
//!
//! # Las capas
//!
//! - [`pty`]: el shell de verdad, bajo un pseudoterminal. `vim` y `htop`
//!   funcionan porque hay un TTY completo, no una herramienta «ejecuta esto».
//! - [`marker`]: saca del flujo de bytes la instrumentación que emite el
//!   shell, sin que el usuario la vea nunca.
//! - [`ledger`]: el registro compartido. La pieza que mata el copiar y pegar.
//! - [`proveedores`]: de dónde salen los modelos.
//! - [`ai`]: el cerebro, un daemon que no tiene herramientas y por tanto no
//!   puede tocar nada.
//! - [`agente`]: subagentes de solo lectura, para repartir trabajo grande.
//! - [`edicion`]: cambiar ficheros de verdad, no con `sed` retorcidos.
//! - [`plan`]: pensar el trabajo entero y que lo apruebes antes de empezarlo.
//! - [`adjunto`]: soltar un fichero en la conversación y que lo vea.
//! - [`skills`]: instrucciones especializadas, leídas de donde el usuario ya
//!   las tenga instaladas.
//! - [`voz`]: hablarle y que conteste, sin que el audio salga del equipo.
//! - [`safety`]: cuánta ceremonia merece un comando antes de insertarlo.
//! - [`bienvenida`]: la puesta en marcha la primera vez que se abre.
//! - [`slash`]: los comandos `/` del panel del agente.
//! - [`keymap`]: el reparto de teclas. `Ctrl` es del shell, `Alt` es nuestro.
//! - [`raton`]: la rueda mueve la pantalla y no escribe el historial.
//! - [`marcado`]: el markdown que de verdad usa un agente, y nada más.
//! - [`tema`]: los colores, por lo que significan.
//! - [`app`] y [`ui`]: estado y vista.

pub mod adjunto;
pub mod agente;
pub mod ai;
pub mod app;
pub mod bienvenida;
pub mod edicion;
pub mod keymap;
pub mod ledger;
pub mod marcado;
pub mod marker;
pub mod plan;
pub mod proveedores;
pub mod pty;
pub mod raton;
pub mod safety;
pub mod skills;
pub mod slash;
pub mod tema;
pub mod ui;
pub mod voz;
