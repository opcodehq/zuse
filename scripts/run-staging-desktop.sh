#!/usr/bin/env bash
set -euo pipefail
checkout="$(cd "$(dirname "$0")/.." && pwd)"
cd "$checkout"
# Optional, checkout-local Linux development packages (no system installation).
native_deps="$checkout/.context/linux-deps/root/usr"
if [[ -d "$native_deps/lib/x86_64-linux-gnu/pkgconfig" ]]; then
    export PKG_CONFIG_PATH="$native_deps/lib/x86_64-linux-gnu/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
    export C_INCLUDE_PATH="$native_deps/include:$native_deps/include/x86_64-linux-gnu${C_INCLUDE_PATH:+:$C_INCLUDE_PATH}"
fi
export ZUSE_API_URL="${ZUSE_STAGING_API_URL:-http://127.0.0.1:28700}"
export ZUSE_ALLOW_LOOPBACK_ACCOUNT_HTTP=1
export ZUSE_WORKOS_CLIENT_ID="client_01KW6ZEZKVMZ0G429A89XZD83Q"
export ZUSE_DATA_DIR="${ZUSE_STAGING_DATA_DIR:-$checkout/.context/staging-desktop}"
export ZUSE_IPC_PORT="${ZUSE_STAGING_IPC_PORT:-28654}"
cargo build -j 2 -p zuse
exec "$checkout/target/debug/zuse" "$@"
