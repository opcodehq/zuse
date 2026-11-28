#!/usr/bin/env bash
# Run inside a disposable guest with the selected streamer on loopback.
set -eu
: "${BENCH_ROOT:?Set BENCH_ROOT to the pinned upstream benchmark checkout}"
: "${PLAYWRIGHT_NODE_MODULES:?Set the absolute path to node_modules/}"
engine=${1:?Expected zuse or voxl}
shift
case "$engine" in
  zuse|voxl) ;;
  *) exit 2 ;;
esac
work=$(mktemp -d)
cleanup() {
  for pid in ${generator:-} ${proxy:-}; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
}
trap 'cleanup; rm -rf "$work"' EXIT
generator_bin=${MONITOR_LOADGEN_BIN:-$work/loadgen}
if [[ -z "${MONITOR_LOADGEN_BIN:-}" ]]; then
  cc -O2 "$BENCH_ROOT/bench/loadgen.c" -o "$generator_bin" -lX11 -lXext
fi
export VIEWPORT=1920x1080 WARMUP=2000
for rtt in "$@"; do
  python3 "$BENCH_ROOT/bench/shaper.py" 7902 127.0.0.1:6901 30 "$rtt" &
  proxy=$!
  "$generator_bin" 1920 1080 60 20 0 0 >&2 &
  generator=$!
  sleep .5
  kill -0 "$proxy" "$generator"
  node --input-type=module - "$engine" "$rtt" <<'JS'
import {readFileSync} from 'node:fs';
import {homedir} from 'node:os';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
const [engine, rtt] = process.argv.slice(2);
const token = readFileSync(process.env.MONITOR_PASSWORD_FILE || join(homedir(), '.local/state/zuse-monitor/password'), 'utf8').trim();
const marker = join(process.env.BENCH_ROOT, 'bench/measure.mjs');
const url = 'http://127.0.0.1:7902/' + (engine === 'voxl' ? '?token=' : '#') + encodeURIComponent(token);
process.argv = [process.execPath, marker, url, '10', rtt + 'ms', '0', '0'];
await import(pathToFileURL(marker));
JS
  cleanup
  unset generator proxy
done
