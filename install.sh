#!/usr/bin/env bash
#
# install.sh — build agent-controller from the repo and install the binary.
#
# From a checkout:
#   ./install.sh                 # build (release) and install to ~/bin
#   PREFIX=/usr/local/bin ./install.sh
#   ./install.sh --install-deps  # also `brew install` missing build/runtime deps
#   ./install.sh --debug         # build a debug binary (faster build)
#
# Remote one-liner (clones the repo, then builds + installs):
#   curl -fsSL https://raw.githubusercontent.com/praiseisaac/agent-controller/main/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/praiseisaac/agent-controller/main/install.sh | bash -s -- --install-deps
#
set -euo pipefail

# When piped from curl, BASH_SOURCE is not a file in the repo.
if [ -n "${BASH_SOURCE:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
  REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
else
  REPO_DIR=""
fi
PREFIX="${PREFIX:-$HOME/bin}"
BIN="agent-controller"
PROFILE="release"
INSTALL_DEPS=0
REPO_URL="${AGENT_CONTROLLER_REPO:-https://github.com/praiseisaac/agent-controller.git}"
REF="${AGENT_CONTROLLER_REF:-main}"

while [ $# -gt 0 ]; do
  case "$1" in
    --install-deps) INSTALL_DEPS=1 ;;
    --debug)        PROFILE="debug" ;;
    --repo)         REPO_URL="$2"; shift ;;
    -h|--help)      sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 1 ;;
  esac
  shift
done

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "agent-controller targets macOS."

# ---- prerequisites --------------------------------------------------------

have() { command -v "$1" >/dev/null 2>&1; }

maybe_brew_install() {
  # $1 = brew formula, $2 = human name
  if [ "$INSTALL_DEPS" = "1" ] && have brew; then
    info "Installing $2 via Homebrew…"
    brew install "$1"
  else
    return 1
  fi
}

# Rust toolchain (required).
have cargo || die "Rust/cargo not found. Install from https://rustup.rs and re-run."

# protoc (required: the ios-sim backend compiles idb.proto via tonic-build).
if ! have protoc; then
  maybe_brew_install protobuf "Protocol Buffers (protoc)" \
    || die "protoc not found (needed to build the ios-sim backend).
       Install it:  brew install protobuf   (or re-run with --install-deps)"
fi

# ---- obtain the source ----------------------------------------------------
# If not running inside a checkout (e.g. piped from curl), clone the repo.
is_checkout() { [ -n "$REPO_DIR" ] && [ -f "$REPO_DIR/Cargo.toml" ] && [ -d "$REPO_DIR/crates/core" ] && [ -f "$REPO_DIR/app/Cargo.toml" ]; }
if ! is_checkout; then
  have git || die "git not found (needed to clone the repo)."
  CLONE_DIR="$(mktemp -d)/agent-controller"
  info "Cloning $REPO_URL ($REF)…"
  git clone --depth 1 --branch "$REF" "$REPO_URL" "$CLONE_DIR" 2>/dev/null \
    || git clone --depth 1 "$REPO_URL" "$CLONE_DIR" \
    || die "failed to clone $REPO_URL"
  REPO_DIR="$CLONE_DIR"
fi

# ---- build ----------------------------------------------------------------

info "Building $BIN ($PROFILE)…"
cd "$REPO_DIR"
if [ "$PROFILE" = "release" ]; then
  cargo build --release
  BUILT="$REPO_DIR/target/release/$BIN"
else
  cargo build
  BUILT="$REPO_DIR/target/debug/$BIN"
fi
[ -x "$BUILT" ] || die "build did not produce $BUILT"

# ---- install --------------------------------------------------------------

mkdir -p "$PREFIX"

# A running firefox daemon is `current_exe` = the installed binary; overwriting
# it in place while mapped invalidates its code signature ("Killed: 9"). Stop any
# daemon and remove the old file first so we get a fresh inode.
pkill -f "__firefox-daemon" 2>/dev/null || true
rm -f "$PREFIX/$BIN"
cp "$BUILT" "$PREFIX/$BIN"
chmod 755 "$PREFIX/$BIN"
info "Installed $PREFIX/$BIN"

case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *) warn "$PREFIX is not on your PATH. Add:  export PATH=\"$PREFIX:\$PATH\"" ;;
esac

# ---- optional runtime deps for specific backends --------------------------

if [ "$INSTALL_DEPS" = "1" ] && have brew; then
  have idb_companion || brew install facebook/fb/idb-companion || \
    warn "could not install idb-companion (needed only for the ios-sim backend)"
fi

# ---- next steps -----------------------------------------------------------

cat <<EOF

$BIN installed. Per-backend setup:

  mac      run '$BIN doctor' → grant Accessibility + Screen Recording to your terminal
  safari   enable Remote Automation: 'safaridriver --enable' (or Safari ▸ Develop ▸ Allow Remote Automation)
  ios-sim  needs idb_companion: 'brew install facebook/fb/idb-companion' + a booted simulator
  firefox  needs Firefox installed (auto-detected); no setup
  chrome   needs Google Chrome installed (auto-detected); no setup

Check everything:   $BIN doctor
Register with Claude (MCP):   claude mcp add agent-controller -- $BIN mcp
Docs:               $REPO_DIR/docs/
EOF
