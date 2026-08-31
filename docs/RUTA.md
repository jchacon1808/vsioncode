# Hoja de ruta

Qué está hecho, qué falta y en qué orden. El diseño actual está en
[`ARQUITECTURA.md`](ARQUITECTURA.md); esto es el plan.

**Objetivo:** un entorno de trabajo en terminal que funcione con cualquier
modelo —incluidos los gratuitos— sin dejar de ser un binario pequeño y un
proceso ligero.

---

## 1. Estado actual

| | |
|---|---|
| Lenguaje | Rust, edición 2024 (mínimo 1.90) |
| Sistema | Linux, `x86_64` y `aarch64`; en Windows, dentro de WSL2 |
| Tamaño | ~28.000 líneas propias; binario de 5,4 MB |
| Pruebas | 415 unitarias, 12 de integración, clippy sin avisos |
| Interfaz | `ratatui` 0.30 + `crossterm` 0.29 |
| Terminal interior | `portable-pty` + `vt100` + `tui-term` |
| Concurrencia | `tokio` (multihilo, procesos, canales) |
| Modelo | proceso `claude` por tuberías, o HTTP de dialecto OpenAI |
| Voz | Python vía `uv`: whisper `tiny` + Silero VAD; `piper` para sintetizar |

El binario creció de 1,8 a 4,4 MB al incorporar el cliente HTTP con TLS —lo que
abre el proyecto a cualquier proveedor— y a 5,4 MB al integrar el portapapeles.
Ambos incrementos se midieron antes de decidir, y se recortaron 2 MB eligiendo
`ring` en lugar del proveedor criptográfico por defecto de `rustls`.

Comparación con la alternativa de referencia, medida en la máquina de
desarrollo:

| | binario | memoria en reposo | arranque |
|---|---|---|---|
| **vsioncode** | **5,4 MB** | **6 MB** | **< 15 ms** |
| Claude Code 2.1.238 | 324 MB | 774 MB | 855 ms |

La diferencia es de reparto de trabajo, no de virtuosismo: los motores pesados
son procesos externos, no bibliotecas enlazadas.

---

## 2. Completado

| | Entrega |
|---|---|
| ✅ | Shell real bajo pseudoterminal, con registro compartido y captura por marcadores OSC |
| ✅ | Cuatro modos de operación, modo libre y cesión de mando comando a comando |
| ✅ | Interfaz del modelo como enum: proceso `claude` y backend HTTP |
| ✅ | Multiproveedor por TOML, `/modelo` en caliente y control del razonamiento |
| ✅ | Subagentes de solo lectura |
| ✅ | Edición de ficheros con aplicador tolerante |
| ✅ | Skills leídas del formato `SKILL.md` ya instalado |
| ✅ | Adjuntos e imágenes, con portapapeles integrado |
| ✅ | Modo de voz local, continuo y bidireccional |
| ✅ | Tema por función semántica y `NO_COLOR` cumplido |
| ✅ | Configuración inicial guiada, reabrible y no destructiva |
| ✅ | Doble licencia MIT/Apache-2.0 e integración continua con límite de tamaño |
| ✅ | Instalación en una orden: binarios publicados por arquitectura, con hash |

---

## 3. Pendiente, por orden

### 3.1 Sesiones reanudables

Guardar el `session_id` del proceso del modelo para poder retomar una
conversación tras cerrar. Hoy cada arranque empieza de cero, incluso cuando el
trabajo es el mismo del día anterior.

### 3.2 Vista de registro navegable

El registro existe y solo se consulta con `/registro`, que vuelca texto.
Recorrerlo, filtrar por fallo y reenviar un evento concreto al modelo aprovecha
una estructura de datos que ya está construida.

### 3.3 Servidor MCP del registro

**El diferenciador del proyecto.** Existen `tmux-mcp`, `mcp-histfile` y otros,
pero todos leen historial estático o exigen ser ellos quienes ejecuten los
comandos. Ninguno expone un registro **observado pasivamente y ya
correlacionado**: comando, salida real, código de salida, directorio y
duración, con ANSI limpio y secretos redactados.

```
ledger://events/last            el último comando, completo
ledger://events/last-failure    el último que falló
ledger://cwd                    directorio actual
```

Recursos y no herramientas: es estado que cambia solo, y así otro agente puede
suscribirse y recibir el siguiente comando sin preguntar. Con una única
herramienta, `ledger_context(n, presupuesto)`, que ya está escrita.

**De solo lectura, sin excepciones.** Exponer «ejecuta este comando» por MCP
rompería lo único que el proyecto garantiza.

### 3.4 Análisis con Python

Sin intérprete embebido: `rustpython` está incompleto y `pyo3` ataría el binario
a un Python del sistema. El precedente correcto ya está en el proyecto —la voz
corre por `uv run` sin compilar nada—. Un bloque ` ```python ` pasaría por **la
misma clasificación de riesgo que un comando de shell**, se ejecutaría con
`uv run --with plotext,rich` y su salida se pintaría con el mismo analizador
vt100 que usa el panel del shell.

Un script Python toca el sistema igual que un comando: no puede haber un camino
privilegiado porque se llame «análisis».

### 3.5 Distribución en los gestores de paquetes

Instalar ya es una orden: `instalar.sh` descarga el binario de la arquitectura,
comprueba su hash y lo deja en el `PATH`, y una etiqueta `vX.Y.Z` dispara la
compilación para `x86_64` y `aarch64`. Lo que falta es que no haga falta ni esa
orden: paquete AUR, `.deb` y `.rpm`, y `cargo install vsioncode` a secas —lo
que exige publicar en crates.io y, antes, decidir el nombre (ver §5).

`release-plz` para versionar a partir de los mensajes de commit encaja bien con
la convención del proyecto, donde cada commit cuenta una decisión.

### 3.6 Windows nativo

Hoy funciona por WSL2 y no hay versión nativa. El camino está decidido:
**Git Bash, no PowerShell**. PowerShell no tiene `trap DEBUG` ni un código de
salida único —`$?` es un booleano—, así que no hay captura posible; un bash de
verdad deja intacto `shell/vsion-init.bash`, que ya lleva guardas por versión,
y salva las 1.632 líneas de `safety.rs`, que clasifican comandos POSIX.

El trabajo está **escrito y compilando** desde el 31 de agosto de 2026: el
shell busca Git Bash donde el instalador lo deja, las rutas se traducen en las
cuatro fronteras por donde cruzan, el `.env` se cierra con una ACL en vez de
con tres bits de modo, y el clasificador dice cuándo un comando se sale de
POSIX. Un runner `windows-latest` lo comprueba en cada cambio, `tests/circuito.rs`
incluido — que es el que arranca un bash de verdad bajo ConPTY.

**No está soportado, y no se anunciará hasta que alguien lo haya abierto.** El
puerto toca el pseudoterminal, el lanzamiento de procesos y las rutas —las tres
zonas donde este proyecto ya sabe que viven sus fallos—, y ahí `cargo test` no
alcanza: si `vim` se pinta bien dentro del panel, si `Ctrl+C` corta lo que tiene
que cortar, si al salir el terminal queda como estaba. Un repositorio que
promete Windows y se cuelga al arrancar hace más daño que un «solo Linux» dicho
con claridad.

### 3.7 Documentación en inglés

El contenido es bueno y el idioma limita el alcance en un repositorio público.
La traducción va después de la distribución, no antes: primero conviene que haya
algo instalable.

---

## 4. Descartado, con motivo

| Propuesta | Por qué no |
|---|---|
| Bloques plegables tipo Warp | Exigen tratar el shell como una secuencia de comandos discretos, lo que contradice el pilar del proyecto: es un `bash` real y continuo. |
| Coloreado propio de diffs | Cuando se ejecuta `git diff`, git ya colorea. |
| Imágenes reales (kitty/sixel) | Coste alto para un caso de uso que `plotext` cubre en texto. |
| LSP integrado | `rust-analyzer` vive en el editor; integrarlo aquí es discutir la premisa. |

---

## 5. Riesgos permanentes

- **El peso.** Cada dependencia nueva se mide antes de aceptarse; la integración
  continua falla por encima de 6 MB.
- **La correa.** Cada capacidad nueva —cliente MCP, Python, lo que venga— es una
  vía más por la que algo puede tocar el sistema, y debe pasar por el mismo
  filtro de riesgo. En cuanto una tenga camino propio, el modelo de seguridad
  deja de significar nada.
- **Los avisos improcedentes.** Ya ha ocurrido dos veces. Precisión antes que
  amplitud.
- **Lo invisible no existe.** Una capacidad que la interfaz no muestra no existe
  para quien la usa; se descubre abriendo vsioncode, no con `cargo test`.
- **El nombre.** «vsioncode» puede confundirse con «VS Code». Desde el
  2026-08-31 el repositorio público es `jchacon1808/vsioncode`, así que el
  cambio ya no es gratis: habría que reetiquetar, romper la URL del instalador
  y dejar sin sitio a quien ya lo tenga. Sigue libre en npm y crates.io, y ahí
  es donde todavía se decide barato.
