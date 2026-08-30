#!/usr/bin/env bash
#
# Instalador de vsioncode.
#
#   curl -fsSL https://raw.githubusercontent.com/jchacon1808/vsioncode/main/instalar.sh | bash
#
# Descarga el binario publicado para esta arquitectura, comprueba su hash y lo
# deja en el PATH. Si no hay binario para este equipo —o si se pide con
# «--fuente»— compila desde el código.
#
# No pregunta nada: se ejecuta a través de una tubería y ahí no hay a quién
# preguntar. Todo lo que se puede decidir se decide con opciones.

set -euo pipefail

REPO="jchacon1808/vsioncode"
PROGRAMA="vsioncode"

# Ajustables por línea de órdenes.
VERSION=""
PREFIJO=""
FORZAR_FUENTE=0
DESINSTALAR=0

# Por qué no sirvió el binario publicado, para poder decirlo al caer a la
# compilación. Sin esto todos los caminos dan el mismo mensaje y ninguno sirve
# para saber qué pasó.
MOTIVO=""

# ─── Cómo se habla con quien mira ────────────────────────────────────────────
#
# A stderr y no a stdout: quien canalice la salida del instalador quiere el
# resultado, no la narración.

if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
    C_TITULO=$'\033[1m'; C_BIEN=$'\033[32m'; C_AVISO=$'\033[33m'
    C_MAL=$'\033[31m';   C_TENUE=$'\033[2m'; C_FIN=$'\033[0m'
else
    C_TITULO=""; C_BIEN=""; C_AVISO=""; C_MAL=""; C_TENUE=""; C_FIN=""
fi

paso()  { printf '%s\n' "${C_TITULO}==>${C_FIN} $*" >&2; }
bien()  { printf '%s\n' "  ${C_BIEN}✓${C_FIN} $*" >&2; }
aviso() { printf '%s\n' "  ${C_AVISO}!${C_FIN} $*" >&2; }
nota()  { printf '%s\n' "  ${C_TENUE}$*${C_FIN}" >&2; }
morir() { printf '%s\n' "${C_MAL}vsioncode:${C_FIN} $*" >&2; exit 1; }

ayuda() {
    cat >&2 <<'AYUDA'
Instalador de vsioncode.

USO
    instalar.sh [opciones]

OPCIONES
    --version <vX.Y.Z>  versión a instalar (por defecto, la última publicada)
    --prefijo <dir>     dónde dejar el ejecutable
                        (por defecto ~/.local/bin, o /usr/local/bin como root)
    --fuente            compilar desde el código en vez de descargar el binario
    --desinstalar       quitar el ejecutable instalado
    -h, --help          esta ayuda

EJEMPLOS
    curl -fsSL https://raw.githubusercontent.com/jchacon1808/vsioncode/main/instalar.sh | bash
    ./instalar.sh --prefijo /usr/local/bin
    ./instalar.sh --fuente
AYUDA
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)     VERSION="${2:-}"; shift 2 || morir "«--version» necesita un valor." ;;
        --prefijo)     PREFIJO="${2:-}"; shift 2 || morir "«--prefijo» necesita un valor." ;;
        --fuente)      FORZAR_FUENTE=1; shift ;;
        --desinstalar) DESINSTALAR=1; shift ;;
        -h|--help)     ayuda; exit 0 ;;
        *)             morir "opción desconocida «$1». Prueba --help." ;;
    esac
done

# ─── Dónde acaba el ejecutable ───────────────────────────────────────────────
#
# Como root, en /usr/local/bin: es donde se busca lo instalado a mano y ya
# está en el PATH de todos. Como usuario normal, ~/.local/bin, que es lo que
# manda el estándar de directorios de freedesktop y no necesita permisos.

if [ -z "$PREFIJO" ]; then
    if [ "$(id -u)" -eq 0 ]; then PREFIJO="/usr/local/bin"; else PREFIJO="$HOME/.local/bin"; fi
fi

hay() { command -v "$1" >/dev/null 2>&1; }

if [ "$DESINSTALAR" -eq 1 ]; then
    destino="$PREFIJO/$PROGRAMA"
    if [ -e "$destino" ]; then
        rm -f "$destino" || morir "no se pudo borrar $destino"
        bien "borrado $destino"
    else
        aviso "no había nada en $destino"
    fi
    nota "La configuración sigue en ~/.config/vsioncode; bórrala a mano si quieres."
    exit 0
fi

# ─── Que este equipo sirva ───────────────────────────────────────────────────
#
# Linux y no «cualquier Unix»: vsioncode lee /proc para saber si un shell sigue
# vivo, y la voz usa ALSA. Decirlo aquí, y no a mitad de la compilación, es la
# diferencia entre un mensaje y diez minutos perdidos.

paso "Comprobando el equipo"

[ "$(uname -s)" = "Linux" ] || morir \
"vsioncode solo funciona en Linux (este equipo dice «$(uname -s)»).
       Depende de /proc y de un pseudoterminal de tipo Unix. En Windows,
       funciona dentro de WSL2."

case "$(uname -m)" in
    x86_64|amd64)  ARCO="x86_64" ;;
    aarch64|arm64) ARCO="aarch64" ;;
    *)
        aviso "arquitectura «$(uname -m)»: no hay binario publicado, se compilará"
        ARCO=""
        FORZAR_FUENTE=1
        ;;
esac
[ -n "$ARCO" ] && bien "Linux $ARCO"

hay bash || morir "hace falta «bash»: vsioncode ejecuta un bash de verdad por dentro."
bien "bash $(bash --version | head -1 | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -1)"

# ─── Compilar desde el código ────────────────────────────────────────────────

compilar_desde_fuente() {
    paso "Compilando desde el código"

    hay cargo || morir \
"hace falta Rust 1.90 o superior para compilar. Se instala con:
           curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
       Y después vuelve a lanzar este instalador."

    hay cc || aviso "no se ve un enlazador («cc»); si la compilación falla, instala build-essential"

    # A un directorio propio y luego se copia. `cargo install --root X` deja el
    # ejecutable en X/bin, que solo coincide con el destino pedido cuando este
    # termina en «/bin»; copiarlo a mano hace que «--prefijo» signifique lo
    # mismo por las dos vías.
    local raiz_tmp; raiz_tmp="$(mktemp -d)"
    trap 'rm -rf "$raiz_tmp"' RETURN

    # Desde una copia del repositorio se compila lo que hay delante; si no,
    # cargo se trae el código él solo y no hace falta clonar nada.
    local aqui="${BASH_SOURCE[0]:-}"
    if [ -n "$aqui" ] && [ -f "$(dirname "$aqui")/Cargo.toml" ]; then
        local raiz; raiz="$(cd "$(dirname "$aqui")" && pwd)"
        nota "código local: $raiz"
        cargo install --path "$raiz" --locked --root "$raiz_tmp" --force
    else
        nota "código de https://github.com/$REPO"
        local ref=(--branch main)
        [ -n "$VERSION" ] && ref=(--tag "$VERSION")
        cargo install --git "https://github.com/$REPO" "${ref[@]}" --locked \
            --root "$raiz_tmp" --force
    fi

    mkdir -p "$PREFIJO"
    install -m 755 "$raiz_tmp/bin/$PROGRAMA" "$PREFIJO/$PROGRAMA"
}

# ─── Descargar el binario publicado ──────────────────────────────────────────

descargar() {
    # `curl` y `wget` cubren entre los dos cualquier instalación mínima.
    if   hay curl; then curl -fsSL "$1" -o "$2"
    elif hay wget; then wget -qO "$2" "$1"
    else return 1
    fi
}

ultima_version() {
    local api="https://api.github.com/repos/$REPO/releases/latest" tmp
    tmp="$(mktemp)"
    descargar "$api" "$tmp" 2>/dev/null || { rm -f "$tmp"; return 1; }
    # Sin `jq`: es una dependencia más para leer un campo de una línea.
    grep -m1 '"tag_name"' "$tmp" | sed -E 's/.*"tag_name" *: *"([^"]+)".*/\1/'
    rm -f "$tmp"
}

instalar_binario() {
    paso "Buscando la versión publicada"

    if [ -z "$VERSION" ]; then
        VERSION="$(ultima_version || true)"
        if [ -z "$VERSION" ]; then
            MOTIVO="todavía no hay ninguna versión publicada"
            return 1
        fi
    fi
    bien "$VERSION"

    local nombre="$PROGRAMA-$VERSION-$ARCO-unknown-linux-gnu.tar.gz"
    local base="https://github.com/$REPO/releases/download/$VERSION"
    local tmp; tmp="$(mktemp -d)"
    # Que no quede un directorio en /tmp si algo falla a mitad.
    trap 'rm -rf "$tmp"' RETURN

    paso "Descargando $nombre"
    if ! descargar "$base/$nombre" "$tmp/$nombre"; then
        MOTIVO="$VERSION no trae paquete para Linux $ARCO"
        return 1
    fi

    # El hash no es ceremonia: es lo único que distingue el binario publicado
    # de lo que devuelva un proxy por el camino.
    if descargar "$base/SHA256SUMS" "$tmp/SHA256SUMS" 2>/dev/null && hay sha256sum; then
        local esperado real
        esperado="$(grep -F "$nombre" "$tmp/SHA256SUMS" | awk '{print $1}')"
        real="$(sha256sum "$tmp/$nombre" | awk '{print $1}')"
        if [ -n "$esperado" ] && [ "$esperado" != "$real" ]; then
            morir "el hash no coincide. Descarga corrupta o manipulada; no se instala nada."
        fi
        [ -n "$esperado" ] && bien "hash comprobado"
    else
        aviso "no se pudo comprobar el hash (falta SHA256SUMS o sha256sum)"
    fi

    tar xzf "$tmp/$nombre" -C "$tmp" || morir "el paquete descargado no se pudo abrir."
    local bin; bin="$(find "$tmp" -type f -name "$PROGRAMA" -perm -u+x | head -1)"
    [ -n "$bin" ] || morir "el paquete no traía el ejecutable."

    # glibc es el único requisito del binario publicado. Comprobarlo antes de
    # copiarlo evita dejar instalado algo que no arranca.
    if ! "$bin" --version >/dev/null 2>&1; then
        MOTIVO="el binario publicado no arranca aquí (glibc demasiado antigua)"
        return 1
    fi

    mkdir -p "$PREFIJO"
    install -m 755 "$bin" "$PREFIJO/$PROGRAMA"
}

# ─── Instalar ────────────────────────────────────────────────────────────────

if [ "$FORZAR_FUENTE" -eq 1 ]; then
    compilar_desde_fuente
elif ! instalar_binario; then
    aviso "${MOTIVO:-no se pudo usar el binario publicado}"
    compilar_desde_fuente
fi

destino="$PREFIJO/$PROGRAMA"
[ -x "$destino" ] || morir "algo salió mal: no hay ejecutable en $destino"
bien "instalado en $destino ($("$destino" --version))"

# ─── Lo que queda por saber ──────────────────────────────────────────────────

case ":$PATH:" in
    *":$PREFIJO:"*) ;;
    *)
        aviso "$PREFIJO no está en tu PATH. Añádelo:"
        nota "echo 'export PATH=\"$PREFIJO:\$PATH\"' >> ~/.bashrc && exec bash"
        ;;
esac

paso "Componentes opcionales"
# Ninguno hace falta para arrancar: vsioncode los detecta y su ausencia nunca
# es un error. Se enumeran para que se sepa qué queda apagado y por qué.
opcional() {
    if hay "$1"; then bien "$1 — $2"; else nota "· $1 no está — $3"; fi
}
opcional claude   "asistente con tu suscripción de Anthropic" "usa una clave de API de otro proveedor"
opcional uv       "reconocimiento de voz"                     "el modo de voz no se ofrece"
opcional arecord  "micrófono (alsa-utils)"                    "el modo de voz no se ofrece"
opcional aplay    "respuesta hablada (alsa-utils)"            "escucha, pero no contesta en voz"

paso "Listo"
nota "Abre  $PROGRAMA  y elige proveedor de modelos en la ventana de configuración."
nota "Para volver a ella en cualquier momento:  $PROGRAMA --configurar"
