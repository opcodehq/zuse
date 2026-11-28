#!/usr/bin/env bash
# Keep package and feature selection identical for build and test invocations.
set -euo pipefail
cd "$(dirname "$0")/.."

suite=${1:?Usage: ci-rust.sh rust|native|cloud|cloud-compat}
build=(--locked --profile ci)
tests=(--locked --cargo-profile ci --profile ci --test-threads 16)
case "$suite" in
  rust)
    packages=(zuse-analytics zuse-cli zuse-client zuse-commands zuse-control zuse-doc
      zuse-engine zuse-harness zuse-harness-extensions zuse-i18n zuse-mcp zuse-model
      zuse-model-accounts zuse-orb zuse-plugins zuse-protocol zuse-providers zuse-relay
      zuse-remote zuse-rpc zuse-serve zuse-sync zuse-update zuse-voice-session)
    args=(--features zuse-providers/native-fixture,zuse-sync/mock-server)
    for package in "${packages[@]}"; do args+=(-p "$package"); done
    cargo build "${build[@]}" "${args[@]}" --tests --bin zuse-cli --bin zuse-control --example control_types
    cargo nextest run "${tests[@]}" "${args[@]}"
    target/ci/examples/control_types --check
    ;;
  native)
    # Building test harnesses also type-checks the desktop and standalone binaries.
    cargo nextest run "${tests[@]}" --lib --bins -p zuse -p zuse-cli -p zuse-relay \
      -p zuse-remote -p zuse-ui -p zuse-viewport -p zuse-text
    ;;
  cloud|cloud-compat)
    # Guests inherit this directory, preserving the shared-volume effect guard
    # within one test while unrelated fixtures can reuse their workspace IDs.
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="python3 $PWD/scripts/ci_test_runner.py"
    args=(-p zuse-cloud -p zuse-cloud-runtime -p zuse-cloud-projection
      -p zuse-cloud-api -p zuse-cloud-guest -p zuse-client -p zuse-protocol
      --features zuse-cloud/runtime-acceptance,zuse-cloud-runtime/engine)
    if [ "$suite" = cloud ]; then
      cargo build "${build[@]}" "${args[@]}" --tests --bins
      # Keep cloud's opt-in PostgreSQL/guest tests enabled. Other packages retain
      # their normal ignore policy, including subprocess-only projection helpers.
      cargo nextest run "${tests[@]}" "${args[@]}" --run-ignored all \
        -E '(package(zuse-cloud) | package(zuse-cloud-runtime)) & !test(rust_cloud_composer_launch_close_and_reopen)'
      cargo nextest run "${tests[@]}" "${args[@]}" \
        -E '!(package(zuse-cloud) | package(zuse-cloud-runtime))'
      cargo test "${build[@]}" "${args[@]}" --doc
    else
      cargo nextest run "${tests[@]}" "${args[@]}" --run-ignored only \
        -E 'package(zuse-cloud) & binary(postgres) & (test(rust_cloud) | test(recovery_acceptance)) & !test(rust_cloud_composer_launch_close_and_reopen)'
    fi
    ;;
  *) printf 'Unknown CI suite: %s\n' "$suite" >&2; exit 2 ;;
esac
