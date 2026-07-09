#!/usr/bin/env bash
#
# Run CAVS Desktop locally.
#
#   ./run.sh            launch the app in dev mode (Tauri + Vite, hot reload)
#   ./run.sh web        frontend only (Vite dev server on http://localhost:1420)
#   ./run.sh build      production bundle (installers/app package)
#   ./run.sh check      typecheck frontend + cargo check backend
#
# Dependencies are installed automatically the first time.

set -euo pipefail

# Always operate from the desktop project directory, regardless of cwd.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

MODE="${1:-dev}"

# ---- prerequisites ---------------------------------------------------------
need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "✗ '$1' is required but was not found in PATH." >&2
    echo "  $2" >&2
    exit 1
  fi
}
need node "Install Node.js (https://nodejs.org)."
need npm  "npm ships with Node.js."

# ---- install deps once -----------------------------------------------------
if [ ! -d node_modules ]; then
  echo "→ Installing frontend dependencies (first run)…"
  npm install
fi

# ---- run -------------------------------------------------------------------
case "$MODE" in
  dev)
    echo "→ Launching CAVS Desktop (dev)…"
    npm run tauri dev
    ;;
  web)
    echo "→ Starting Vite dev server (frontend only) on http://localhost:1420 …"
    npm run dev
    ;;
  build)
    echo "→ Building CAVS Desktop production bundle…"
    npm run tauri build
    ;;
  check)
    echo "→ Typechecking frontend…"
    npm run build
    echo "→ Checking Rust backend…"
    cargo check --manifest-path src-tauri/Cargo.toml
    ;;
  *)
    echo "Unknown mode: '$MODE'" >&2
    echo "Usage: ./run.sh [dev|web|build|check]" >&2
    exit 1
    ;;
esac
