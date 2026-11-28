#!/usr/bin/env bash
# Run against already-started servers. Keep all output on stdout as JSONL.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
upstream=${BENCH_ROOT:?Set BENCH_ROOT to the pinned voxl checkout}/bench
engine=${1:?Expected an engine name}
endpoint=${2:?Expected host:port or Moonlight host}
workload=${3:?Expected scroll, anim1080 or animfull}
link=${4:?Expected lan, 10/40 or 5/60}
seconds=${SAMPLE_SECONDS:-20}
python=${BENCH_PYTHON:-python3}
export BENCH_DISPLAY=${DISPLAY:?Set DISPLAY to the engine desktop}
port=${SHAPER_PORT:-7979}
dir=$(mktemp -d)
jobs=()
cleanup() {
  for pid in "${jobs[@]}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf -- "$dir"
}
trap cleanup EXIT
case "$engine" in
  poof) client=("$here/../native-client.py");;
  voxl|selkies) client=("$upstream/nclient.py" "$engine");;
  kasm|novnc) client=("$upstream/kclient.py");;
  neko|neko-tuned) client=("${NEKO_CLIENT:-$upstream/nkclient.py}");;
  sunshine|sunshine-tuned) client=("$upstream/mlclient.py");;
  *) echo "Unknown engine $engine" >&2; exit 2;;
esac
case "$link" in lan|10/40|5/60) ;; *) exit 2;; esac
if [[ $link != lan ]]; then
  case "$engine" in
    neko*|sunshine*)
      # These transports carry media over UDP. Apply shaping to the routed
      # interface with upstream's netem setup, or pass an active TUN shaper.
      if [[ -n ${UDP_SHAPE_FILE:-} ]]; then
        echo "$link" >"$UDP_SHAPE_FILE.next"
        mv -- "$UDP_SHAPE_FILE.next" "$UDP_SHAPE_FILE"
      elif [[ ${UDP_LINK_CONFIGURED:-} != "$link" ]]; then
        echo "Set up the UDP link before running this cell" >&2; exit 2
      fi;;
    *)
      "$python" "$upstream/shaper.py" "$port" "$endpoint" "${link%/*}" "${link#*/}" >&2 &
      jobs+=("$!")
      endpoint=127.0.0.1:$port;;
  esac
elif [[ -n ${UDP_SHAPE_FILE:-} ]]; then
  echo lan >"$UDP_SHAPE_FILE.next"
  mv -- "$UDP_SHAPE_FILE.next" "$UDP_SHAPE_FILE"
fi
case "$workload" in
  anim1080|animfull)
    cc -O2 "$upstream/loadgen.c" -o "$dir/loadgen" -lX11 -lXext
    if [[ $workload == anim1080 ]]; then w=1920; h=1080; x=300; y=150
    else w=2560; h=1600; x=0; y=0; fi
    "$dir/loadgen" "$w" "$h" 60 "$((seconds+40))" "$x" "$y" >&2 &
    jobs+=("$!");;
  scroll)
    # The native readers sample 24 physical pixels per cell. Keep Chrome at
    # scale 1 and find the marker on X11 to allow for WM decorations and DPI.
    sed -e 's/width:320px;height:32px/width:480px;height:48px/' \
        -e 's/repeat(20,16px)/repeat(20,24px)/' \
        -e 's/grid-auto-rows:16px/grid-auto-rows:24px/' \
        "$upstream/scroll.html" >"$dir/scroll.html"
    cc -O2 "$here/find-marker.c" -o "$dir/find-marker" -lX11
    CFLAGS="--disable-gpu --force-device-scale-factor=1 --kiosk" \
      SCROLL_PAGE="$dir/scroll.html" node "$upstream/scrolltarget.mjs" "$((seconds+40))" >&2 &
    jobs+=("$!")
    sleep 3
    read -r x y < <("$dir/find-marker");;
  *) echo "Unknown workload $workload" >&2; exit 2;;
esac
inside=()
if [[ -n ${CLIENT_NETNS_PID:-} && ( $engine == neko* || $engine == sunshine* ) ]]; then
  inside=(sudo nsenter -t "$CLIENT_NETNS_PID" -n sudo -u "$(id -un)" env
    "HOME=$HOME" "MOONLIGHT=${MOONLIGHT:-}" "ML_HOME=${ML_HOME:-}" "ML_BITRATE=${ML_BITRATE:-}"
    "NEKO_RECEIVE_BYTES=${NEKO_RECEIVE_BYTES:-}")
fi
result=$(timeout "$((seconds+45))" "${inside[@]}" "$python" "${client[@]}" "$endpoint" "$seconds" "$x" "$y")
"$python" - "$engine" "$workload" "$link" "$result" <<'PY'
import json, sys
engine, workload, link, data = sys.argv[1:]
result = json.loads(data)
print(json.dumps(dict(engine=engine, workload=workload, link=link, result=result)))
if result['latencyMs']['p50'] is None:
    sys.exit("No fresh pixel marker decoded")
PY
