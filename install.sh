#!/usr/bin/env bash
# install aitop: build the release binary and drop it in a bin dir on your PATH.
#
#   ./install.sh                      # build + install to ~/.local/bin
#   ./install.sh --check              # report build dependencies and exit (no build)
#   ./install.sh --offline            # build without network (needs a warm crate cache)
#   ./install.sh --rust-default       # set the rustup default toolchain if none is set
#   ./install.sh --run                # install, then run aitop once (plain snapshot)
#   ./install.sh --no-config          # skip seeding ~/.config/aitop/.env
#   ./install.sh --prefix /usr/local  # install to /usr/local/bin
#   ./install.sh --bin-dir DIR        # install to an explicit bin dir
#   ./install.sh --conf-dir DIR       # explicit config dir
#   ./install.sh --uninstall          # remove the installed binary
#
# Env overrides: PREFIX, BIN_DIR, CONF_DIR, CARGO_FLAGS, TOOLCHAIN
#
# Requirements: a working Rust toolchain (cargo + rustc), a POSIX shell, coreutils.
# First build also needs network access to fetch crates unless --offline is used.

set -euo pipefail

say()  { printf '\033[1;36maitop\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33maitop\033[0m note: %s\n' "$*"; }
die()  { printf '\033[1;31maitop\033[0m error: %s\n' "$*" >&2; exit 1; }

# --- locate this script (portable; readlink -f is not on every BSD/macOS) ------
SCRIPT_DIR=$(dirname "${0}")
case "$SCRIPT_DIR" in
    /*) : ;;
    *) SCRIPT_DIR="${PWD}/${SCRIPT_DIR#./}" ;;
esac
SCRIPT_DIR=$(cd "$SCRIPT_DIR" && pwd -P)
cd "$SCRIPT_DIR"

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${BIN_DIR:-$PREFIX/bin}"
CONF_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/aitop"
RUN=0
MAKE_CONFIG=1
OFFLINE=0
SET_RUST_DEFAULT=0
CHECK_ONLY=0

while [ $# -gt 0 ]; do
    case "$1" in
        --check)         CHECK_ONLY=1; shift ;;
        --offline)       OFFLINE=1; shift ;;
        --rust-default)  SET_RUST_DEFAULT=1; shift ;;
        --run)           RUN=1; shift ;;
        --no-config)     MAKE_CONFIG=0; shift ;;
        --prefix)        [ $# -ge 2 ] || die "--prefix needs a value"; PREFIX="$2"; BIN_DIR="$PREFIX/bin"; shift 2 ;;
        --bin-dir)       [ $# -ge 2 ] || die "--bin-dir needs a value"; BIN_DIR="$2"; shift 2 ;;
        --conf-dir)      [ $# -ge 2 ] || die "--conf-dir needs a value"; CONF_DIR="$2"; shift 2 ;;
        --uninstall)
            rm -f "$BIN_DIR/aitop"
            say "removed $BIN_DIR/aitop"
            exit 0 ;;
        -h|--help)
            awk 'NR==1 { next } /^#/ { print; next } { exit }' "$0" | sed 's/^# \?//'
            exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

# --- toolchain resolution ------------------------------------------------------
# `command -v cargo` is not enough: rustup ships cargo/rustc as shims that fail
# when no default toolchain is configured. Probe by actually running cargo.
CARGO=()
TC_USED=""
pick_cargo() {
    if cargo --version >/dev/null 2>&1; then CARGO=(cargo); return 0; fi
    if command -v rustup >/dev/null 2>&1; then
        local tc
        tc=$(rustup toolchain list 2>/dev/null | awk '/^stable/{print $1; exit}')
        [ -n "$tc" ] || tc=$(rustup toolchain list 2>/dev/null | awk 'NR==1{print $1}')
        if [ -n "${TOOLCHAIN:-}" ]; then tc="$TOOLCHAIN"; fi
        if [ -n "$tc" ] && rustup run "$tc" cargo --version >/dev/null 2>&1; then
            CARGO=(rustup run "$tc" cargo)
            TC_USED="$tc"
            warn "rustup has no default toolchain; building via 'rustup run $tc cargo'."
            warn "to fix it once: rustup default $tc"
            return 0
        fi
        if [ "$SET_RUST_DEFAULT" = 1 ] && [ -n "$tc" ]; then
            say "setting rustup default to $tc…"
            rustup default "$tc"
            if cargo --version >/dev/null 2>&1; then CARGO=(cargo); return 0; fi
        fi
        die "cargo is present but not usable and no installed rustup toolchain works.
       Install one (needs network): rustup default stable"
    fi
    die "no Rust toolchain found. Install one: https://www.rust-lang.org/tools/install
       (or run this installer from a directory that already has target/release/aitop)"
}

if ! pick_cargo; then die "no usable Rust toolchain"; fi

# --- dependency report ---------------------------------------------------------
rustc_probe() {
    if [ -n "$TC_USED" ]; then rustup run "$TC_USED" rustc --version >/dev/null 2>&1; else rustc --version >/dev/null 2>&1; fi
}

report_deps() {
    say "deps:"
    say "  toolchain  -> ${CARGO[*]} $("${CARGO[@]}" --version 2>/dev/null | head -1)"
    say "  rustc      -> $(rustc_probe && echo ok || echo missing)"
    local f
    for f in mkdir chmod cp sed; do
        command -v "$f" >/dev/null 2>&1 || warn "missing coreutils tool: $f"
    done
    if [ "$OFFLINE" = 1 ]; then
        say "  network    -> skipped (--offline); needs crates already in \${CARGO_HOME:-~/.cargo}/registry"
    elif command -v curl >/dev/null 2>&1; then
        if curl -s -m 8 -o /dev/null https://crates.io/; then
            say "  network    -> crates.io reachable (first build can fetch dependencies)"
        else
            warn "crates.io unreachable; pass --offline if the crates are already cached"
        fi
    else
        warn "no curl to probe crates.io; build may fail offline — try --offline"
    fi
    [ -f Cargo.lock ] && say "  Cargo.lock -> present (build is --locked by default)" \
        || warn "Cargo.lock missing; building unlocked (dependency versions may drift)"
}

if [ "$CHECK_ONLY" = 1 ]; then report_deps; say "check complete — nothing built."; exit 0; fi

# --- build ---------------------------------------------------------------------
CARGO_FLAGS_ARR=()
if [ -n "${CARGO_FLAGS:-}" ]; then read -r -a CARGO_FLAGS_ARR <<< "$CARGO_FLAGS"; fi
if [ "${#CARGO_FLAGS_ARR[@]}" -eq 0 ] && [ -f Cargo.lock ]; then CARGO_FLAGS_ARR=(--locked); fi
if [ "$OFFLINE" = 1 ]; then CARGO_FLAGS_ARR=(${CARGO_FLAGS_ARR[@]+"${CARGO_FLAGS_ARR[@]}"} --offline); fi

say "building (release) with ${CARGO[*]} ${CARGO_FLAGS_ARR[*]}…"
if ! "${CARGO[@]}" build --release ${CARGO_FLAGS_ARR[@]+"${CARGO_FLAGS_ARR[@]}"}; then
    warn "if the failure was 'no matching package' or a download error, you need network access"
    warn "if the crates are already cached locally, retry with: ./install.sh --offline"
    warn "if rustup complained about a default toolchain, retry with: ./install.sh --rust-default"
    die "build failed"
fi

BIN="target/release/aitop"
[ -x "$BIN" ] || die "expected $BIN after a successful build, but it is missing"

# --- install -------------------------------------------------------------------
if ! mkdir -p "$BIN_DIR" 2>/dev/null; then
    die "cannot create $BIN_DIR (try --prefix \$HOME/.local, or run with sudo for system paths)"
fi
if [ ! -w "$BIN_DIR" ]; then
    die "$BIN_DIR is not writable (try --prefix \$HOME/.local, or sudo)"
fi
if command -v install >/dev/null 2>&1; then
    install -m 755 "$BIN" "$BIN_DIR/aitop"
else
    cp "$BIN" "$BIN_DIR/aitop" && chmod 755 "$BIN_DIR/aitop"
fi
say "installed $BIN_DIR/aitop"

case ":$PATH:" in
    *":$BIN_DIR:"*) : ;;
    *) warn "$BIN_DIR is not on your PATH — add it, or use --prefix" ;;
esac

# --- config --------------------------------------------------------------------
if [ "$MAKE_CONFIG" = 1 ]; then
    if ! mkdir -p "$CONF_DIR" 2>/dev/null; then
        warn "cannot create $CONF_DIR; skipping config seed"
        MAKE_CONFIG=0
    fi
    if [ "$MAKE_CONFIG" = 1 ]; then
        if [ -f "$CONF_DIR/.env" ]; then
            say "config exists: $CONF_DIR/.env"
        elif [ -f .env ]; then
            cp .env "$CONF_DIR/.env"; chmod 600 "$CONF_DIR/.env"
            say "copied project .env to $CONF_DIR/.env (chmod 600)"
        elif [ -f .env.example ]; then
            cp .env.example "$CONF_DIR/.env"; chmod 600 "$CONF_DIR/.env"
            say "wrote $CONF_DIR/.env (chmod 600) — fill in your keys"
        else
            warn "no .env or .env.example to seed; configure \$HOME/.config/aitop/.env yourself"
        fi
    fi
fi

# --- smoke test ----------------------------------------------------------------
if [ "$RUN" = 1 ]; then
    "$BIN_DIR/aitop" --plain
else
    say "run: aitop          (TUI)  ·  aitop --plain  ·  aitop --json"
fi
