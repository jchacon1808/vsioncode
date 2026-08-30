//! El plan: pensar el trabajo entero antes de empezarlo.
//!
//! Lo que faltaba no era capacidad, era **granularidad de la decisión**. En
//! modo ejecutar el humano decide comando a comando y edición a edición; en
//! modo libre ni eso. Para un encargo de veinte pasos las dos cosas fallan por
//! el mismo sitio: nadie ha aprobado nunca *el trabajo*, solo sus trozos, y
//! cuando el tercer comando revela que el agente entendió otra cosa ya se han
//! tocado ficheros. Enterarse de lo que iba a hacer exigía verlo hacerlo.
//!
//! Un plan mueve esa decisión al único sitio donde es barata: antes de empezar.
//!
//! ## Dos fases, y la frontera entre ellas es lo importante
//!
//! - **Trazar.** El agente se entera —lee, pregunta al disco, reparte entre
//!   subagentes— y termina escribiendo un bloque ` ```plan: `. Aquí **no toca
//!   nada**: los comandos que lanza son para mirar y las ediciones que se le
//!   ocurran se quedan esperando tu pulsación aunque el modo libre esté
//!   encendido. Es la única guarda que añade este módulo al resto del programa,
//!   y va en el código y no en el prompt porque «planificar no toca» es
//!   justamente lo que no se puede dejar en manos de que se acuerde.
//! - **Ejecutar.** Solo después de tu `Alt+↵`. El plan aprobado vuelve como
//!   encargo, y entonces sí trabaja como en cualquier otra tanda del modo
//!   libre.
//!
//! ## Por qué un bloque de texto y no una llamada a herramienta
//!
//! Por lo mismo que los comandos van en ` ```bash ` y los encargos en
//! ` ```agente: `: la llamada a herramientas es donde los proveedores se
//! diferencian y donde los modelos pequeños fallan. Un bloque con una lista
//! numerada dentro lo escribe cualquiera, incluido un modelo local, y se puede
//! probar sin gastar una llamada.
//!
//! ## Por qué el plan es datos y no prosa
//!
//! Porque contar los pasos es lo único que sabe vsioncode y no sabe el modelo.
//! De ahí salen las dos cosas que este módulo hace posibles y que ningún
//! párrafo bien redactado haría: que la barra diga «plan 3/7» mientras la
//! conversación se va hacia arriba, y que al pararse se pueda decir **en qué
//! paso** se paró en vez de «hecho en 19 comandos», que fue el fallo que ya se
//! pagó una vez.

/// Un plan trazado: para qué es y en qué pasos se hace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// El objetivo, en una línea. Lo que iba en la valla del bloque.
    pub objetivo: String,
    /// Los pasos, en el orden en que se hacen.
    pub pasos: Vec<String>,
}

impl Plan {
    /// Los pasos numerados, para enseñarlos y para devolvérselos al modelo.
    ///
    /// La numeración la pone vsioncode y no se copia la del modelo: los
    /// números son los que va a usar el marcador de avance, y dos numeraciones
    /// distintas para lo mismo —la suya al escribirlo, la nuestra al contarlo—
    /// se separan en cuanto se salta un número o empieza por cero.
    pub fn lista(&self) -> String {
        let mut t = String::new();
        for (i, p) in self.pasos.iter().enumerate() {
            t.push_str(&format!("{}. {p}\n", i + 1));
        }
        t
    }
}

/// En qué punto está la planificación.
///
/// Un enum y no tres banderas sueltas porque los tres estados se excluyen: no
/// se puede estar trazando un plan y ejecutándolo a la vez, y con banderas esa
/// verdad habría que sostenerla a mano en cada sitio que las toque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fase {
    /// Se le ha pedido el plan y todavía no ha llegado. Puede tardar varios
    /// turnos: enterarse es parte de trazarlo.
    Trazando,
    /// Trazado y esperando tu decisión. Nada se ha ejecutado.
    Propuesto(Plan),
    /// Aprobado y en marcha.
    Marcha {
        plan: Plan,
        /// El último paso que el agente dijo estar empezando; 0 si no ha dicho
        /// ninguno todavía. **No se cuenta aquí**: contar comandos y llamarlos
        /// pasos sería inventarse un progreso que nadie ha declarado.
        paso: usize,
    },
}

/// La palabra con la que el agente dice por qué paso va.
///
/// Un token exacto al principio de línea, como [`crate::ai::PETICION_LIBRE`] y
/// por el mismo motivo: reconocer la intención por parecido de texto falla en
/// los dos sentidos, y aquí un falso positivo movería la barra a un paso que
/// nadie ha empezado.
pub const AVANCE: &str = "PASO";

/// Saca del bloque de texto el plan que traiga, si trae alguno.
///
/// **El último y no el primero**, igual que con los comandos y las ediciones:
/// un turno puede traer un plan y luego su corrección —típico si el modelo
/// mira algo a mitad—, y quedarse con el primero es aprobar la versión que él
/// mismo descartó.
///
/// Un bloque con título y sin pasos devuelve un [`Plan`] con `pasos` vacío en
/// vez de `None`. Es a propósito: quien llama tiene que poder distinguir «no
/// ha trazado nada» de «ha escrito un plan vacío», que se contestan de forma
/// distinta.
pub fn extraer(md: &str) -> Option<Plan> {
    let mut planes: Vec<Plan> = Vec::new();
    let mut dentro: Option<String> = None;
    let mut cuerpo = String::new();

    for linea in md.lines() {
        let t = linea.trim_start();
        if t.starts_with("```") {
            if let Some(objetivo) = dentro.take() {
                planes.push(Plan {
                    objetivo,
                    pasos: pasos(&cuerpo),
                });
                cuerpo.clear();
                continue;
            }
            if let Some(objetivo) = cabecera(t.trim_start_matches('`').trim()) {
                dentro = Some(objetivo);
                cuerpo.clear();
            }
            continue;
        }
        if dentro.is_some() {
            cuerpo.push_str(linea);
            cuerpo.push('\n');
        }
    }

    // Un bloque sin cerrar es un plan igual: el turno pudo cortarse por cuota
    // o por un Esc, y tirar lo escrito obligaría a pagar otra vez por lo mismo.
    if let Some(objetivo) = dentro {
        planes.push(Plan {
            objetivo,
            pasos: pasos(&cuerpo),
        });
    }

    planes.pop()
}

/// Lee la valla de apertura: `plan`, `plan: objetivo`.
///
/// Lo que va detrás de `plan` tiene que empezar por `:` o por un espacio, por
/// lo mismo que en [`crate::agente`]: sin esa condición, `planificación:` —o
/// cualquier palabra que empiece igual— abriría un plan con un objetivo
/// absurdo.
fn cabecera(etiqueta: &str) -> Option<String> {
    let resto = etiqueta.strip_prefix("plan")?;
    match resto.strip_prefix(':') {
        Some(r) => Some(r.trim().to_string()),
        None if resto.is_empty() || resto.starts_with(char::is_whitespace) => {
            Some(resto.trim().to_string())
        }
        None => None,
    }
}

/// Trocea el cuerpo del bloque en pasos.
///
/// Tolerante con la forma de la lista —`1.`, `1)`, `-`, `*`— porque la forma
/// no es la decisión: la decisión es qué se hace en cada paso, y rechazar un
/// plan bien pensado por escribirlo con guiones sería cobrar por la sintaxis.
///
/// Una línea que no abre paso **continúa el anterior**. Un paso puede ocupar
/// dos renglones sin convertirse en dos pasos, que es lo que pasaba antes de
/// esto y descuadraba la cuenta del avance.
fn pasos(cuerpo: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for linea in cuerpo.lines() {
        let t = linea.trim();
        if t.is_empty() {
            continue;
        }
        match sin_vinieta(t) {
            Some(paso) if !paso.is_empty() => v.push(paso),
            // Sin viñeta: o continúa el paso de arriba, o es el primero y
            // entonces vale igual —una lista sin numerar sigue siendo una
            // lista—.
            _ => match v.last_mut() {
                Some(ultimo) => {
                    ultimo.push(' ');
                    ultimo.push_str(t);
                }
                None => v.push(t.to_string()),
            },
        }
    }
    v
}

/// Quita la viñeta de una línea, si la trae. `None` si no era una.
fn sin_vinieta(t: &str) -> Option<String> {
    if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        return Some(r.trim().to_string());
    }
    let digitos: String = t.chars().take_while(char::is_ascii_digit).collect();
    if digitos.is_empty() {
        return None;
    }
    let resto = &t[digitos.len()..];
    let resto = resto
        .strip_prefix(". ")
        .or_else(|| resto.strip_prefix(") "))
        .or_else(|| resto.strip_prefix("."))
        .or_else(|| resto.strip_prefix(")"))?;
    Some(resto.trim().to_string())
}

/// ¿Ha dicho el agente que empieza un paso? Devuelve cuál.
///
/// El último del texto, no el primero: en un turno largo puede cerrar un paso
/// y abrir el siguiente, y lo que interesa saber es dónde está ahora.
pub fn avance(texto: &str) -> Option<usize> {
    texto
        .lines()
        .filter_map(|l| numero_de_paso(l.trim()))
        .next_back()
}

/// `PASO 3`, `PASO 3:`, `PASO 3/7` — el número, si la línea es solo eso.
///
/// Que la línea sea **solo** el marcador es la condición que evita mover la
/// barra cuando el agente escribe «en el PASO 3 me equivoqué» en mitad de una
/// frase.
fn numero_de_paso(linea: &str) -> Option<usize> {
    let resto = linea.strip_prefix(AVANCE)?.trim_start();
    let digitos: String = resto.chars().take_while(char::is_ascii_digit).collect();
    if digitos.is_empty() {
        return None;
    }
    // Lo que quede detrás del número tiene que ser adorno, no otra frase.
    let cola = resto[digitos.len()..].trim();
    let adorno = cola.is_empty()
        || cola == ":"
        || cola == "."
        || (cola.starts_with('/') && cola[1..].chars().all(|c| c.is_ascii_digit()));
    adorno.then(|| digitos.parse().ok()).flatten()
}

/// Quita las líneas de avance: son para vsioncode, no para leerse.
pub fn sin_avance(texto: &str) -> String {
    texto
        .lines()
        .filter(|l| numero_de_paso(l.trim()).is_none())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Lo que se le pide al modelo para que trace el plan.
///
/// Vive aquí y no incrustado en `app` por lo mismo que
/// [`crate::ai::modo_de_trabajo`]: lo que decide el comportamiento del agente
/// tiene que poder probarse con un modelo de verdad sin abrir la TUI.
///
/// Las tres cosas que este texto tiene que conseguir, y que costaron una
/// prueba cada una:
///
/// 1. **Que se entere antes de escribir.** Un plan salido de los nombres de
///    los ficheros es una lista de deseos.
/// 2. **Que no toque nada.** El código lo sujeta para las ediciones, pero los
///    comandos los lanza él y ahí solo está esta frase.
/// 3. **Que termine con el bloque.** Sin bloque no hay nada que aprobar, y el
///    trabajo de enterarse se habría pagado para nada.
pub fn peticion(orden: &str, cwd: &str) -> String {
    format!(
        "Vas a hacer esto: «{orden}». Estás en {cwd}.\n\n\
         Pero antes de tocar nada, enseña CÓMO lo vas a hacer. El plan es tu propia ruta \
         para el trabajo que viene a continuación, no un documento aparte.\n\n\
         PRIMERO ENTÉRATE. Mira lo que haga falta mirar —los ficheros que tocarías, cómo \
         funciona hoy lo que vas a cambiar, qué hay ya hecho— y reparte entre subagentes lo \
         que sea leer mucho. Una ruta deducida del nombre de las cosas no vale: lo que \
         afirmes, compruébalo.\n\n\
         NO CAMBIES NADA TODAVÍA. Puedes lanzar los comandos que quieras para mirar; no \
         escribas ficheros, no instales nada y no arregles de paso lo que encuentres roto. \
         Esto es decidir el camino, no andarlo.\n\n\
         Después decide la ruta: qué haces y en qué orden, qué depende de qué, dónde puede \
         torcerse y cómo sabrás que ha salido bien. Si hay dos caminos razonables, elige \
         uno y di en una frase por qué ese.\n\n\
         Cuando lo tengas, cuenta en dos o tres frases lo que has averiguado y termina con \
         el plan en su bloque:\n\n\
         ```plan: el objetivo en una línea\n\
         1. el primer paso, con el fichero o el comando concreto\n\
         2. el siguiente\n\
         ```\n\n\
         Los pasos son los que vas a ejecutar tú, así que escríbelos como se los escribirías \
         a alguien que va a hacerlo: concretos y comprobables —«añadir X a src/y.rs», no \
         «mejorar el manejo de errores»—, en el orden en que se hacen, y **tantos como pida \
         el trabajo**: dos para algo pequeño, ocho o diez para algo grande. No infles la \
         lista para que parezca un plan ni la partas en pasos que no se pueden comprobar \
         por separado. Si el trabajo termina con una comprobación —un test, un comando que \
         lo demuestra—, ese es el último paso.\n\n\
         Si algo del encargo no se puede hacer, o no se entiende, dilo ANTES del bloque en \
         vez de inventarte un paso que lo tape.\n\n\
         Escribirlo no lo pone en marcha: lo lee el usuario y decide. Escríbelo para que se \
         pueda decir que no, y para que se pueda cambiar un paso sin rehacerlo entero."
    )
}

/// Lo que se le manda cuando el usuario aprueba el plan.
///
/// Le devuelve el plan **numerado por vsioncode**, y no el que él escribió, por
/// lo mismo que [`Plan::lista`]: los números son los que va a marcar y los que
/// va a contar la barra.
///
/// Dos reglas de este texto salieron de verlo trabajar y no de pensarlo:
///
/// - **Un cambio de fichero por mensaje.** De cada respuesta se aplica una sola
///   edición. Sin decirlo, el modelo hacía el plan entero en un turno —tres
///   bloques `edit:` seguidos— y solo llegaba el último, con su `BUSCA` escrito
///   para un fichero que nadie había tocado: no se aplicaba nada.
/// - **La ruta se puede rehacer.** Un plan del que se descubre a mitad que está
///   mal deja al agente entre seguir algo que ya sabe malo o abandonarlo entero.
///   Escribir un plan nuevo en su bloque para y lo devuelve a quien lo aprobó,
///   que es de quien es esa decisión.
pub fn encargo(plan: &Plan) -> String {
    format!(
        "PLAN APROBADO. Objetivo: {}\n\n\
         {}\n\
         Es tu ruta y empiezas ya, en ese orden.\n\n\
         Al empezar cada paso escribe en una línea suelta exactamente:\n\
         {AVANCE} <número>\n\
         Nada más en esa línea y sin explicarla —es para la barra de vsioncode—, y una sola \
         marca por mensaje: la del paso que empiezas, no la de los que piensas hacer \
         después.\n\n\
         El ritmo lo pone una regla técnica: UN CAMBIO DE FICHERO POR MENSAJE. De cada \
         respuesta se aplica una sola edición, así que dos bloques edit seguidos se pierden \
         y el siguiente se escribiría sobre un fichero que no es el que creías. Los \
         comandos sí pueden encadenarse dentro de un paso mientras no dependan de lo que \
         devuelva el anterior.\n\n\
         Los pasos son un mapa, no una jaula. Lo que descubras y no cambie la ruta, hazlo y \
         sigue sin preguntar. Si un paso resulta innecesario, dilo al llegar a él y pasa al \
         siguiente. Pero si la ruta entera resulta equivocada —falta un paso, el orden no \
         sirve, el problema era otro— NO improvises otro trabajo: escribe el plan nuevo en \
         un bloque ```plan: y vsioncode para y se lo enseña a quien lo aprobó, que es de \
         quien es esa decisión.\n\n\
         Al terminar, resume qué pasos quedaron hechos y cuáles no, sin bloque.",
        plan.objetivo,
        plan.lista(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_un_plan_normal() {
        let p = extraer(
            "Ya he mirado el módulo.\n\
             ```plan: arreglar el parpadeo de la barra\n\
             1. Reproducir el fallo con un test que falle\n\
             2. Meter el reloj en la condición del select!\n\
             3. cargo test --release\n\
             ```\n",
        )
        .expect("hay un plan");
        assert_eq!(p.objetivo, "arreglar el parpadeo de la barra");
        assert_eq!(p.pasos.len(), 3);
        assert_eq!(p.pasos[2], "cargo test --release");
    }

    /// La forma de la lista no es la decisión. Rechazar un plan bien pensado
    /// por escribirlo con guiones sería cobrar por la sintaxis.
    #[test]
    fn acepta_cualquier_forma_de_lista() {
        for cuerpo in [
            "1. uno\n2. dos",
            "1) uno\n2) dos",
            "- uno\n- dos",
            "* uno\n* dos",
            "uno\n- dos",
        ] {
            let p = extraer(&format!("```plan: x\n{cuerpo}\n```")).expect("hay plan");
            assert_eq!(p.pasos.len(), 2, "con «{cuerpo}»: {:?}", p.pasos);
            assert!(p.pasos[0].ends_with("uno"), "{:?}", p.pasos);
        }
    }

    /// Un paso partido en dos renglones es un paso, no dos: si contara dos, la
    /// cuenta del avance quedaría descuadrada desde el primer marcador.
    #[test]
    fn un_paso_partido_sigue_siendo_uno() {
        let p = extraer(
            "```plan: x\n\
             1. cambiar la condición del select! para que\n\
             también mire los subagentes\n\
             2. probarlo\n\
             ```",
        )
        .expect("hay plan");
        assert_eq!(p.pasos.len(), 2, "{:?}", p.pasos);
        assert!(p.pasos[0].contains("también mire"), "{:?}", p.pasos);
    }

    /// Si el turno trae dos versiones del mismo plan manda la última, que es la
    /// que se escribió sabiendo más. Igual que con los comandos y las
    /// ediciones.
    #[test]
    fn manda_el_ultimo_plan() {
        let p = extraer(
            "```plan: primera idea\n1. a\n```\n\
             Mejor no, he visto que ya existe.\n\
             ```plan: segunda idea\n1. b\n2. c\n```",
        )
        .expect("hay plan");
        assert_eq!(p.objetivo, "segunda idea");
        assert_eq!(p.pasos.len(), 2);
    }

    /// Un bloque a medias es un plan igual: el turno pudo cortarse por cuota, y
    /// tirar lo escrito obligaría a pagar otra vez por lo mismo.
    #[test]
    fn un_bloque_sin_cerrar_vale() {
        let p = extraer("```plan: algo\n1. uno\n2. dos").expect("hay plan");
        assert_eq!(p.pasos.len(), 2);
    }

    /// Una palabra que empieza por «plan» no abre un plan. Es el mismo fallo
    /// que ya tuvo `agente:` con `agentes:`.
    #[test]
    fn no_confunde_una_palabra_que_empieza_igual() {
        assert!(extraer("```planificación: no\n1. uno\n```").is_none());
        assert!(extraer("```planes\n1. uno\n```").is_none());
        // Y lo que no es un bloque de plan, ni se mira.
        assert!(extraer("```bash\nls\n```").is_none());
        assert!(extraer("no hay ningún plan aquí").is_none());
    }

    /// Distinguir «no ha trazado nada» de «ha escrito un plan vacío»: se
    /// contestan de forma distinta, así que no pueden ser el mismo valor.
    #[test]
    fn un_plan_sin_pasos_no_es_la_ausencia_de_plan() {
        let p = extraer("```plan: un objetivo sin pasos\n```").expect("el bloque estaba");
        assert!(p.pasos.is_empty());
    }

    #[test]
    fn lee_el_avance() {
        assert_eq!(avance("PASO 1\nmiro los tests"), Some(1));
        assert_eq!(avance("PASO 3:"), Some(3));
        assert_eq!(avance("PASO 2/7"), Some(2));
        // El último, que es donde está ahora.
        assert_eq!(avance("PASO 1\nhecho\nPASO 2\nsigo"), Some(2));
    }

    /// Un falso positivo movería la barra a un paso que nadie ha empezado.
    #[test]
    fn el_avance_solo_cuenta_si_la_linea_es_solo_eso() {
        assert_eq!(avance("en el PASO 3 me equivoqué"), None);
        assert_eq!(avance("PASO 3 y luego miro los tests"), None);
        assert_eq!(avance("PASOS 3"), None);
        assert_eq!(avance("paso 3"), None, "en minúsculas es prosa corriente");
        assert_eq!(avance("PASO"), None, "sin número no dice nada");
    }

    /// La marca es para vsioncode; en pantalla sobra.
    #[test]
    fn el_avance_no_se_lee_en_pantalla() {
        let t = sin_avance("PASO 2\nMiro cómo está hoy la condición.");
        assert_eq!(t, "Miro cómo está hoy la condición.");
    }

    /// La numeración que se le devuelve es la nuestra: es la que va a marcar y
    /// la que cuenta la barra. Dos numeraciones para lo mismo se separan en
    /// cuanto el modelo se salta un número.
    #[test]
    fn el_encargo_renumera_los_pasos() {
        let plan = Plan {
            objetivo: "x".into(),
            pasos: vec!["uno".into(), "dos".into()],
        };
        let e = encargo(&plan);
        assert!(e.contains("1. uno"), "{e}");
        assert!(e.contains("2. dos"), "{e}");
        assert!(e.contains("PASO <número>"), "tiene que pedir la marca: {e}");
    }

    /// Un cambio por mensaje, y esto no es estilo: de cada respuesta se aplica
    /// **un** cambio. Sin la regla, el modelo hace el plan entero en un turno
    /// —tres bloques edit seguidos— y solo llega el último, con su BUSCA
    /// escrito para un fichero que nadie ha tocado: no se aplica nada y la
    /// tanda se pierde. Visto en la TUI con haiku, no en un test.
    #[test]
    fn el_encargo_pide_un_cambio_por_mensaje() {
        let e = encargo(&Plan {
            objetivo: "x".into(),
            pasos: vec!["uno".into(), "dos".into()],
        });
        assert!(e.contains("UN CAMBIO DE FICHERO POR MENSAJE"), "{e}");
    }

    /// Las tres cosas que el prompt de trazado no puede perder por una
    /// reescritura: enterarse, no tocar nada y terminar con el bloque.
    #[test]
    fn la_peticion_dice_las_tres_cosas() {
        let p = peticion("arreglar la barra", "/home/x/proyecto");
        assert!(p.contains("arreglar la barra"));
        assert!(p.contains("/home/x/proyecto"));
        assert!(p.contains("NO CAMBIES NADA TODAVÍA"), "{p}");
        assert!(p.contains("```plan:"), "{p}");
        assert!(p.to_lowercase().contains("subagentes"), "{p}");
    }

    /// El plan que se enseña y el que se ejecuta salen de la misma función: si
    /// se numeraran en dos sitios, un día dirían cosas distintas.
    #[test]
    fn la_lista_va_numerada_desde_uno() {
        let plan = Plan {
            objetivo: "x".into(),
            pasos: vec!["a".into(), "b".into(), "c".into()],
        };
        assert_eq!(plan.lista(), "1. a\n2. b\n3. c\n");
    }
}
