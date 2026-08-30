//! Cómo queda la conversación, sin abrir un terminal.
//!
//! El diseño de esta parte se decide mirándola, y mirarla a través de un
//! pseudoterminal convierte cada retoque en una expedición. Esto compone una
//! conversación de muestra y la imprime tal cual se vería.
//!
//! ```text
//! cargo run --release --example vista
//! cargo run --release --example vista -- 60   # a otro ancho
//! ```

use vsioncode::app::{ChatMsg, Role};

fn main() {
    let ancho: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(78);

    let muestra = vec![
        ChatMsg {
            role: Role::System,
            text: "Listo. Ejecuta lo que quieras: veo cada comando y su salida sin que \
                   tengas que pegarme nada."
                .into(),
        },
        ChatMsg {
            role: Role::User,
            text: "el push ha fallado, ¿por qué?".into(),
        },
        ChatMsg {
            role: Role::Agent,
            text: "La rama remota tiene commits que tú no tienes, así que git se niega a \
                   sobrescribirlos.\n\nTráetelos primero y vuelve a intentarlo:\n\n\
                   ```bash\ngit pull --rebase origin master\n```"
                .into(),
        },
        // Un cambio ya escrito, con su diff. Es lo que más se mira de esta
        // pantalla en modo libre, donde nadie ha pulsado nada: si el bloque
        // no se lee de un vistazo, no se revisa.
        ChatMsg {
            role: Role::System,
            text: "Escrito src/app.rs: −3 +2 líneas.\n\
                   ```diff\n\
                   \u{20}1315         // Una lista de dominios no lo arregla: envejece mal.\n\
                   \u{20}1316         //\n\
                   -1317         // seguridad. Lo que sí lo corta es una pulsación tuya justo\n\
                   -1318         // aquí, que es el único eslabón que importa.\n\
                   -1319         if self.web_en_turno {\n\
                   +1317         // seguridad. Lo que sí lo corta es una pulsación tuya aquí.\n\
                   +1318         if self.web_en_turno && !(self.libre && v.risk == Risk::Safe) {\n\
                   \u{20}1320             self.proposal = Some(prop);\n\
                   \u{20}1321             self.push(\n\
                   ```"
                .into(),
        },
        ChatMsg {
            role: Role::User,
            text: "pásame el pdf".into(),
        },
        // El ciclo entero del modo ejecutar, que es lo que hay que poder mirar
        // de un vistazo: trabaja (atenuado), para y te espera (su verde), y
        // cierra contándote lo que salió (a plena luz).
        ChatMsg {
            role: Role::Agent,
            text: "Compruebo si pandoc está instalado.\n\n```bash\ncommand -v pandoc\n```"
                .into(),
        },
        ChatMsg {
            role: Role::Agent,
            text: "Instalo el motor de PDF que le falta a pandoc.\n\n                   ```bash\nsudo apt install -y texlive-latex-base\n```"
                .into(),
        },
        ChatMsg {
            role: Role::Espera,
            text: "Te toca a ti («apt» instala o quita software): Alt+↵ lo pone en tu \
                   línea y sigo con la salida."
                .into(),
        },
        ChatMsg {
            role: Role::Agent,
            text: "Hecho: diagnostico.pdf tiene las tres secciones y pesa 41 KB.".into(),
        },
        // Y un plan esperando aprobación, que es la otra pantalla que se lee
        // entera antes de pulsar: si los pasos no se distinguen de un vistazo,
        // aprobar deja de ser una decisión y pasa a ser un trámite.
        ChatMsg {
            role: Role::User,
            text: "/plan que el registro guarde también el usuario del sistema".into(),
        },
        ChatMsg {
            role: Role::Espera,
            text: "PLAN — guardar el usuario del sistema en cada comando del registro\n\
                   1. Añadir el campo `usuario` a `ledger::Event` y rellenarlo en                    `Ledger::start`\n\
                   2. Sacarlo una sola vez al arrancar: es el mismo en toda la sesión\n\
                   3. Pintarlo en `/registro` solo cuando no sea el de siempre\n\
                   4. Un test que compruebe que un `sudo` queda anotado con quien lo lanzó\n\
                   5. cargo test --release\n\n\
                   Alt+↵ lo pone en marcha; Ctrl+N lo descarta. Nada se ha tocado todavía."
                .into(),
        },
    ];

    println!("{}", "─".repeat(ancho));
    // Con color: es la mitad de lo que hay que decidir aquí.
    for l in vsioncode::ui::previa_color(&muestra, ancho) {
        println!("{l}");
    }
    println!("{}", "─".repeat(ancho));
}
