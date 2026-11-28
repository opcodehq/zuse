#!/usr/bin/env bash
# lld shortens links for the large test executables without changing Rust codegen.
set -euo pipefail
{
  echo 'CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang'
  echo 'CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS=-C link-arg=-fuse-ld=lld'
} >> "${GITHUB_ENV:?This setup step requires the GitHub Actions environment file}"
