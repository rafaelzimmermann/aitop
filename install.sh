#!/usr/bin/env bash
# install aitop: build the release binary and drop it in a bin dir on your PATH.
#
#   ./install.sh                      # build + install to ~/.local/bin
#   ./install.sh --prefix /usr/local  # install to /usr/local/bin
#   ./install.sh --no-config          # skip seeding ~/.config/aitop/.env
#   ./install.sh --run                # install, then run aitop once (plain snapshot)
#   ./install.sh --uninstall          # remove the installed binary
#
# Env overrides: PREFIX, BIN_DIR, CONF_DIR, CARGO_FLAGS

set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")"

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${BIN_DIR:-$PREFIX/bin}"
CONF_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/aitop"
RUN=0
MAKE_CONFIG=1

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)    PREFIX="$2"; BIN_DIR="$PREFIX/bin"; shift 2 ;;
        --bin-dir)   BIN_DIR="$2"; shift 2 ;;
        --conf-dir)  CONF_DIR="$2"; shift 2 ;;
        --no-config) MAKE_CONFIG=0; shift ;;
        --run)       RUN=1; shift ;;
        --uninstall)
            rm -f "$BIN_DIR/aitop"
            echo "removed $BIN_DIR/aitop"
            exit 0 ;;
        -h|--help)
            sed -n '2,10p' "$0" | sed 's/^# \?//'
            exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 1 ;;
    esac
done

say() { printf '\033[1;36maitop\033[0m %s\n' "$*"; }

# --- build --------------------------------------------------------------------
if command -v cargo >/dev/null 2>&1; then
    say "building (release)…"
    cargo build --release ${CARGO_FLAGS:---locked}
    BIN="target/release/aitop"
elif [ -x "target/release/aitop" ]; then
    say "cargo not found, using existing target/release/aitop"
    BIN="target/release/aitop"
else
    echo "error: cargo is required to build, and no prebuilt binary exists" >&2
    exit 1
fi

# --- install ------------------------------------------------------------------
mkdir -p "$BIN_DIR"
if command -v install >/dev/null 2>&1; then
    install -m 755 "$BIN" "$BIN_DIR/aitop"
else
    cp "$BIN" "$BIN_DIR/aitop" && chmod 755 "$BIN_DIR/aitop"
fi
say "installed $BIN_DIR/aitop"

case ":$PATH:" in
    *":$BIN_DIR:"*) : ;;
    *) say "note: $BIN_DIR is not on your PATH — add it, or use --prefix" ;;
esac

# --- config -------------------------------------------------------------------
if [ "$MAKE_CONFIG" = 1 ]; then
    mkdir -p "$CONF_DIR"
    if [ -f "$CONF_DIR/.env" ]; then
        say "config exists: $CONF_DIR/.env"
    elif [ -f ".env" ]; then
        cp .env "$CONF_DIR/.env"
        chmod 600 "$CONF_DIR/.env"
        say "copied project .env to $CONF_DIR/.env (chmod 600)"
    else
        cp .env.example "$CONF_DIR/.env"
        chmod 600 "$CONF_DIR/.env"
        say "wrote $CONF_DIR/.env (chmod 600) — fill in your keys"
    fi
fi

# --- smoke test ---------------------------------------------------------------
if [ "$RUN" = 1 ]; then
    "$BIN_DIR/aitop" --plain
else
    say "run: aitop          (TUI)  ·  aitop --plain  ·  aitop --json"
fi
