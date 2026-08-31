# Arquitectura

Descripción del sistema: cómo está construido y **por qué**, que es lo que no
se deduce leyendo el código.

Documentos relacionados: [`CICLOS.md`](CICLOS.md) detalla los ciclos de
ejecución y las trampas ya resueltas, [`DESARROLLO.md`](DESARROLLO.md) recoge
las convenciones de trabajo, y [`RUTA.md`](RUTA.md), la hoja de ruta.

**Estado del proyecto:** Rust edición 2024, ~15.100 líneas propias, binario de
5,4 MB en release, 415 pruebas unitarias y 12 de integración, clippy sin avisos.

---

## 1. Principio de diseño

Un `bash` real y un modelo de lenguaje leen el mismo registro. El usuario
ejecuta; el modelo observa lo ejecutado sin intervención manual. Y el modelo no
accede al sistema: escribe en la línea del usuario, y pulsar Enter sigue siendo
competencia del usuario.

De ahí derivan dos invariantes que prevalecen sobre cualquier otra
consideración:

1. **El shell es real.** TTY completo bajo pseudoterminal, no una herramienta de
   tipo «ejecuta este comando». Por eso `vim`, `htop`, `ssh`, `less` y
   `git rebase -i` funcionan sin adaptaciones.
2. **El modelo no tiene manos.** El proceso principal se lanza con `--tools ""`
   o, como máximo, `WebSearch,WebFetch`; los subagentes, con `Read,Grep,Glob`.
   Una prueba falla si aparece `Bash`, `Write` o `Edit`.

---

## 2. Capas y responsabilidades

```
     teclado          PTY (bytes)          modelo (AiMsg)
        │                  │                     │
        └──────────┬───────┴──────────┬──────────┘
                   ▼                  ▼
              app.rs  ── un select!, un estado, sin cerrojos ──
                   │
        ┌──────────┼───────────┬──────────────┐
        ▼          ▼           ▼              ▼
     ui.rs     ledger.rs   edicion.rs     agente.rs
    (vista)   (registro)  (aplicar cambios) (subagentes)
```

| Módulo | Responsabilidad |
|---|---|
| `pty.rs` | El `bash` real bajo pseudoterminal. Lector en hilo propio; escáner OSC intercalado con los bytes. |
| `marker.rs` | Extrae del flujo la instrumentación del shell (`Piece::Bytes` / `Piece::Mark`) sin que llegue a pintarse. |
| `ledger.rs` | Registro compartido: comando, directorio, salida acotada, código de salida y duración. Redacta secretos. |
| `ai/mod.rs` | Interfaz del modelo como **enum**, no como trait: `Daemon` (proceso) y `Http` (sin estado) no comparten estructura interna. |
| `ai/daemon.rs` | `claude -p --input-format stream-json`, arrancado **una sola vez**. Su inicialización cuesta entre 2 y 5 s medidos. |
| `ai/http.rs` | Cualquier `/v1/chat/completions` de dialecto OpenAI, por SSE. Filtro de `<think>` implementado como máquina de estados. |
| `proveedores.rs` | Configuración TOML de proveedores. Las claves nunca residen en el fichero: `env:NOMBRE`. |
| `agente.rs` | Subagentes de solo lectura. Su motivo no es la velocidad sino preservar la ventana de contexto principal. |
| `edicion.rs` | Bloques `edit:` con marcadores de conflicto de git y aplicador tolerante. |
| `plan.rs` | El trabajo entero antes de empezarlo: bloque `plan:`, marca de avance y los dos prompts. |
| `safety.rs` | Clasificación de riesgo. Es un cinturón, no el freno. |
| `slash.rs` | Comandos `/`: descubribles al teclear y sin disparo accidental. |
| `keymap.rs` | Reparto de teclas: `Ctrl` íntegro para el shell, `Alt` para la aplicación. |
| `raton.rs` | Clic para enfocar, rueda para desplazar; `/raton` libera la captura. |
| `marcado.rs` | Markdown mínimo: negrita, código en línea, bloques y títulos. |
| `adjunto.rs` | Ficheros e imágenes que entran en la conversación. |
| `skills.rs` | Descubrimiento y entrega de skills instaladas. |
| `voz.rs` | Conversación hablada: reconocimiento (whisper vía `uv`) y síntesis (`piper` + `aplay`), en procesos aparte. |
| `bienvenida.rs` | La configuración inicial: servicios sugeridos, claves y generación o fusión del TOML. |
| `privado.rs` | Cerrar un fichero o una carpeta a su dueño: modo `600`/`700` en Unix, ACL de una entrada y sin herencia en Windows. Y comprobarlo leyéndolo de vuelta. |
| `rutas.rs` | La frontera con el sistema de ficheros del shell: `/c/Users/...` ↔ `C:\Users\...`, y buscar un ejecutable en el `PATH` sin dar por hecho el separador ni la extensión. |
| `app.rs` | Estado y bucle principal. Arquitectura Elm. |
| `ui.rs` | Vista, función pura del estado. |
| `tema.rs` | Colores por función semántica, no por nombre. |

---

## 3. El bucle principal

`app.rs` sigue la arquitectura Elm: un estado, mensajes que lo modifican y una
vista que es función pura de ese estado. Las tres fuentes de mensajes —teclado,
pseudoterminal y modelo— convergen en un único `select!`, de modo que nunca hay
dos escritores concurrentes del estado. No hay ningún `Mutex` en el camino
caliente.

**El repintado está desacoplado deliberadamente.** Un `cat` de un fichero grande
genera miles de lecturas por segundo; pintar en cada una consume la CPU
dibujando fotogramas que nadie llega a ver. El hilo lector solo marca una
bandera y el bucle dibuja como máximo a 30 fps, y solo si hay algo nuevo.

**Caché de líneas ajustadas.** Reajustar la conversación completa en cada
fotograma se percibía como tartamudeo durante el streaming; ahora solo se
reajusta lo que cambia.

**Y de ahí sale la trampa de todo lo que se mueva solo:** «si hay algo nuevo»
es una lista concreta —el shell ha escrito, el modelo está pensando, hay un
aviso, hay subagentes fuera—, y lo que no esté en ella se pinta una vez y se
queda congelado. Le pasó al reloj de los subagentes: el reparto apaga
`thinking`, así que la barra marcaba sucio al lanzarlos y no volvía a
dibujarse hasta que tocaras una tecla. Cualquier giro, cuenta atrás o reloj
nuevo tiene que entrar en esa condición del `select!`, no basta con pintarlo.

---

## 4. Captura de la actividad del shell

`shell/vsion-init.bash` instrumenta el bash con marcadores OSC 133 y 7331:

| Marcador | Significado |
|---|---|
| `ESC ] 133 ; A BEL` | inicio del prompt (estándar OSC 133) |
| `ESC ] 133 ; B BEL` | fin del prompt |
| `ESC ] 7331 ; cwd ; <b64> BEL` | directorio de trabajo |
| `ESC ] 7331 ; cmd ; <b64> BEL` | comienza un comando |
| `ESC ] 7331 ; end ; <ec> ; <us> BEL` | termina, con código de salida y duración |

El payload viaja en base64 para que un comando con `;`, saltos de línea,
comillas o UTF-8 no rompa el análisis.

Tres condicionantes que costaron tiempo y que volverán a aparecer si se toca
esta zona:

- **Los prompts que se regeneran.** `starship`, `oh-my-posh` y `powerlevel10k`
  reescriben `PS1` en cada línea, de modo que unas marcas colocadas una sola vez
  desaparecen tras el primer Enter y la captura deja de funcionar en silencio.
  La solución es reaplicarlas en cada `PROMPT_COMMAND`, situando esa función la
  **última** del array; la de captura va la **primera**, para leer `$?` antes que
  nadie.
- **La configuración regional.** `EPOCHREALTIME` usa el separador decimal del
  locale, y en aritmética de bash la coma es el operador coma. Se normaliza con
  `${t//[!0-9]/}`.
- **El trap `DEBUG` también ve los hooks del prompt.** Se filtran con la bandera
  `__vsion_in_prompt`.

El escáner de `marker.rs` devuelve `Piece::Bytes` y `Piece::Mark`
**intercalados**, no agrupados: agruparlos asocia la salida al comando
equivocado. Es la razón por la que no se usan los callbacks de `vt100`, que
entregan los marcadores después de procesar el bloque completo y pierden la
posición exacta dentro del flujo.

---

## 5. Modos de operación

`Alt+M` rota entre cuatro modos: **observar** (sin coste), **sugerir** (ofrece
ayuda cuando algo falla, sin consultar al modelo hasta que se acepta),
**conversar** (el modelo propone, el usuario ejecuta) y **ejecutar** (el modelo
lanza comandos en la terminal del usuario, con su directorio, sus alias y su
entorno, y todo queda en el mismo registro).

**Modo libre** es un submodo de *ejecutar*: eleva el techo de pasos, sube el
esfuerzo de razonamiento del modelo y habilita el reparto entre subagentes.

Solo puede activarse dentro de *ejecutar*, y salir de ese modo lo desactiva. La
razón es de coherencia: el modo libre libera la ejecución automática, y donde no
hay ejecución automática no hay nada que liberar; mostrar `LIBRE` en la barra
estando en *observar* prometería una capacidad inexistente. La activación se
canaliza por `App::cambiar_modo`, de forma que la relación entre modo y submodo
no dependa de recordarla en cada punto de cambio.

Dos excepciones deliberadas: `/analizar` y `/plan` sí cambian el modo por su
cuenta, porque pedir un análisis a fondo —o el plan de un trabajo— **es** pedir
que el sistema se entere solo, y enterarse exige poder mirar el disco. `/plan`
además desactiva por su cuenta una parte del modo libre mientras traza: ver
§5.5.

**Vistas:** `Alt+Z` rota entre los dos paneles, solo shell y solo asistente. La
vista de solo asistente ocupa la pantalla completa; una versión anterior la
centraba a 96 columnas por el argumento tipográfico de la medida de línea, y en
uso real un recuadro flotando entre bandas negras se percibe como un fallo de
dibujado, no como una decisión.

### 5.1 Quién ejecuta cada comando

El modo determina quién ejecuta **por defecto**, y el valor por defecto no
siempre es el adecuado. Pedir «dame el comando y lo ejecuto yo» no producía
ningún efecto: el modelo obedecía, pero el modo es global y vsioncode lanzaba el
comando igualmente.

El modelo puede ahora devolver el mando de un comando concreto: una línea
`LO-LANZAS-TU` sobre el bloque hace que vsioncode lo deje preparado para
`Alt+↵` en lugar de ejecutarlo. Lo que distingue esto del modo conversar es que
**el comando queda registrado como pendiente**: en cuanto el usuario lo lanza,
el modelo recibe la salida y continúa donde estaba.

No hay lista de frases que activen el mecanismo. Detectar «dame el comando» con
una comparación de texto falla en ambos sentidos y envejece mal; interpretar la
petición es precisamente la función del modelo. Los ejemplos viven en el prompt,
donde son ejemplos, y no en el código, donde serían reglas.

**Con una excepción, y está medida:** la instalación de software. De tres
modelos pequeños evaluados, dos ejecutaban `apt install` por su cuenta con la
regla escrita delante. Instalar introduce código externo en la máquina y se
revierte mal, así que lo sujeta `safety::instala_software` en lugar de confiar
en que el modelo lo recuerde. El verbo importa tanto como el programa: `apt
list`, `pip show` y `cargo build` no instalan nada, y bloquearlos por el prefijo
sería un aviso improcedente.

### 5.2 Qué escribe el asistente mientras trabaja

El panel llegó a comportarse como un segundo terminal que repetía el primero: el
modelo ejecutaba `lspci` y a continuación reproducía la salida completa en la
conversación, con su comentario encima.

No era verbosidad del modelo, sino un dato que le faltaba: **nadie le había
indicado que el usuario está viendo la terminal**. Sin esa información hace lo
razonable —relatar lo que ha visto, por creer que es su única forma de
mostrarlo— y pedirle brevedad no lo corrige, porque considera necesario lo que
hace. El prompt del modo ejecutar empieza ahora por describir dónde escribe: la
pantalla está partida, la salida íntegra está a la izquierda y en directo, y
todo lo que se cuente de ella se lee dos veces.

De ahí se sigue el resto: una línea de intención antes de cada comando, silencio
tras el resultado salvo que haya algo que **no** se vea en pantalla —un cambio
de plan, un error que va a rodear— y el detalle reservado para el resumen final,
que es lo único que se relee. La regla se repite en la cola de cada resultado y
no solo en el prompt de sistema, porque el prompt queda a veinte turnos de
distancia mientras que la salida que invita a comentar llega pegada al turno.

La interfaz refuerza la distinción: **un turno que trae comando se atenúa y uno
sin comando, no**. No es una lista de casos, es el propio ciclo —con comando hay
trabajo en curso; sin comando el trabajo terminó y se está informando—, de modo
que el resumen destaca sin añadir ninguna marca adicional.

### 5.3 Cómo se lleva un encargo hasta el final

El prompt de sistema describe qué puede hacer el modelo y el del modo de trabajo
cómo se expresa mientras lo hace. Faltaba una tercera pieza: qué significa haber
terminado, qué hacer con una duda que aparece a mitad, y qué se cuenta de lo que
salió mal.

Que faltaba se vio antes en el código que en el prompt. Quedarse sin cuota a
medio trabajo se anunciaba como «Hecho en 19 comandos» —el modelo no proponía
otro comando, y el bucle interpretaba esa ausencia como tarea terminada—, y hubo
que distinguir los dos casos con una condición. **Una regla de conducta resuelta
con un `if` es una regla de conducta que no estaba escrita.** En el modo normal
el desajuste es inocuo, porque son tres turnos y hay alguien mirando; en el modo
libre el agente encadena decenas de pasos por su cuenta y esa regla es lo único
que separa una tanda aprovechable de un montón de comandos.

Las seis que se fijaron: el encargo no se agranda ni se recorta por cuenta
propia; terminar es terminar, y lo que quede fuera se dice en el resumen en vez
de callarse; se cuenta lo que ocurrió y no lo que se pretendía, con el fallo
delante cuando lo hay; ante una duda se resuelve primero todo lo que no dependa
de ella; una objeción ya planteada y desoída queda decidida; y un error propio
se corrige sin disculpas ni recuento.

Van en la explicación larga del modo y no en el recordatorio de los turnos
siguientes, por el mismo criterio que separa a las dos: se dicen una vez y se
sostienen, a diferencia de quién lanza cada comando o de que el usuario ve la
terminal, que se olvidan a los pocos turnos y por eso se repiten.

### 5.4 Enterarse cuesta contexto, y repartirlo es gratis

Antes de tocar nada, el agente se dedica a averiguar cómo funciona lo que va a
cambiar: una sucesión de `grep`, `sed` y `cat` que en las sesiones medidas llega
a ocho comandos seguidos. Cada una de esas salidas se queda en la conversación y
vuelve a enviarse en todos los turnos posteriores, de modo que el
reconocimiento entero se arrastra hasta el final de la tanda. Un subagente hace
ese mismo trabajo en su propia ventana y devuelve solo la conclusión.

La capacidad existe y el prompt la explica, incluido cuándo compensa. Aun así no
se usaba: catorce bloques `agente:` en 1219 turnos, y 31 de 34 sesiones sin
ninguno. La explicación más plausible no es que el modelo no lo entienda, sino
que **la decisión de repartir hay que tomarla antes de saber que hará falta**.
Nadie empieza una investigación diciendo «voy a mirar ocho sitios»: lanza un
comando, ve algo, tira del hilo, y cuando lleva seis ya no compensa delegar
porque casi todo está leído. Lo que le falta no es la instrucción, es un dato
sobre su propia trayectoria que el programa sí tiene.

De ahí que el recordatorio viaje en la cola del resultado y no en el prompt de
sistema, exactamente igual que el balance periódico y por el mismo motivo: en el
turno en que sirve, es lo último que ha leído. Aparece cuando lleva seis
comandos seguidos sin producir nada, una sola vez por racha, y redactado como un
dato y no como una orden —si ya sabe lo que necesitaba, seguir él es lo
correcto y la frase lo dice—.

El seis sale de medir, no de estimar. Contada en serio la racha —todos los
tramos de un comando encadenado tienen que limitarse a mirar, porque
`sed …; cargo build` empieza como un vistazo y termina compilando— la mitad son
de uno o dos comandos y solo seis pasaron de cinco en 34 sesiones. Con cuatro el
recordatorio entraría en la mitad de las tandas, y lo que entra siempre deja de
leerse.

Lo que se descartó por el camino tiene el mismo origen: agrupar varios comandos
en un turno ahorraría contexto, pero el valor del modo libre es que el agente ve
cada resultado antes de decidir el siguiente, y pedirle que administre lo que
gasta le hace administrar en vez de investigar. Es la misma razón por la que en
ese modo no se le dice cuántos comandos le quedan.

#### Un subagente no cuesta lo que su jefe

Durante mucho tiempo el reparto pasaba `cfg.model` tal cual a cada subagente, de
modo que con opus arriba se lanzaban hasta cuatro opus abajo. No era una
decisión: era herencia por omisión, y con una cuota de suscripción detrás es la
diferencia entre una tarde de trabajo y media hora. El trabajo de un subagente
—leer con `Read`, `Grep` y `Glob` y devolver un resumen— lo hace bien un modelo
de gama media; lo que no cabe en su ventana no cabe mejor por ser caro.

La regla que se aplica ahora es **nunca por encima de quien manda, y por defecto
nunca por encima de sonnet**: con opus arriba, sonnet abajo; con haiku arriba,
haiku abajo. Si el modelo principal no se reconoce —otro proveedor, o ninguno—
se cae en sonnet en lugar de arrastrar lo que hubiera en `cfg.model`, que
conserva el último alias de claude aunque se haya cambiado a Groq y sería un dato
viejo.

Sobre esa base hay dos mandos, y la asimetría entre ellos es lo importante:

- **El agente elige a la baja.** `` ```agente(haiku): … `` para lo que es
  localizar y contar. Puede bajar de gama por su cuenta —sabe mejor que nadie si
  un encargo es un `grep` con patas— y no puede subir: `agente::bajo_techo` corta
  al techo lo que se pase, porque el límite de gasto no lo puede decidir quien no
  lo paga. Es el mismo criterio que el resto del proyecto: techos sí, presión no.
- **El usuario elige en los dos sentidos.** `/agente modelo <nombre>` fija el
  modelo de la sesión, y ese valor pasa a ser a la vez el defecto y el techo.
  Y `/agente [modelo] <tarea>` lanza uno directamente, sin pasar por el cerebro,
  con el modelo que se diga y sin techo: ahí quien decide es quien paga.

Ese `/agente` cubre un caso que antes obligaba a dar un rodeo caro: cuando se
sabe de antemano que algo es leer mucho y contar poco, pedírselo al modelo
principal para que lo reparta cuesta un turno de ida y otro de vuelta del modelo
más caro de la casa para decidir algo ya decidido. Lo que traiga el subagente
vuelve por el mismo camino que el resto —a la conversación **y** al cerebro—,
que es lo que lo convierte en delegar y no en preguntar aparte: si el informe no
llegara arriba, la respuesta siguiente no lo tendría y habría que pegárselo a
mano.

Y con qué modelo va cada subagente se dice en la conversación al lanzarlo
(`subagente (sonnet): …`). Sin eso el ahorro sería invisible, y un ahorro que no
se ve no se distingue de una promesa.

#### Un subagente puede ir con cualquier modelo configurado

Delegar estuvo atado a la suscripción mucho más tiempo del que debía: `/agente`
ofrecía —y entendía— los tres alias de `claude` y nada más. Quien tenía cinco
proveedores y veinte modelos declarados en su fichero podía hablar con todos y
delegar en tres; los otros diecisiete no existían para el reparto. Es el fallo de
§9 otra vez: una opción que la interfaz no enseña no existe para quien la usa.

Y era además el reparto al revés. Un subagente hace el trabajo más mecánico de la
casa —leer mucho y contar poco— y es justo donde un modelo rápido y gratuito
rinde igual que uno caro; tenerlo atado a la cuota de la suscripción significaba
gastar lo caro en lo tonto.

Ahora `agente::posibles` arma la lista con el fichero entero y `agente::Motor`
resuelve cada nombre una sola vez: `Daemon` para los de `claude` —un `claude -p`
con `Read`, `Grep` y `Glob`—, `Http` para el resto. Tres consecuencias que
importan:

- **Un subagente por HTTP no tiene herramientas que declarar**, porque la API de
  chat no sabe de eso. Se las da `agente::lector`: las mismas tres, escritas en
  Rust y encerradas con `canonicalize` en el directorio de trabajo. Ahí la
  promesa de solo lectura no depende de que otro proceso la cumpla, sino de este
  código — y hay un test que intenta salirse y falla si lo consigue.
- **El protocolo es de texto, no `tool_calls`.** Por lo mismo que los comandos
  van en bloques ` ```bash `: la llamada a herramientas es donde los proveedores
  se diferencian y donde los modelos pequeños fallan. El subagente escribe
  `LEER`, `BUSCAR` o `LISTAR` al principio de una línea y se le contesta con el
  contenido; un mensaje sin órdenes es su informe. Eso lo cumple cualquiera,
  incluido un modelo local.
- **Los topes salieron de una prueba, no de una estimación.** Con 400 líneas por
  lectura, la primera prueba real mandó once mil tokens y Groq devolvió `413`:
  su plan gratuito corta en ocho mil por minuto. Hay tope por lectura, tope de
  rondas y presupuesto total, y al agotarse se le pide el informe en vez de
  cortar — un informe incompleto que dice qué le faltó sirve; un error, no.

La escalera de gamas sigue siendo la de `claude`, y solo la suya: un modelo de
Groq no está por encima ni por debajo de sonnet, es otra cuenta con otro precio,
y ordenarlos entre sí sería inventar una equivalencia que no existe. Lo que sí se
corrige es lo contrario — con un techo de otro proveedor, un `agente(opus)` del
cerebro se cambia por el que fijó el usuario: pedir la suscripción cuando se la
había dejado fuera es gastar lo que no se decidió gastar.

Y la lista puede quedar **vacía**: sin fichero y sin el proceso `claude` en el
`PATH`, `/agente` no ofrece nada y lo dice. No hay lista de consolación, porque
ofrecer lo que no se puede lanzar es prometer lo que no se tiene.

### 5.5 El plan: mover la decisión a donde es barata

En modo ejecutar el humano decide comando a comando; en modo libre, ni eso. Para
un encargo de veinte pasos las dos cosas fallan por el mismo sitio: **nadie ha
aprobado nunca el trabajo, solo sus trozos**, y cuando el tercer comando revela
que el agente entendió otra cosa ya hay ficheros tocados. Enterarse de lo que
iba a hacer exigía verlo hacerlo.

`/plan` parte el encargo en dos fases y pone la decisión entre ellas:

- **Trazar.** El agente se entera —lee, lanza comandos de lectura, reparte entre
  subagentes— y termina con un bloque ` ```plan: ` de pasos numerados. Aquí no
  toca nada: aunque el modo libre esté encendido, una edición se queda esperando
  la pulsación. Esa guarda vive en el código y no en el prompt, porque
  «planificar no toca» es justamente lo que no puede depender de que el modelo
  se acuerde. Es la única excepción que la planificación añade al resto del
  programa.
- **Ejecutar.** Solo tras `Alt+↵`. El plan vuelve como encargo y el trabajo
  transcurre como cualquier otra tanda del modo libre.

El protocolo es un bloque de texto y no una llamada a herramienta, por lo mismo
que los comandos van en ` ```bash `. Y **viaja en la petición, no en el prompt de
sistema**: cada línea del prompt de sistema se paga en todos los turnos, y el
bloque `plan:` solo significa algo mientras hay una planificación viva. Fuera de
ahí, un bloque así es prosa del modelo y no se interpreta.

**El plan es una ruta, no un formulario**, y eso son dos reglas concretas:

- **Se puede rehacer hablando.** Un bloque `plan:` se atiende en cualquier fase:
  sobre uno ya propuesto lo sustituye —«quita el paso 2 y añade este»— y sobre
  uno en marcha para el bucle y lo devuelve a aprobación, diciendo por qué paso
  iba. Mirándolo solo mientras se trazaba, un plan que resultaba equivocado a
  mitad dejaba al agente entre seguir algo que ya sabía malo o abandonarlo
  entero; ahora replantear es un camino declarado, y el prompt de ejecución lo
  dice.
- **Los pasos son un mapa y no una jaula.** Lo que el agente descubra y no
  cambie la ruta, lo hace y sigue; un paso que resulta innecesario se dice y se
  pasa. Lo que no puede es agrandar el encargo por su cuenta, que es §5.3.

Que el plan sea datos y no prosa da dos cosas que ningún párrafo bien redactado
daría: la barra puede decir «plan 3/7» mientras la conversación se va hacia
arriba —el agente marca `PASO n` al empezar cada paso—, y al acabarse la tanda
vsioncode puede decir **en qué paso** quedó, que es lo único de la tanda que él
sabe y el modelo no. Sin eso volvería el fallo de «Hecho en 19 comandos» sobre
un trabajo que se había parado a medias.

**Cortar no es olvidar.** `Esc` a mitad de un trazado para el turno y cierra la
fase, pero lo averiguado sigue en la conversación: `interrumpir_turno` relanza
el proceso **retomando la sesión** en vez de empezar de cero. Antes no, y el
precio era el peor posible —veinte comandos de investigación y un agente que
volvía sin saber nada de lo que acababa de averiguar ni de lo que ya había
descartado—. El proceso hay que relanzarlo igual, porque las dos veces que esto
hace falta (colgado, o con la conexión rota) no vuelve solo; lo que no hay que
tirar es la conversación. Con un proveedor por HTTP no hay sesión que retomar y
se dice, que prometer continuidad sin darla es peor que la pérdida.

Sin ninguna marca recibida no se pinta «0/7»: por el paso cero no va nadie, así
que la barra dice cuántos pasos hay y nada más. Y el plan vive en la
conversación: si el cerebro se relanza con hilo nuevo —`/limpiar`, cambiar de
modelo, un turno interrumpido— el plan se va con ella, porque un `Alt+↵` que
mandara ejecutar unos pasos a un proceso que no ha oído hablar de ellos sería
peor que no tener plan.

**Dos cosas se aprendieron abriendo vsioncode, no en los tests:**

- El prompt de trazado necesita las reglas del modo de trabajo pegadas al turno.
  Sin ellas el modelo solo conoce la regla general del prompt de sistema —«no
  ejecutas nada por tu cuenta»— y se queda sin poder enterarse de nada. Ahora
  las arma `App::reglas_de_modo`, que es la misma pieza que usa un mensaje
  normal.
- El encargo tiene que pedir **un cambio de fichero por mensaje**. Sin esa
  frase, el modelo hacía el plan entero en un turno —tres bloques `edit:`
  seguidos—; de cada respuesta se aplica un solo cambio, así que llegaba el
  último con su `BUSCA` escrito para un fichero que nadie había tocado, no se
  aplicaba nada y la tanda entera se perdía. La regla es esa y no «un paso por
  mensaje»: los comandos de un mismo paso pueden encadenarse, y pedir más
  ceremonia de la que exige el mecanismo es lo que convierte un plan en un
  trámite.

---

## 6. Modelo de seguridad

El freno **no** es la lista de comandos peligrosos. Existe evidencia de que las
listas negras son estructuralmente frágiles: en un estudio sobre 1.709 listas
reales de proyectos con agentes de código, entre el 69 % y el 98,6 % no
bloqueaban por completo la operación que pretendían bloquear.

El freno es de diseño: **escribir un comando en la línea del usuario no lo
ejecuta.** `safety.rs` decide cuánta información se muestra antes de pulsar
Enter, no si se permite pulsarlo.

Detalles relevantes de la clasificación:

- `sudo` pertenece a `PRIVILEGED`, no a `DANGEROUS`: se clasifica **lo que va
  detrás**. Tratarlo como peligro fijo generaba avisos improcedentes.
- Un comando compuesto hereda **el nivel más alto** de sus partes.
- Todos los patrones se declaran en minúscula; hay una prueba que lo exige.
- **Inyección desde internet:** un turno que ha consultado la web no ejecuta
  automáticamente. Solo marca el turno un `tool_result` con éxito y no vacío;
  antes, un `about:blank` bloqueaba un `cat` local, que es exactamente el aviso
  improcedente que hay que evitar.

> Lección ya pagada dos veces —con `sudo` y con la defensa frente a inyección—:
> un aviso que salta cuando no corresponde se aprende a ignorar, y entonces
> tampoco avisa cuando importa. **Precisión antes que amplitud.**

---

## 7. Edición de ficheros

El formato usa los marcadores de conflicto de git deliberadamente: cualquier
modelo los ha visto miles de veces y los reproduce sin necesidad de
explicaciones.

````text
```edit: src/app.rs
<<<<<<< BUSCA
let x = 1;
=======
let x = 2;
>>>>>>> CAMBIA
```
````

**La tolerancia reside en quien aplica el cambio, no en el prompt**, porque el
objetivo declarado incluye los modelos gratuitos, que rompen las sintaxis
estrictas. La cascada de coincidencia es:

1. Exacta.
2. Ignorando el sangrado (el fallo más frecuente).
3. Por anclas: coinciden la primera y la última línea.

Cada escalón exige que la coincidencia sea **única**. Si el texto aparece dos
veces se rechaza en lugar de elegir: modificar el punto equivocado de un fichero
es peor que no modificar nada.

`parece_edicion_fallida()` existe por un fallo real: el extractor no reconocía
la etiqueta, no se aplicaba nada y **el modelo afirmaba que el cambio estaba
hecho**. Un fallo silencioso que además informa incorrectamente.

### El cambio se ve

Todo cambio —propuesto o ya escrito— sale a la conversación con su diff:
`edicion::diff()` monta un bloque ` ```diff ` con las líneas que se van y las
que entran, cada una con su número **en su propio fichero**, y dos líneas de
contexto a cada lado. `marcado.rs` reconoce ese bloque y lo devuelve como
`Linea::Cambio`, que `ui.rs` pinta con fondo rojo o verde hasta el borde del
panel.

Es una decisión sobre revisión, no sobre estética: `−7 +3 líneas` dice cuánto
cambia y no dice qué, y **un cambio que no se ve no se revisa: se acepta**. En
modo libre pesa el doble, porque ahí ya no hay una pulsación por medio y el
diff es lo único que queda entre un fichero tocado y enterarse.

El recuento y el diff comparten `tramo()` a propósito. Calculándolo cada uno
por su lado, un día dirían cosas distintas del mismo cambio.

Un fichero reescrito entero se corta a `MAX_DIFF` líneas por lado y se dice
cuántas faltan: el panel es una conversación, no un visor de ficheros.

---

## 8. La configuración inicial

La primera vez que vsioncode se abre en una máquina no hay `proveedores.toml`
que leer, de modo que no sabe a quién preguntar. Antes eso se resolvía con un
comando (`/proveedores`) que escribía un fichero de ejemplo para editar a mano,
lo cual exige saber de antemano que ese comando existe. Es el mismo problema que
§9 describe para la configuración: **una opción que la interfaz no enseña no
existe para quien la usa**.

`bienvenida.rs` implementa un asistente de cuatro pasos —portada, elección de
servicio, clave y resumen— que se abre solo cuando falta el fichero de
configuración, y que puede reabrirse con `/configurar` o `--configurar`.

Decisiones que conviene conocer antes de tocarlo:

- **La condición de «primera vez» es la ausencia del fichero**, no una marca
  aparte. Quien lo borre para empezar de cero vuelve a ver la puesta en marcha,
  que es lo que esperaría.
- **Se escriben dos ficheros, no uno.** El TOML se comparte, se copia entre
  máquinas y aparece en capturas de pantalla; las claves van en un `.env` con
  permisos `600` y el TOML solo las nombra (`env:GROQ_API_KEY`). Es la misma
  regla que ya regía la configuración escrita a mano, aplicada a la generada.
- **Si la variable ya está exportada, no se vuelve a pedir.** Quien tiene
  `GROQ_API_KEY` en su `.bashrc` no debería teclearla otra vez.
- **No se valida la clave contra el servidor.** Una comprobación en red
  convierte la primera pantalla en una espera que puede fallar por causas
  ajenas —sin cobertura, servicio caído, proxy—, y dejar a alguien atascado en la
  puerta de entrada es peor que dejarle pasar con un dato por confirmar. Sí se
  rechaza lo que se ve a simple vista: campo vacío, espacios, una URL pegada por
  error o algo demasiado corto.
- **Al reabrirla, parte de lo que ya hay.** Lee el catálogo actual y marca lo
  configurado, porque también se abre con `/configurar` sobre una instalación en
  marcha: empezar en blanco haría que guardar borrara lo anterior.
- **El fichero existente se fusiona, no se reescribe.** Reescribirlo entero se
  llevaría por delante lo añadido a mano —otro servidor, otros modelos, notas
  propias—, y perder la configuración de alguien por abrir una ventana es
  exactamente lo que no puede ocurrir. Se añaden los bloques que faltan y se
  actualizan `por_defecto` y `env_file` en su sitio.
- **Los enlaces se abren desde la aplicación**, con `O` o con un clic sobre la
  dirección. Una TUI no puede convertir un texto en enlace pulsable —lo que
  pinta son celdas—, así que la dirección se muestra escrita y la apertura es
  una acción explícita, resuelta con el abridor del sistema.
- **Al cerrar se relee el fichero recién escrito** en vez de construir el estado
  en memoria, de forma que lo que arranca es exactamente lo que hay en el disco
  y un error de escritura se manifiesta en el momento.
- **La ventana se queda con todas las teclas mientras está abierta**, incluidos
  los atajos globales: sin nadie a quien preguntar, «explicar lo último» o «modo
  libre» no significan nada, y una tecla que no hace lo que anuncia es peor que
  una tecla ausente.

El nombre y la versión aparecen enmarcados solo en esta pantalla, dibujados con
los mismos trazos que el resto de la interfaz. Identifican qué se acaba de abrir
—que es lo que nadie sabe la primera vez— sin competir con la única acción que
hay que realizar aquí: elegir un proveedor.

---

## 9. Modelos y proveedores

La configuración vive en `~/.config/vsioncode/proveedores.toml`. Las claves se
declaran como `env:NOMBRE` o mediante `env_file`, nunca literales: un fichero de
configuración termina en una copia de seguridad o en una captura de pantalla.

Verificados de extremo a extremo: **Groq** (~1 s hasta el primer token),
**NVIDIA NIM**, **Google AI Studio** y cualquier servidor local por `ollama` o
`llama.cpp`.

Lo que hace viable esta amplitud: vsioncode **no usa llamada a herramientas del
modelo**. Solicita texto y lo interpreta. El requisito para admitir un modelo
nuevo es «sabe escribir un bloque de código», no «implementa el tool calling de
OpenAI».

Decisiones de dependencias que deben respetarse al modificar `Cargo.toml`:

- `reqwest` con `rustls-no-provider` y `rustls` con **`ring`**. El proveedor
  criptográfico por defecto (`aws-lc-rs`) añadía 2 MB al binario.
- El proveedor criptográfico se instala **dentro de la biblioteca**, no solo en
  `main`: de lo contrario `reqwest` entra en pánico desde los ejemplos y las
  pruebas.
- La característica `http2` es obligatoria: sin ella, NVIDIA y Gemini no
  responden y la petición queda colgada sin error.

**Un proveedor no es un modelo.** Groq sirve cinco y NVIDIA más de cien;
mientras la configuración admitía un solo `modelo` por bloque, probar otro
obligaba a repetir el servidor completo con su URL y su clave. Cada
`[[proveedor]]` lleva ahora sus `[[proveedor.modelos]]`, y `/modelo` abre un
selector agrupado por servidor.

**El control del razonamiento es el parámetro de mayor impacto.** La mayoría de
los modelos actuales razonan por defecto, lo que en una terminal se traduce en
segundos de pantalla quieta. Medido con el mismo cliente y la misma pregunta:

| Modelo | Por defecto | Con el interruptor aplicado |
|---|---|---|
| `qwen/qwen3.6-27b` | 972 ms | **165 ms** |
| `nemotron-3-super-120b` | 6,5 s | **564 ms** |
| `gemini-3.6-flash` | 3,7 s | **1,0 s** |

Son dos interruptores y no uno porque los proveedores no han convergido:
`reasoning_effort` en casi todos, con valores distintos (`none`, `minimal`,
`low`…), y `chat_template_kwargs.enable_thinking` en los Nemotron de NVIDIA.
Cada modelo declara el suyo en el TOML y no se traduce ninguno: inventar una
equivalencia produciría valores que el servidor rechaza con un 400.

Dos reglas generales que salieron de fallos reales en esta zona:

- **Una opción de configuración que la interfaz no muestra no existe para quien
  la usa.** El fichero se leía entero y la interfaz no reflejaba nada: `/modelo`
  ofrecía los alias escritos en el binario y `por_defecto` se analizaba sin que
  nadie lo aplicara. Por eso `slash::candidatos` recibe los proveedores y
  `Arranque::preparar` aplica `por_defecto` antes de lanzar el modelo.
- **Lo que se aplica al streaming solo puede examinar caracteres.** El filtro de
  emojis trabajaba con `lines()` y se aplica a cada fragmento SSE, que puede ser
  `"\n"` o `` "```" ``; al consumir el salto de línea final, la valla dejaba de
  abrir línea, `extract_commands` no encontraba nada y la ejecución automática
  dejó de funcionar por completo. Cualquier limpieza que necesite la línea
  entera va después, sobre el texto completo del turno.

---

## 10. Interfaz: selector de modelos y menú de comandos

- **`/modelo` abre una ventana, no un volcado de texto.** Los servidores actúan
  como cabecera, sus modelos aparecen debajo, el activo se marca con `●` —glifo
  además de color, para que siga siendo legible en monocromo— y cada uno muestra
  su tiempo hasta la primera palabra. La latencia es lo que convierte una lista
  en una decisión informada.
- **El menú de `/` muestra catorce comandos, no diecisiete.** Llegó a tener
  diecisiete para siete filas visibles, con lo que los más útiles quedaban
  enterrados. Lo que ya tiene atajo (`/ayuda`, `/salir`, `/captura`, `/libre`,
  `/voz`, `/pantalla`) y lo que se usa una vez (`/microfono`, `/raton`) pasó a
  `slash::OCULTOS`: se reconocen igual y aparecen en
  `/ayuda`, pero no ocupan sitio. **El menú es para lo que se usa a diario y no
  tiene atajo.** `/configurar` estuvo entre los ocultos y volvió al menú por lo
  contrario: no tiene tecla, y quien quiere añadir un servicio lo busca
  exactamente ahí. `/agente` entró por lo mismo: no tiene tecla, y delegar una
  tarea concreta es de uso diario en cuanto se sabe que se puede.

- **Lo que se elige dentro de un comando también sale del disco.** Es la regla
  de §9 un nivel más abajo, y se pagó dos veces en el mismo sitio: `/modelo`
  ofrecía los servidores y, sueltos al final, los tres alias de claude escritos
  en el binario —cuatro nombres de servidor seguidos de tres modelos de uno de
  ellos, sin nada que dijera cuál era cuál—; y `/agente modelo ` no ofrecía
  nada, porque el menú se quedaba en el segundo nivel y volvía a proponer la
  palabra «modelo». Ahora hay un `slash::Catalogo` que la aplicación rellena
  con lo que hay de verdad —proveedores, sus modelos, los alias que puede
  llevar un subagente y los motores de voz descargados— y el menú solo ordena y
  filtra. Los servidores llevan a sus modelos detrás, en el orden del fichero:
  la misma agrupación que la ventana, porque es la misma decisión.
- **Un modelo que se ofrece tiene que poder elegirse.** `/modelo <nombre>`
  busca ahora entre los declarados y cambia de servidor si hace falta, igual
  que la ventana. Antes escribía el nombre en `cfg.model` —el campo de los
  alias de claude— y el proveedor se quedaba como estaba: elegir de la lista no
  cambiaba de modelo.
- **Una fila que se envía al elegirla tiene que ser una orden completa.**
  `/agente modelo` no lo es —falta decir cuál—, y como el menú lo daba por
  cerrado, señalarlo y pulsar Enter lanzaba un subagente con la tarea
  «modelo»: una llamada al modelo pagada por elegir una fila.
- **Y lo que ofrece un comando es lo que ese comando puede hacer de verdad.**
  `/agente` enseñaba tres alias de `claude` con veinte modelos configurados;
  ahora la lista sale del fichero entero y cada fila trae ya resuelto con qué se
  lanzaría, de modo que no se puede ofrecer una que al elegirla no arranque.

#### La barra de delegar

Con dieciocho modelos, el menú dejó de ser el sitio para elegir uno: la lista no
cabía, el verbo `modelo` se iba al fondo, y aun eligiendo había que escribir
después el comando con la tarea. Delegar es de lo que más se usa en cuanto se
sabe que está, y lo que más se usa es lo que menos debe costar teclear.

De ahí la barra (`app::delegar`): se abre con **←** desde la caja del agente o
escribiendo `/agente`, enseña con quién va —nombre y de quién es, que `haiku`
sale de tu suscripción y `qwen3.6-27b` de tu clave de Groq—, **↑** despliega los
agentes y Enter manda el encargo. Tres decisiones que no son de estética:

- **← solo con la caja vacía.** Con texto escrito la flecha es del cursor, y
  robársela para un atajo rompería lo único que una caja de texto promete. Lo
  mismo vale dentro: **↑** despliega solo si no has escrito nada.
- **Es una capa, no un modo.** Se queda las teclas que son suyas y deja pasar
  las demás, así que Alt+Q sigue saliendo y Alt+H sigue abriendo la ayuda. Y
  solo con el foco en el agente: si el foco está en el shell, lo que tecleas es
  para el shell — quedárselo sería la peor forma posible de fallar aquí.
- **Elegir en la barra fija la sesión**, igual que `/agente modelo <n>`, y se
  dice en la conversación. Quien elige un agente suele querer ese mismo para lo
  siguiente, y volver a elegirlo cada vez cobraría dos pulsaciones por una
  decisión ya tomada.

El menú de `/agente ` dejó de ofrecer la lista: dos sitios para la misma
decisión son dos sitios que se aprenden por separado y divergen. Lo que sigue
funcionando es escribir el nombre —`/agente haiku mira los tests`— para quien se
lo sabe, y `/agente modelo <n>`, que es el tercer nivel del menú.

Un argumento entre corchetes es opcional, de modo que `/modelo` ya está completo
sin él y Enter lo envía directamente.

Se acepta lo señalado con Tab o con **→**, que es como se acepta una sugerencia
en cualquier shell y deja la mano donde ya está —en las flechas, eligiendo
fila—. La flecha solo completa con el cursor al final del texto: si queda algo a
la derecha sigue siendo la flecha, porque robársela en mitad de una línea
dejaría sin forma de volver a una letra para corregirla sin cerrar el menú.

### La barra de estado

La barra es el único sitio que está siempre en pantalla, así que lo que se pone
ahí tiene que ser algo que se **mira**, no algo que se recuerda: el modo, el
estado de la captura, el `cwd`, el resultado del último comando, el micrófono.

Dos añadidos, y los dos por la misma razón —el dato existía y solo se conseguía
preguntando por él, o sea después de que ya hiciera falta—:

- **Lo gastado (`↑1.2M ↓34k`)**, mientras haya algo que contar. La cuota de
  este proyecto se va por la entrada —el contexto que se reenvía en cada
  turno—, y ese número solo salía escribiendo `/coste`. En la barra va en
  orden de magnitud, que es lo que se mira de un vistazo; el desglose y el
  porcentaje que sirve la caché siguen en `/coste`.
- **Lo que tardó el último comando**, y solo si tardó un segundo o más. Un
  `0s` detrás de cada `ls` es un número que nunca dice nada, y entonces
  tampoco se lee el día que el build tarda tres minutos.

Los ratos se dicen en minutos a partir del minuto (`24m 3s`, no `1443s`). En
modo libre un turno de veinte minutos es normal, así que el caso de los
minutos no es el raro.

Lo que **no** se ha puesto ahí, aunque el dato esté: el esfuerzo del modelo.
Va atado a `LIBRE`, que ya sale, y repetirlo sería una etiqueta que confirma
lo que la de al lado ya dice.

#### Los subagentes, cuando son más de uno

Con uno trabajando basta una línea en la barra: el giro, el recuento, cuánto
lleva y en qué anda. El giro es **el mismo** que el de «pensando», porque desde
la barra es el mismo hecho —hay alguien trabajando—, y darle un dibujo propio
haría creer que son dos cosas distintas. Va sin fondo de color: en esta barra el
fondo es el distintivo del modo y de nada más, y una segunda chapa al lado se
lee como si hubiera dos modos puestos.

Desde **dos**, el título suelto dejaba de servir: decía qué hace *uno* de ellos
sin decir cuál, que es justo la pregunta. Ahí la barra crece hacia arriba y cada
uno se lleva su fila, con su giro, su modelo y su propio reloj —el del reparto
diría que llevan un minuto los que acaban de salir de la cola—. Los que esperan
turno salen detrás, en gris y sin giro: pedir tres y ver dos parecía que uno se
había perdido, y el mensaje que lo explicaba se va con el scroll a los diez
segundos. Lo que no cabe se cuenta (`+2 más`).

El tiempo va en columna a la derecha y no detrás del título: con tres a la vez
lo que se busca de un vistazo es cuál se está eternizando, y eso solo se ve si
los números están alineados.

**Esas filas se las quita el panel del agente, nunca el shell.** El
pseudoterminal solo se redimensiona cuando cambia la ventana —`refit` únicamente
marca para repintar—, así que quitarle filas al shell al repartir pintaría un
terminal de un tamaño dentro de un hueco de otro: las últimas líneas de lo que
estés mirando, cortadas. Si al panel no le quedan al menos seis filas (sus dos
bordes, la caja y una de conversación), la lista no sale. Y la barra deja de
repetir el título del primero cuando la lista está: enseñarlo dos veces sería
enseñarlo sin decir de cuál es.

---

## 11. Ficheros e imágenes en la conversación

Arrastrar un fichero a una terminal no transfiere el fichero: transfiere su
**ruta**, en texto. `adjunto.rs` la reconoce, lee su contenido y lo antepone al
siguiente mensaje del usuario.

La regla que evita el problema evidente: **se adjunta solo si todo lo pegado son
rutas existentes**. Pegar un párrafo que mencione `/etc/passwd` no puede acabar
enviando ese fichero a un modelo, y una ruta relativa suelta tampoco cuenta, o
cualquier palabra que coincida con un fichero del directorio actual se
convertiría en adjunto por accidente.

**El pegado con corchetes es requisito.** Sin habilitarlo, `Event::Paste` no
llega nunca y todo el manejo de pegado es código muerto. No se detectaba porque
un pegado sin él llega igualmente, carácter a carácter, como si se tecleara; lo
que no puede hacerse entonces es distinguir «he pegado esto» de «he escrito
esto». Habilitarlo corrige además el pegado dentro de `vim`, que llegaba como
pulsaciones y se autoindentaba.

**El portapapeles se enlaza en el binario, y es la excepción al criterio
general.** Todo lo pesado del proyecto vive fuera —el modelo, whisper, piper— y
aquí se procedió al revés de forma consciente: pegar una captura es una acción
diaria, y condicionarla a que cada usuario instale `wl-clipboard` la convierte
en un trámite. El coste se midió antes de decidir: **653 KB**, casi todos del
protocolo Wayland y del cliente X11. `wl-paste` y `xclip` permanecen como
respaldo para sesiones sin escritorio accesible; por SSH no funciona ninguno,
y eso no tiene solución porque no hay portapapeles remoto que leer.

La imagen se busca **antes** que el texto. El orden es determinante: al copiar
una captura, muchos programas ofrecen además su nombre en texto, de modo que
examinar el texto primero acabaría pegando el nombre del fichero en lugar de la
imagen.

**Un campo nuevo en la configuración no puede dejar una función apagada en los
ficheros existentes.** El campo `imagenes` llegó después de que los ficheros
estuvieran escritos, así que nació desactivado en todos ellos: pegar una imagen
no funcionaba con ningún modelo y no había forma de averiguar por qué. Por eso
`ModeloToml::ve_imagenes` consulta además una lista de familias conocidas: lo
declarado manda y lo no declarado se deduce.

Que un modelo admita imágenes se declara (`imagenes = true`) y no se adivina:
`qwen3.6-27b` las interpreta y `gpt-oss-120b` no, y nada en sus nombres lo
indica. Los píxeles viajan por HTTP como **bloques de contenido** con la imagen
en un `data:` URI; sin imágenes el turno sigue siendo una cadena, que es lo que
entienden también los proveedores que no admiten bloques.

Las imágenes permanecen en el historial en lugar de consumirse en un turno: si
se retiraran, el modelo perdería de vista la captura en cuanto se le preguntara
una segunda cosa sobre ella. No se recomprimen ni se reescalan, porque reescalar
sería decidir por el modelo qué detalle puede permitirse ver. Por el daemon de
`claude` no se envían: su tubería JSON no admite imágenes sin reescribir el
protocolo, y por eso el aviso se da al pegarlas.

Verificarlo requiere un modelo real, y para eso existe `examples/imagen.rs`: que
el `data:` URI esté bien formado y que la petición no dé error no demuestra
nada, porque un modelo que no interpreta imágenes responde igualmente,
inventando el contenido.

---

## 12. Skills

Formato no propietario: una carpeta con un `SKILL.md`, un encabezado con `name`
y `description`, e instrucciones en prosa. El formato es el que usan Claude Code
y `agents`, así que cualquier skill de por ahí vale aquí tal cual.

**El formato se comparte; el armario no.** Se leen `~/.config/vsioncode/skills`
y `.vsioncode/skills` del proyecto —el segundo manda sobre el primero, porque
una skill del repositorio describe *este* código—, y ninguna otra. Antes se
leían también `~/.claude/skills` y `~/.agents/skills`, con el argumento de
aprovechar lo que el usuario ya tuviera instalado. El argumento era malo: son
dos programas distintos, y que instalar algo para Claude Code lo hiciera
aparecer aquí —sin haberlo puesto, escrito para otra herramienta y con otras
capacidades detrás— no es aprovechar, es heredar el armario de otro. Aquí se
instala lo que se quiere aquí.

Traer una de fuera es `/skills instalar <usuario/repo | URL | carpeta>`, y no
lo hace vsioncode: **arma el comando y lo deja en tu línea sin Enter**. Bajar
de la red y escribir en el disco son las dos cosas que este programa no hace a
tus espaldas, y lo que se está trayendo son instrucciones que después va a
seguir el modelo, así que el último paso es leerlas. El comando termina
listando la carpeta, que es la única señal de que llegó algo: las skills se
leen al arrancar y hasta el reinicio siguiente no se notan.

**Y si no sabes de dónde, lo busca él.** Lo que va detrás de `instalar` puede
ser una frase en vez de un sitio —`/skills instalar una de diseño de
terminales`—, y entonces el encargo va al agente: busca, dice qué ha
encontrado y devuelve el origen en una línea `SKILL-DE: usuario/repo`. Es el
mismo patrón de palabra exacta que `NECESITO-SKILL:`, y por el mismo motivo:
detectar la intención por parecido de texto falla en los dos sentidos.

Lo que **no** cambia es el final. El modelo dice de dónde; vsioncode arma el
comando y lo deja en tu línea sin pulsar. Dejarle instalar directamente sería
un agente bajándose de internet sus propias instrucciones futuras sin que nadie
las lea, que es exactamente la cadena que §6 existe para cortar, y sería
además la capacidad con «camino propio porque es distinta» de §15. Cuando el
sitio lo ha elegido él, el aviso lo dice: lo que viene de internet se lee
sabiendo que viene de internet.

Qué es un sitio y qué es una frase lo decide `skills::es_origen`, que pregunta
por lo que ya sabe leer el propio analizador de orígenes en vez de por un
patrón nuevo: dos formas de decidir lo mismo acabarían discrepando, y la
discrepancia sería salir a la red teniendo la ruta delante. Una sola palabra
que no se entiende tampoco sale a buscar — eso es una errata, y decirlo cuesta
menos que una llamada al modelo.

El mecanismo de activación es el mismo patrón de palabra exacta que el modo
libre: en el prompt de sistema viaja un **catálogo** —nombre y descripción
recortada, unos 2,3 KB con diez instaladas—, el modelo escribe
`NECESITO-SKILL: nombre` en una línea suelta y vsioncode le envía el documento
completo en el turno siguiente. Diez skills del formato habitual suman más de
130 KB de cuerpo: pagarlas en cada turno para usar una sola es lo que evita el
reparto en dos niveles. Sin ninguna instalada no se manda catálogo, y el
apartado desaparece del prompt en vez de anunciar una lista vacía.

Tres restricciones deliberadas:

- **No ejecuta nada.** Ni los `scripts/` que contenga ni los bloques de comandos
  de sus ejemplos. Si de leerla el modelo concluye que hay que ejecutar algo, lo
  propone como cualquier otra cosa y pasa por `safety.rs`.
- **No envía la misma skill dos veces.** Además del coste, un modelo que no
  recuerde tenerla podría pedirla en bucle; al no responder a la repetición, el
  bucle no puede cerrarse.
- **No incorpora `serde_yaml`.** El encabezado se analiza a mano —solo hay
  `clave: valor` y bloques con `|`— porque el peso del binario es una
  característica declarada.

---

## 13. Voz

Dos mitades en dos procesos: reconocimiento con **whisper `tiny`** vía `uv`
(`voz/escucha.py`) y síntesis con **piper** más `aplay`.

- **Silero VAD**, no detección por energía. El micrófono de la máquina de
  desarrollo tiene un suelo de ruido de 3388 con picos de 11873: ningún umbral
  fijo funciona.
- **Grupo de procesos propio** para todo lo lanzado con `uv run`: al terminar
  `uv` sobrevivía el proceso Python nieto **grabando el micrófono**. Es un fallo
  de privacidad, no de comodidad.
- **Número de generación** en cada daemon: sin él llegó a aparecer en pantalla
  un mensaje procedente de un oyente anterior.
- Se descartan los primeros 0,75 s: el transitorio de arranque de `arecord` se
  interpretaba como saturación y el control de ganancia reducía el micrófono
  hasta dejarlo inaudible.

Advertencia de diagnóstico ya pagada: **la CPU saturada imita síntomas de
hardware.** Con carga 10, el ruido de fondo medía 3268 y el pico 32768; con
carga 2, 1739 y 13737. Antes de tocar el audio, conviene mirar `uptime`.

---

## 14. Ficheros y procesos externos

| Ruta | Función |
|---|---|
| `~/.config/vsioncode/proveedores.toml` | Proveedores y modelo por defecto. |
| `~/.config/vsioncode/.env` | Claves de API, con permisos 600. |
| `~/.local/share/vsioncode/voz/` | Motores de voz. |
| `shell/vsion-init.bash` | Se copia a un `init.*.bash` temporal por sesión. |
| `voz/escucha.py`, `voz/microfono.py` | Oyente y sonda de micrófono. |
| `~/.config/vsioncode/skills`, `.vsioncode/skills` | Skills, propias y del proyecto. Solo lectura. |
| `tests/circuito.rs` | Pruebas de integración con un `bash` real. |
| `examples/` | Bancos de ensayo sin abrir la interfaz. |
| `instalar.sh` | Instalador: descarga el binario publicado o compila. |
| `.github/workflows/` | Integración continua y publicación por etiqueta. |
| `imagenes/` | Capturas de la interfaz para el README. |

Programas externos, todos detectados al arrancar y todos opcionales: `claude`,
`uv`, `piper`, `aplay`, `arecord`.

---

## 15. Puntos de vigilancia

- **El peso del binario.** Es una característica declarada y la integración
  continua lo comprueba. Medirlo, no suponerlo. El límite no es dogma: una
  mejora que lo necesita lo sube a propósito en el mismo cambio; lo que se
  vigila es el crecimiento silencioso, no el peso consciente.
- **La correa.** Cada capacidad nueva —cliente MCP, ejecución de Python, lo que
  venga— es una vía más por la que algo puede tocar el sistema, y debe pasar por
  el mismo filtro de riesgo. En cuanto una tenga un camino propio «porque es
  distinta», el modelo de seguridad deja de significar nada.
- **Los avisos improcedentes.** Véase §6.
- **El nombre.** «vsioncode» puede confundirse con «VS Code» al pronunciarlo.
  Publicado desde el 2026-08-31 en `github.com/jchacon1808/vsioncode`, así que
  cambiarlo ya cuesta: reetiquetar, romper la URL del instalador y dejar sin
  sitio a quien lo tenga instalado. Sigue libre en npm y crates.io.

---

## 16. Dónde se han detectado los fallos

Casi todos los fallos serios —todos menos dos— se detectaron **usando la
herramienta** o mediante las pruebas de integración. Ninguna prueba unitaria ni
revisión de código localizó un fallo grave.

No es un argumento contra las pruebas unitarias: sujetan correctamente el
markdown, el mapa de teclas, la clasificación de riesgo y el aplicador de
ediciones. Es un dato sobre **dónde vive el riesgo real** en este proyecto: en
la integración con procesos, terminales y configuración regional del sistema,
que es justo lo que una prueba unitaria no alcanza.
