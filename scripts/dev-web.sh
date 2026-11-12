#!/usr/bin/env bash
# Build both halves from this checkout before opening the local workspace.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<'USAGE'
Usage: bash scripts/dev-web.sh [zuse serve options]

Build the browser and start its matching local engine. Open the private link
printed by the server in a WebGPU-capable browser.

Defaults: localhost:27656, an automatically allocated IPC port, and the
persistent .context/local-profile data directory. Server options override
these defaults, for example:

  bash scripts/dev-web.sh --listen 127.0.0.1:3000
  bash scripts/dev-web.sh --data-dir /path/to/profile

Requires stable Rust, nightly Rust with rust-src and wasm32-unknown-unknown,
and Trunk 0.21.14. See internal-docs/development.md for setup.
USAGE
  exit 0
fi

command -v cargo >/dev/null || { echo 'Install Rust before starting Zuse.' >&2; exit 1; }
command -v "${TRUNK_BIN:-trunk}" >/dev/null || {
  echo 'Install Trunk 0.21.14 or set TRUNK_BIN to its executable.' >&2
  exit 1
}

# Keep caller values intact, including paths containing spaces. Clap rejects
# repeated flags, so supply a default only when the caller did not name it.
server_defaults=()
for option in ipc-port assets data-dir; do
  supplied=false
  for argument in "$@"; do
    if [[ "$argument" == "--$option" || "$argument" == "--$option="* ]]; then
      supplied=true
      break
    fi
  done
  if [[ "$supplied" == false ]]; then
    case "$option" in
      ipc-port) server_defaults+=(--ipc-port 0) ;;
      assets) server_defaults+=(--assets "$PWD/dist/web") ;;
      data-dir) server_defaults+=(--data-dir "${ZUSE_DATA_DIR:-$PWD/.context/local-profile}") ;;
    esac
  fi
done

bash scripts/build-web.sh
exec python3 scripts/cargo_dev.py +stable run --locked -p zuse-cli --bin zuse-cli -- serve "${server_defaults[@]}" "$@"
