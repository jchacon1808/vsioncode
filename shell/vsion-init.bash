# vsion-init.bash — instrumentación del shell para vsioncode
#
# Se carga en el bash que vsioncode arranca dentro del pseudoterminal, después
# del `.bashrc` del usuario. Emite marcadores OSC que vsioncode intercepta y
# ELIMINA del flujo antes de pintar, así que no se ven nunca. Ejecutado en una
# terminal normal tampoco molesta: son secuencias que cualquier terminal ignora.
#
# Marcadores emitidos
#   ESC ] 133 ; A  BEL                  inicio del prompt   (estándar OSC 133)
#   ESC ] 133 ; B  BEL                  fin del prompt
#   ESC ] 7331 ; cwd ; <b64> BEL        directorio actual
#   ESC ] 7331 ; cmd ; <b64> BEL        empieza un comando (payload: la línea)
#   ESC ] 7331 ; end ; <ec> ; <us> BEL  termina (código de salida y duración)
#
# El payload va en base64 para que un comando con `;`, saltos de línea,
# comillas o UTF-8 no rompa el parseo. Es la diferencia entre funcionar en la
# demo y funcionar con un heredoc.
#
# ## Por qué esto es más largo de lo que parece que debería
#
# Porque hay que convivir con el prompt del usuario. `starship`, `oh-my-posh`,
# `powerlevel10k` y compañía **regeneran PS1 en cada prompt**, así que unas
# marcas puestas una sola vez al arrancar se borran en el primer Enter y la
# captura deja de funcionar en silencio. La solución es re-aplicarlas en cada
# prompt y colocarnos los **últimos** en PROMPT_COMMAND, para pasar después de
# quien lo reescriba.

[[ -n "${__VSION_LOADED:-}" ]] && return 0
__VSION_LOADED=1

__vsion_in_cmd=0
__vsion_start=""
__vsion_status=0
__vsion_in_prompt=0
__vsion_last_hist=""

# Las marcas de prompt, ya envueltas en \[ \]. Ese envoltorio es obligatorio:
# le dice a readline que estos bytes no ocupan ancho. Sin él, el borrado de
# línea se descuadra en cuanto escribes más de una pantalla.
__vsion_pre=$'\001\033]133;A\a\002'
__vsion_post=$'\001\033]133;B\a\002'
__vsion_needle=$'\033]133;A'

__vsion_b64() { printf '%s' "$1" | base64 | tr -d '\n'; }

# Reloj en microsegundos, como entero.
#
# CUIDADO CON LA CONFIGURACIÓN REGIONAL. `EPOCHREALTIME` usa el separador
# decimal del locale: en `en_US` es «1787272124.027953», pero en `es_ES`,
# `fr_FR` o `de_DE` es «1787272124,027953» — con coma.
#
# Quitar solo el punto deja la coma, y en aritmética de bash la coma es el
# **operador coma**: `$(( a , b ))` evalúa las dos partes y devuelve la
# segunda. Así que el cálculo no explota, simplemente devuelve la parte
# decimal... salvo cuando empieza por cero y contiene un 8 o un 9, que
# entonces sí falla porque bash la lee como octal.
#
# El resultado era un fallo intermitente que dependía de los dígitos del
# reloj: unas veces daba una duración absurda y otras rompía la función
# entera, dejando de emitir el marcador de fin y matando la captura en
# silencio. Se arregla borrando todo lo que no sea un dígito.
# Deja el resultado en `__vsion_now` en vez de imprimirlo: devolverlo por
# salida estándar obligaría a un `$( )`, y eso es un fork en cada comando que
# escribes. Aquí no se forka nada.
__vsion_now=0
__vsion_ahora() { local t=${EPOCHREALTIME:-0}; __vsion_now=${t//[!0-9]/}; }

# ¿Tenemos sustitución de función sin fork? (bash 5.3+)
__vsion_funsub=0
if [[ "${BASH_VERSINFO[0]}" -gt 5 ]] 2>/dev/null; then
    __vsion_funsub=1
elif [[ "${BASH_VERSINFO[0]}" -eq 5 && "${BASH_VERSINFO[1]}" -ge 3 ]] 2>/dev/null; then
    __vsion_funsub=1
fi

# --- se dispara justo antes de ejecutar cada comando -----------------------
#
# El trap DEBUG es ruidoso: salta por CADA comando simple, también dentro de
# pipelines, dentro de funciones y —esto sorprende— por las funciones que el
# propio shell ejecuta para pintar el prompt. Sin filtrar, el registro se
# llena de `starship_precmd` y de la segunda mitad de cada `a | b`.
#
# Dos filtros lo resuelven, y ninguno depende de adivinar nombres:
#
#  1. `__vsion_in_prompt`, que vale 1 mientras corre PROMPT_COMMAND. Nos
#     ponemos el primero y el último de esa lista, así que cubre a cualquier
#     hook de prompt que haya en medio, se llame como se llame.
#  2. `__vsion_in_cmd`, que hace que solo cuente el primer disparo tras el
#     prompt: el que el usuario escribió de verdad.
__vsion_preexec() {
    (( __vsion_in_prompt )) && return            # es un hook del prompt, no tuyo
    [[ -n "${COMP_LINE:-}" ]] && return          # completado con Tab
    [[ "$BASH_COMMAND" == __vsion_* ]] && return # nuestras propias funciones
    (( __vsion_in_cmd )) && return               # ya estamos dentro de un comando

    __vsion_in_cmd=1
    __vsion_ahora
    __vsion_start=$__vsion_now

    # De dónde sale el texto del comando.
    #
    # `$BASH_COMMAND` trae los alias YA EXPANDIDOS: si escribes `ls x` y tienes
    # `alias ls='ls --color=auto'`, ahí llega `ls --color=auto x`. Eso no es lo
    # que escribiste, y verlo en pantalla desconcierta.
    #
    # El historial sí guarda la línea literal. Pero puede estar rancio: con
    # `HISTCONTROL=ignoredups` (el valor por defecto en Ubuntu) repetir un
    # comando no añade una entrada nueva. Por eso comparamos el NÚMERO de
    # historial: si cambió, el texto es fresco y lo usamos; si no, caemos a
    # `$BASH_COMMAND`, que será menos fiel pero nunca es de otro comando.
    local linea num texto
    # `${ ...; }` (bash 5.3+) captura la salida SIN forkar un subshell. Con
    # `$( ... )` pagaríamos un fork por cada comando que escribes, dentro del
    # trap: es el camino más corto a que vsioncode se note lento.
    if (( __vsion_funsub )); then
        linea=${ builtin history 1 2>/dev/null; }
    else
        linea=$(builtin history 1 2>/dev/null)
    fi
    linea="${linea#"${linea%%[![:space:]]*}"}"   # quita espacios por delante
    num="${linea%%[[:space:]]*}"
    texto="${linea#*[[:space:]]}"
    texto="${texto#"${texto%%[![:space:]]*}"}"

    if [[ -n "$num" && "$num" != "$__vsion_last_hist" && -n "$texto" ]]; then
        __vsion_last_hist="$num"
    else
        texto="$BASH_COMMAND"
    fi

    printf '\033]7331;cmd;%s\a' "$(__vsion_b64 "$texto")"
}

# --- primero en PROMPT_COMMAND: capturar $? antes de que nadie lo pise -----
__vsion_capture() {
    __vsion_status=$?
    __vsion_in_prompt=1
    return "$__vsion_status"
}

# --- último en PROMPT_COMMAND: emitir y re-aplicar las marcas --------------
__vsion_precmd() {
    if (( __vsion_in_cmd )); then
        # La duración es un extra: vsioncode ya cronometra en Rust. Si algo
        # sale mal aquí se manda 0, pero el marcador de fin se emite SIEMPRE.
        # Perderlo deja el comando abierto para siempre y la captura muerta.
        local us=0
        if [[ -n "$__vsion_start" ]]; then
            __vsion_ahora
            us=$(( __vsion_now - __vsion_start )) 2>/dev/null || us=0
            (( us < 0 )) && us=0
        fi
        printf '\033]7331;end;%s;%s\a' "$__vsion_status" "$us"
        __vsion_in_cmd=0
        __vsion_start=""
    fi
    printf '\033]7331;cwd;%s\a' "$(__vsion_b64 "$PWD")"

    # Re-envolver el prompt si alguien lo ha regenerado sin nuestras marcas.
    # Este `if` es lo que hace que vsioncode siga funcionando con starship.
    if [[ "$PS1" != *"$__vsion_needle"* ]]; then
        PS1="${__vsion_pre}${PS1}${__vsion_post}"
    fi

    # A partir de aquí ya no estamos pintando el prompt: el siguiente disparo
    # del trap DEBUG sí será un comando del usuario.
    __vsion_in_prompt=0
    return "$__vsion_status"
}

# Nos ponemos el primero (para `$?`) y el último (para el prompt). Quien haya
# entre medias puede hacer lo que quiera con PS1: lo arreglamos después.
if [[ "${BASH_VERSINFO[0]}${BASH_VERSINFO[1]}" -ge 51 ]] 2>/dev/null; then
    # bash 5.1+ admite PROMPT_COMMAND como array, que conserva el del usuario
    # sin tener que concatenar cadenas y rezar.
    if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
        PROMPT_COMMAND=(__vsion_capture "${PROMPT_COMMAND[@]}" __vsion_precmd)
    elif [[ -n "${PROMPT_COMMAND:-}" ]]; then
        PROMPT_COMMAND=(__vsion_capture "$PROMPT_COMMAND" __vsion_precmd)
    else
        PROMPT_COMMAND=(__vsion_capture __vsion_precmd)
    fi
else
    PROMPT_COMMAND="__vsion_capture${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __vsion_precmd"
fi

# Y las marcas ya para el primer prompt, sin esperar al segundo.
if [[ "$PS1" != *"$__vsion_needle"* ]]; then
    PS1="${__vsion_pre}${PS1:-\u@\h:\w\$ }${__vsion_post}"
fi

export VSIONCODE_SHELL=1

# El trap va al final a propósito: registrado antes, se dispararía con las
# propias líneas de este fichero y ensuciaría el registro con ruido de arranque.
trap '__vsion_preexec' DEBUG
