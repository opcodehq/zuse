#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'for p in ${workload:-} ${shaper:-}; do kill "$p" 2>/dev/null || true; done; rm -rf "$work"' EXIT
generator=${MONITOR_SCROLL_BIN:-$work/scroll}
if [[ ! -x "$generator" ]]; then cc -O2 "$here/scroll.c" -o "$generator" -lX11; fi
for rtt in 30 60; do
    python3 "$here/bench-link.py" "$rtt" &
    shaper=$!
    sleep 0.3
    kill -0 "$shaper"
    "$generator" &
    workload=$!
    echo "1080p scrolling at ${rtt} ms RTT"
    MONITOR_VIEWER_PORT=7902 node "$here/scroll-bench.mjs"
    kill "$workload"
    unset workload
    MONITOR_VIEWER_PORT=7902 node "$here/input-bench.mjs"
    kill "$shaper"
    unset shaper
done
