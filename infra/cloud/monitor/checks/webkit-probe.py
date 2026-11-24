"""Probe the real Linux BrowserSurface helper, using its length-framed protocol."""
import json
import queue
import struct
import subprocess
import sys
import threading
import time

helper, port = sys.argv[1:3]
if not 0 < int(port) <= 65535:
    sys.exit("invalid port")
password = sys.stdin.readline().strip()
if not password:
    sys.exit("supply the monitor password on stdin")
proc = subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
responses = queue.Queue()


def receive():
    while header := proc.stdout.read(9):
        kind, _, length = struct.unpack("<cII", header)
        data = proc.stdout.read(length)
        if kind == b"J":
            responses.put(json.loads(data))


def send(cmd, **fields):
    data = json.dumps(dict(id=1, cmd=cmd, **fields)).encode()
    proc.stdin.write(struct.pack("<I", len(data)) + data)
    proc.stdin.flush()


threading.Thread(target=receive, daemon=True).start()
try:
    send("create")
    send("resize", width=960, height=540, scale=1)
    send("visible", value=1)
    send("load", url=f"http://127.0.0.1:{int(port)}/?token={password}")
    for _ in range(15):
        time.sleep(1)
        send("eval", script="""({decoder:typeof VideoDecoder,offscreen:typeof OffscreenCanvas,
            status:document.querySelector('#status')?.textContent,
            fps:window.zuseMonitorStats?.fps,codec:window.zuseMonitorStats?.codec})""")
        result = responses.get(timeout=5)
        if result and (result.get("codec") or "lacks" in (result.get("status") or "")):
            break
    print(json.dumps(result))
    if not result or not result.get("codec"):
        sys.exit("embedded viewer did not confirm a decoded stream")
finally:
    proc.terminate()
    proc.wait(timeout=5)
