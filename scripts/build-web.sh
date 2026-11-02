#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target/browser}"
export RUSTUP_TOOLCHAIN="${ZUSE_WEB_TOOLCHAIN:-nightly}"
export NO_COLOR=true
export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--shared-memory -C link-arg=--max-memory=1073741824 -C link-arg=--import-memory -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base'
export CARGO_UNSTABLE_BUILD_STD='std,panic_abort'
cd apps/web
exec "${TRUNK_BIN:-trunk}" build "$@"
