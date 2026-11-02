#!/usr/bin/env bash
# Exercise the production native shell and real WebKitGTK in an isolated display.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  cat <<'USAGE'
Usage: bash scripts/test-native-browser-linux.sh [--binary PATH] [--output DIR]

Build and run the native browser scenario under Xvfb with software Vulkan.
The scenario uses temporary profile data and a loopback website, and does not
launch agents or access provider credentials. Screenshots and result JSON go
to a new directory under .context/, or an empty directory passed to --output.

--binary PATH  Run an already-built browser-fixture executable.
--output DIR   Save evidence to this empty directory.

Requires stable Rust and Linux native development dependencies, plus
xvfb, xauth, xdotool, imagemagick, dbus-x11 and mesa-vulkan-drivers.
See internal-docs/development.md for details.
USAGE
}

binary=''
output=''
while (($#)); do
  case "$1" in
    --help|-h) usage; exit 0 ;;
    --binary|--output)
      option=$1
      (($# >= 2)) || { echo "$option requires a value." >&2; exit 2; }
      if [[ "$option" == --binary ]]; then binary=$2; else output=$2; fi
      shift 2 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done
[[ $(uname -s) == Linux ]] || { echo 'This scenario requires Linux.' >&2; exit 1; }
for dependency in dbus-run-session xvfb-run xauth xdotool import mktemp timeout; do
  command -v "$dependency" >/dev/null || { echo "Missing native scenario dependency: $dependency" >&2; exit 1; }
done

if [[ -z "$binary" ]]; then
  cargo +stable build --locked -p zuse-ui --features browser-fixture --example browser-fixture
  binary="${CARGO_TARGET_DIR:-target}/debug/examples/browser-fixture"
fi
[[ -x "$binary" ]] || { echo "Native fixture executable not found: $binary" >&2; exit 1; }
binary=$(realpath -- "$binary")
mkdir -p .context
if [[ -z "$output" ]]; then
  output=$(mktemp -d "$PWD/.context/native-browser-linux-XXXXXXXX")
else
  mkdir -p -- "$output"
  output=$(realpath -- "$output")
  [[ -z $(find -- "$output" -mindepth 1 -maxdepth 1 -print -quit) ]] || {
    echo "Evidence directory must be empty: $output" >&2; exit 1;
  }
fi
output=$(realpath -- "$output")
runtime=$(mktemp -d)
trap 'rmdir -- "$runtime" 2>/dev/null || true' EXIT
chmod 700 "$runtime"
printf 'Native browser evidence: %s\n' "$output"
# Leave desktop-owned environment overrides out of this isolated scenario.
# A bounded run prevents a crashed renderer from leaving CI waiting forever.
if timeout --signal=TERM --kill-after=10s 300s \
  dbus-run-session -- xvfb-run -a -s '-screen 0 1440x1000x24' \
  env -u WAYLAND_DISPLAY -u ZUSE_BROWSER_CAPTURE_WINDOW -u ZUSE_BROWSER_STORAGE_URL \
    -u ZUSE_BROWSER_STORAGE_ROOT -u ZUSE_TRANSCRIPT_LINK_FIXTURE_ONLY \
    -u ZUSE_BROWSER_STORAGE_PROFILE -u ZUSE_BROWSER_NATIVE_POINTER \
    XDG_RUNTIME_DIR="$runtime" LC_ALL=C.UTF-8 WGPU_BACKEND=vulkan \
    "$binary" "$output" > "$output/run.log" 2>&1; then
  :
else
  status=$?
  tail -40 "$output/run.log" >&2
  echo "Native scenario failed (exit $status); evidence: $output" >&2
  exit "$status"
fi
[[ -f "$output/result.txt" && -f "$output/linux-results.json" ]] || {
  echo "Native scenario did not produce completion evidence; inspect $output/run.log" >&2
  exit 1
}
printf 'Native browser scenario passed. Evidence: %s\n' "$output"
