# Guía de desarrollo

Convenciones, procedimiento de verificación y trampas ya conocidas. El diseño y
las razones de cada decisión están en [`ARQUITECTURA.md`](ARQUITECTURA.md); esto
es cómo se trabaja sobre él.

## 1. Los dos invariantes

Todo lo demás es negociable; esto no.

**1. El modelo no accede al sistema.** El proceso principal se lanza con
`--tools ""` o, como máximo, `WebSearch,WebFetch`; los subagentes con
`Read,Grep,Glob`. Hay una prueba que falla si aparece `Bash`, `Write` o `Edit`.
Si una capacidad nueva necesita tocar algo del sistema, pasa por la misma
clasificación de riesgo que los comandos: nunca por una vía propia «porque es
distinta».

**2. Escribir un comando en la línea del usuario no lo ejecuta.** Ese es el
freno del diseño. `safety.rs` decide cuánta ceremonia se muestra antes de
pulsar, no si se permite pulsar.

## 2. Idioma y estilo

- **Todo en español**: código, comentarios, documentación y mensajes de commit.
  También los identificadores (`aplicar`, `cerebro`, `encargo`).
- Los comentarios explican **por qué**, no qué. Un comentario que describe lo que
  hace la línea siguiente sobra.
- Los mensajes de commit describen el problema y la decisión, no el diff.

## 3. Verificación antes de dar algo por terminado

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --release          # 428 unitarias + 10 de circuito + 2 de terminal (solo Unix)
cargo test --release --doc    # se olvida con facilidad y falla
shellcheck instalar.sh        # se ejecuta en la máquina de quien instala
ls -l target/release/vsioncode  # el peso es una característica declarada
```

Y si se ha tocado `pty.rs`, `rutas.rs`, `privado.rs`, `agente.rs` o cualquier
`#[cfg]`, también la otra rama — que no la ve nadie hasta que el CI la ve:

```bash
cargo clippy --target x86_64-pc-windows-gnu --all-targets -- -D warnings
```

Necesita `mingw-w64` y `rustup target add x86_64-pc-windows-gnu`. Windows no
está soportado (ver `RUTA.md` §3.6), pero el código de esa rama está escrito y
**un `cfg` que se apaga no falla: desaparece**, así que sin compilar los dos
lados se rompe uno de ellos sin enterarse.

Sobre los doctests: los ejemplos que contienen bloques con vallas de tres
acentos graves necesitan una valla exterior de **cuatro**, o rustdoc intenta
compilarlos como Rust.

Al probar algo que llama a un modelo, usar **haiku**
(`--model claude-haiku-4-5-20251001`): la cuota es la misma que la del trabajo
real y estas pruebas validan el cableado, no el razonamiento.

## 4. Dónde está el riesgo real

Casi todos los fallos serios del proyecto aparecieron **usando la herramienta** o
en las pruebas de integración, no en las unitarias ni en las revisiones de
código. El riesgo vive en la integración con procesos, terminales y
configuración regional del sistema.

Si se modifica `pty.rs`, `marker.rs`, `voz.rs`, `shell/vsion-init.bash` o el
lanzamiento de procesos hijos, hay que **probarlo abriendo vsioncode**, no solo
con `cargo test`.

`tests/terminal.rs` arranca el binario bajo un pseudoterminal y comprueba que
cada modo del emulador que se enciende (`CSI ? n h`) se apaga al salir
(`CSI ? n l`). Al añadir un modo nuevo no hace falta tocar la prueba: la regla
es la simetría, no una lista.

Casos ya pagados, documentados en [`ARQUITECTURA.md`](ARQUITECTURA.md) §4 y §12:

| Trampa | Síntoma |
|---|---|
| `starship` regenerando `PS1` en cada prompt | la captura deja de funcionar en silencio |
| La coma decimal de `EPOCHREALTIME` según el locale | duraciones absurdas y pérdida del marcador de fin |
| El grupo de procesos de `uv run` | procesos huérfanos tras cerrar la voz |
| Números de generación de los daemons | respuestas de una sesión anterior apareciendo en pantalla |

## 5. Bancos de ensayo

Hay comportamientos que solo se comprueban con una red, un modelo o unos ojos
delante. Para eso están los ejemplos, y por eso forman parte del proyecto:

| Ejemplo | Qué comprueba |
|---|---|
| `--example vista` | el aspecto de la conversación, sin abrir una terminal |
| `--example modelos` | qué modelos sirve realmente cada cuenta |
| `--example latencia` | tiempo hasta la primera palabra de cada modelo |
| `--example imagen` | si el modelo interpreta la imagen; codificarla bien no basta |
| `--example skills` | qué skills hay instaladas en esta máquina |
| `--example mando` | si el modelo cede el mando cuando corresponde |
| `--example plan` | si el modelo traza el plan en su bloque y marca los pasos |
| `--example delegar` | qué cuesta y qué trae un subagente, por modelo |
| `--example proveedor` | un proveedor de extremo a extremo |
| `--example sonda` | el micrófono |

## 6. Criterio para los avisos

Un aviso que salta cuando no corresponde se aprende a ignorar, y entonces
tampoco avisa cuando importa. Ya ocurrió dos veces —con `sudo` y con la defensa
frente a inyección desde páginas web—, y en ambos casos la solución fue reducir
el alcance del aviso, no ampliarlo.

**Precisión antes que amplitud.**

## 7. Criterio para el menú de comandos

El menú de `/` muestra unas pocas filas: cada entrada que sobra entierra una que
importa. Antes de añadir un comando a la tabla `COMANDOS` de `src/slash.rs`:

- ¿Tiene ya un atajo de teclado?
- ¿Se usa una vez y nunca más?

Si la respuesta a cualquiera de las dos es sí, que `parse` lo reconozca pero que
viva en `OCULTOS`: sigue funcionando escrito y aparece en `/ayuda`, sin ocupar
sitio en el menú.

## 8. Integración continua

El flujo de trabajo de `.github/workflows/ci.yml` ejecuta formato, clippy con
avisos como errores, las pruebas, los doctests, `shellcheck` sobre los dos
scripts y una comprobación del tamaño del binario con un límite de 6 MB. El
límite no es dogma: si una mejora legítima necesita más peso, se sube a
propósito en el mismo cambio; lo que vigila es el crecimiento silencioso.

`shell/vsion-init.bash` se analiza excluyendo SC2127, SC2128 y SC2178: usa
`${ ...; }` de bash 5.3, que shellcheck 0.10 todavía no reconoce, y elige entre
array y cadena según la versión de bash, lo que le hace creer que se mezclan los
tipos. Las tres son falsos positivos comprobados; cualquier otro aviso es real.

## 9. Publicar una versión

Una etiqueta `vX.Y.Z` dispara `.github/workflows/release.yml`, que compila para
`x86_64` y `aarch64`, pasa las pruebas, empaqueta, calcula los hashes y crea la
publicación en GitHub.

```bash
# Antes: subir la versión en Cargo.toml. El flujo comprueba que la etiqueta y
# el manifiesto dicen lo mismo, y falla si no. Publicar un paquete llamado
# v0.2.0 cuyo `--version` responde 0.1.0 no se descubre hasta que alguien lo
# instala.
git tag v0.2.0 && git push origin v0.2.0
```

Se compila sobre **Ubuntu 22.04** y no sobre `ubuntu-latest`: el binario queda
enlazado contra la glibc de la máquina que lo compila, y con la más reciente no
arrancaría en ninguna distribución con dos años encima. 22.04 trae glibc 2.35,
que cubre de Debian 12 y Ubuntu 22.04 en adelante.

`workflow_dispatch` permite ensayar la construcción sin etiquetar: sube los
paquetes como artefactos y no toca la página de publicaciones.
