# Ciclos, cuellos de botella y decisiones

Documento de ingeniería: dónde está el coste real de vsioncode, qué se decidió
en cada punto y qué se sacrificó. Escrito para que dentro de seis meses se
entienda **por qué** el código es así y no de la forma obvia.

---

## 1. El ciclo que da nombre al proyecto: la intervención humana

Es el único que importa de verdad. Todo lo demás existe para acortarlo.

**Antes** — seis saltos de contexto por iteración:

```
pienso el comando → lo escribo → falla → selecciono la salida con el ratón
→ la pego en el chat → leo → copio el comando propuesto → lo pego → Enter
```

**Ahora** — dos pulsaciones:

```
lo escribo → falla → Alt+E → leo la explicación → Alt+↵ → Enter
```

El coste no se mide en milisegundos sino en **cambios de foco atencional**. Cada
copiar-pegar rompe el hilo de pensamiento y obliga a reconstruirlo. Bajar de
seis a dos es la métrica del proyecto.

**Lo que se sacrifica:** el agente ve todo lo que ejecutas. Eso es un coste real
de privacidad, y por eso `Alt+W` pausa la captura y el estado se pinta
permanentemente en la barra — nunca como un aviso que se desvanece.

---

## 2. Ciclo de lectura del pseudoterminal

```
bash → PTY → read(16 KB) → escáner OSC → parser vt100 → canal → estado
```

**El cuello:** `cat fichero_grande` produce megas por segundo. Un `read()` puede
devolver 16 KB cada pocos microsegundos.

**Decisiones:**

| Punto | Elegido | Alternativa descartada | Por qué |
|---|---|---|---|
| Lectura | hilo dedicado con `read()` bloqueante | sondeo asíncrono | El planificador del sistema despierta el hilo exactamente cuando hay datos. Sondear gasta CPU para no enterarse antes. |
| Canal | `mpsc` sin límite | canal acotado | Un canal acotado bloquearía el hilo lector, que a su vez llenaría el buffer del PTY y **congelaría el programa que está escribiendo**. Se prefiere memoria a bloquear a bash. |
| Candado | `RwLock`, cogido y soltado dentro del bucle | mantenerlo durante el dibujado | Sostenerlo mientras se pinta es el camino directo a la interfaz congelada con salida masiva. Al dibujar se usa `try_read`: perder un fotograma no se nota, bloquear sí. |

**Lo que se sacrifica:** con una ráfaga muy grande, la cola del canal crece antes
de drenarse. Acotado en la práctica porque el consumidor vacía la cola entera
por fotograma.

---

## 3. Ciclo de repintado

**El cuello:** dibujar por cada lectura del PTY. Es el origen documentado del
100 % de CPU y del parpadeo en varias TUIs de agentes conocidas.

**Decisión:** el hilo lector **no dibuja**; solo levanta una bandera atómica. El
bucle principal dibuja como mucho cada 33 ms (≈30 fps) y **solo si** hay algo
nuevo. Además drena la cola completa antes de dibujar, para que mil mensajes
produzcan un fotograma, no mil.

```
1000 lecturas/s  ──►  bandera  ──►  30 fotogramas/s
```

El trabajo de pintar queda desacoplado del volumen de salida: `yes` y un
terminal en reposo cuestan lo mismo en dibujado.

**Lo que se sacrifica:** hasta 33 ms de latencia entre pulsar una tecla y verla.
Por debajo del umbral de percepción, y a cambio la CPU no se dispara.

---

## 4. Ciclo de captura: de bytes a evento

```
Piece::Bytes  ──►  ledger.feed_output()
Piece::Mark(CmdStart) ──► ledger.begin()
Piece::Mark(CmdEnd)   ──► ledger.end()
```

**El cuello, y el bug más sutil del proyecto:** el orden. Si en una misma
lectura de 16 KB llega `«hola\n»` seguido del marcador de fin, y se entregan los
bytes por un lado y los marcadores por otro, «hola» se procesa *después* de
cerrarse el evento y acaba en el comando siguiente — o se pierde.

Este fallo estuvo en el código: el comentario decía que se resolvía y la
implementación agrupaba bytes y marcadores por separado. Lo cazó el test de
extremo a extremo, no la revisión. De ahí sale `Piece`, que devuelve el flujo
troceado **en su posición real**, y el test
`los_bytes_salen_antes_del_marcador_que_les_seguia`, que lo fija para siempre.

**Memoria acotada por construcción.** `CappedBuffer` conserva 24 KB de cabeza y
40 KB de cola y descarta el centro. La cola es mayor a propósito: **los errores
viven al final**, el contexto vive al principio, y lo de en medio casi nunca
importa. Un `yes | head -c 1G` no mueve la aguja de memoria.

**Pausa en pantalla alterna.** Mientras `vim` o `htop` ocupan la pantalla, la
captura se detiene. Un `htop` emite un repintado completo por segundo: eso no es
información, es ruido que llenaría el registro y confundiría al modelo.

---

## 5. Ciclo del modelo

**El cuello medido, y no era el que parecía.** Lanzar `claude -p` por cada
pregunta cuesta entre 2 y 5 segundos **antes** de empezar a pensar:
descubrimiento de CLAUDE.md, MCP, plugins, LSP, keychain. El binario arranca en
0,44 s; el resto es preparación. La red no era el problema.

**Decisión:** un daemon. `claude` se arranca **una sola vez** en modo
`--input-format stream-json` y se le escriben turnos por stdin. Verificado
empíricamente: el proceso sobrevive entre turnos, conserva el mismo
`session_id` y recuerda lo dicho antes.

| | spawn por pregunta | daemon |
|---|---|---|
| Hasta el primer token | 4-9 s | ~1 s |
| Multi-turno | `--resume`, re-paga la caché | nativo |
| Memoria | un proceso por pregunta | uno fijo |

Y tres banderas que recortan el arranque a la mitad y el prompt de sistema de
~30 k a ~6,5 k tokens: `--setting-sources ""`, `--strict-mcp-config`,
`--disable-slash-commands`.

**`--tools ""` no es una optimización, es el invariante.** El modelo no tiene
herramientas: no puede tocar nada. Todo el diseño de seguridad descansa en eso,
así que se comprueba en el evento de arranque y se avisa si alguna vez llegara
con herramientas.

**Lo que se sacrifica:** el daemon ocupa memoria aunque no se use. A cambio,
`Modo observar` no le manda nada nunca y el coste en cuota es exactamente cero.

---

## 6. Ciclo de contexto: qué se le manda al modelo

**El cuello:** cada byte enviado cuesta latencia y cuota. Y el pensamiento
también cuesta: medido, para responder «Ok» el modelo gastó 98 de 105 tokens
pensando.

**Decisiones:**

- Presupuesto duro de 12 KB y 6 eventos por pregunta, llenado **del más
  reciente hacia atrás**: lo de hace un minuto importa más que lo de hace media
  hora.
- `--effort low` y el pensamiento filtrado del panel (solo anima el indicador).
- Redacción de secretos **antes** de enviar, no después.
- La invocación es siempre **a petición**. Nunca automática. Un fallo pinta la
  barra y ofrece `Alt+E`; llamar al modelo lo decides tú.

**Pendiente (v2):** medir el consumo real de la ventana leyendo el `usage` que
ya viene en el flujo —`input + cache_read + cache_creation + output`, con
`contextWindow` incluido— en vez de estimarlo. Es exacto y gratis. Para el
presupuesto *previo* al envío no sirve `bytes/4`: medido contra salidas reales
de terminal, se queda corto casi a la mitad en salida tabular (`ps aux`: −45 %),
y se equivoca en la dirección peligrosa.

---

## 7. Ciclo de ejecución: quién aprieta el gatillo

**El cuello no es técnico, es de confianza.**

La crítica constante a los agentes de código es la **fatiga de aprobación**:
preguntan tanto que la gente acaba pulsando «sí» por reflejo, o desactiva las
protecciones enteras. No se arregla con mejores diálogos: se arregla no
teniendo que aprobar nada.

**Decisión estructural:** el agente **no ejecuta**. Escribe el comando en tu
línea y ahí se queda. Aceptar (`Alt+↵`) y ejecutar (`Enter`) son dos decisiones
separadas, con el comando visible y editable entre medias.

**Y una decisión por encima de esa**, para el trabajo grande: con `/plan`, lo
que se aprueba no es un comando sino el trabajo entero, antes de que empiece.
El agente traza los pasos sin tocar nada —ni siquiera en modo libre, que ahí
las ediciones se quedan esperando— y el `Alt+↵` que lo pone en marcha es una
sola decisión sobre todo lo que va a pasar. Es el mismo principio movido al
sitio donde la decisión es barata: antes, y no veinte comandos después. Ver
[ARQUITECTURA §5.5](ARQUITECTURA.md#55-el-plan-mover-la-decisión-a-donde-es-barata).

Sobre la clasificación de riesgo hay que ser honesto: **las listas negras son
frágiles**. En un estudio sobre 1.709 listas reales de proyectos con agentes,
entre el 69 % y el 98,6 % no bloqueaban del todo lo que pretendían. Por eso
aquí la lista es un cinturón, no el freno, y por eso lo importante es la regla
de que **un comando compuesto hereda el nivel de su parte más peligrosa** —
`ls && rm -rf ~` es exactamente lo que se cuela por el filtro de «empieza por
ls» de casi todas.

---

## 8. Ciclo del teclado: el reparto imposible

**El cuello:** un shell real usa **todas** las teclas. `Ctrl+A` es inicio de
línea, `Ctrl+E` fin de línea, `Ctrl+R` busca en el historial, `Tab` completa,
`Esc` es Meta en readline y modo normal en vim.

Robar una es romper músculo de años. No robar ninguna es no tener interfaz.

**Decisión:** `Ctrl` entero para el shell, `Alt` para vsioncode. Readline ocupa
`M-b f d t u l c r y p n`; el resto está libre. Hay un test que recorre
`Ctrl+a` … `Ctrl+z` y falla si alguna vez interceptamos una.

Las teclas de función duplican los mismos atajos y se pintan en la barra,
porque el sentido del proyecto es que no haga falta saberse nada. Y cuando un
programa ocupa la pantalla —`htop` usa F1-F10— sus teclas vuelven a ser suyas;
los atajos con Alt siguen funcionando, que para eso se eligieron.

**Lo que se sacrifica:** en algunos terminales `Alt` requiere configurar la
tecla como Meta. Las teclas de función son la red de seguridad.

---

## 9. Convivir con el shell del usuario

**El cuello que casi hunde el proyecto y no estaba en ningún plan.**

`starship`, `oh-my-posh` y `powerlevel10k` **regeneran `PS1` en cada prompt**.
Unas marcas puestas una vez al arrancar se borran en el primer Enter y la
captura deja de funcionar **en silencio**: no hay error, simplemente el registro
se queda vacío para siempre.

Tres problemas encadenados, los tres reales y los tres encontrados ejecutando,
no leyendo:

1. **`PS1` regenerado.** Solución: re-aplicar las marcas en cada prompt y
   colocarse **el último** en `PROMPT_COMMAND`, después de quien lo reescriba.
2. **Ruido de los hooks del prompt.** El trap `DEBUG` salta también por las
   funciones que pintan el prompt: `starship_precmd` aparecía en el registro
   como si fuera un comando tuyo. Solución: una bandera que vale 1 mientras
   corre `PROMPT_COMMAND` — cubre a cualquier hook, se llame como se llame, sin
   tener que adivinar nombres.
3. **La coma decimal.** El más caro de encontrar, y el más tonto.
   `EPOCHREALTIME` usa el separador decimal **de la configuración regional**:
   en inglés es `1787272124.027953`, pero en español, francés o alemán es
   `1787272124,027953`. El código quitaba el punto y la coma sobrevivía. Y en
   aritmética de bash la coma es el **operador coma**: `$(( a , b ))` evalúa
   las dos partes y devuelve la segunda, así que no explotaba — devolvía la
   parte decimal. Salvo cuando esa parte empezaba por cero y contenía un 8 o
   un 9, que entonces bash la leía como octal y **la función moría antes de
   emitir el marcador de fin**.

   El síntoma: la captura se rompía de forma intermitente, dependiendo de los
   dígitos del reloj, en cualquier máquina con locale europeo. Sin ningún
   error visible: el comando se quedaba abierto para siempre.

   Dos lecciones, y las dos están en el código: borrar **todo lo que no sea
   un dígito** en vez de un separador concreto, y que el marcador de fin se
   emita **siempre**, pase lo que pase con la duración — que es un extra,
   porque vsioncode ya cronometra en Rust.

4. **Alias expandidos.** `$BASH_COMMAND` devuelve `ls --color=auto x` cuando tú
   escribiste `ls x`. Solución: leer el texto de `history 1`, que guarda la
   línea literal, comparando el número de historial para detectar si está
   rancio (con `HISTCONTROL=ignoredups`, el valor por defecto en Ubuntu,
   repetir un comando no añade entrada nueva) y caer a `$BASH_COMMAND` cuando
   lo esté.

---

## 10. Lo que queda abierto

- **Medición real de la ventana de contexto** a partir del `usage` que ya
  viene en el flujo del daemon (`input + cache_read + cache_creation +
  output`, con `contextWindow` incluido). Es exacto y no cuesta nada.
- **Servidor MCP** que exponga el registro a otros agentes. Es el hueco más
  claro que se encontró: lo existente (`tmux-mcp` y similares) hace captura de
  pantalla imperativa, y nadie expone `(comando, salida, código, cwd)` ya
  correlacionado.
