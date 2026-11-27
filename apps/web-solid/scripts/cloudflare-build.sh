#!/usr/bin/env bash
# Builds the Solid web client for Cloudflare Workers Builds, whose image has bun and node but no Rust.
# Installs what scripts/build-link.mjs needs to compile crates/web-link to WebAssembly, then builds
# dist/web-solid and fails if the paired transport is missing from it.
set -euo pipefail

app="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="$(cd "$app/../.." && pwd)"
tools="${ZUSE_BUILD_TOOLS:-${XDG_CACHE_HOME:-$HOME/.cache}/zuse-web-build}"
binaryen=version_123

case "$(uname -m)" in
  x86_64 | amd64) bindgen_arch=x86_64-unknown-linux-musl binaryen_arch=x86_64-linux ;;
  aarch64 | arm64) bindgen_arch=aarch64-unknown-linux-gnu binaryen_arch=aarch64-linux ;;
  *) echo "cloudflare-build: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

if ! command -v cargo >/dev/null 2>&1; then
  [ -f "$HOME/.cargo/env" ] || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none --no-modify-path
  . "$HOME/.cargo/env"
fi

# The toolchain comes from rust-toolchain.toml at the repository root.
cd "$repo"
if command -v rustup >/dev/null 2>&1; then
  rustup toolchain install --profile minimal
  rustup target add wasm32-unknown-unknown
fi

version="$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')"
[ -n "$version" ] || { echo "cloudflare-build: wasm-bindgen is not in Cargo.lock" >&2; exit 1; }
mkdir -p "$tools"
bindgen_dir="$tools/wasm-bindgen-$version-$bindgen_arch"
[ -x "$bindgen_dir/wasm-bindgen" ] || curl -fsSL "https://github.com/rustwasm/wasm-bindgen/releases/download/$version/wasm-bindgen-$version-$bindgen_arch.tar.gz" | tar xz -C "$tools"
binaryen_dir="$tools/binaryen-$binaryen"
[ -x "$binaryen_dir/bin/wasm-opt" ] || curl -fsSL "https://github.com/WebAssembly/binaryen/releases/download/$binaryen/binaryen-$binaryen-$binaryen_arch.tar.gz" | tar xz -C "$tools"
export ZUSE_WASM_BINDGEN="$bindgen_dir/wasm-bindgen"
export ZUSE_WASM_OPT="$binaryen_dir/bin/wasm-opt"

cd "$app"
bun install --frozen-lockfile
bun run build:cloudflare

if ! ls "$repo"/dist/web-solid/assets/*.wasm >/dev/null 2>&1; then
  echo "cloudflare-build: dist/web-solid/assets has no .wasm, so paired connections would be missing" >&2
  exit 1
fi
