#!/usr/bin/env python3
"""Compare idle shared backends with manifest-defined local session controls.

No prompts or model turns are sent. Commands and root-creation RPC templates are
supplied by an ignored manifest; the sampler never reads authentication profiles.
"""
import argparse
import importlib.util
import json
import os
import signal
import socket
import subprocess
import tempfile
import time
import statistics
import select
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "evaluation", Path(__file__).with_name("evaluate-harness.py")
)
evaluation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluation)


def template(value, fields):
    if isinstance(value, str):
        return value.format_map(fields)
    if isinstance(value, list):
        return [template(v, fields) for v in value]
    if isinstance(value, dict):
        return {k: template(v, fields) for k, v in value.items()}
    return value


def line(process, expected):
    deadline = time.monotonic() + 20
    pending = bytearray()
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("backend exited before readiness")
        ready, _, _ = select.select([process.stdout], [], [], 0.05)
        if not ready:
            continue
        chunk = os.read(process.stdout.fileno(), 4096)
        if not chunk:
            raise RuntimeError("backend closed its control output")
        pending.extend(chunk)
        if len(pending) > 65536:
            raise RuntimeError("backend readiness output exceeds 64 KiB")
        while b"\n" in pending:
            raw, _, rest = pending.partition(b"\n")
            pending = bytearray(rest)
            if raw.decode().strip() == expected:
                return
    raise TimeoutError("backend readiness timed out")


def rpc(path, request):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(15)
        stream.connect(str(path))
        stream.sendall(json.dumps(request).encode() + b"\n")
        data = bytearray()
        while len(data) <= 1024 * 1024:
            chunk = stream.recv(4096)
            if not chunk:
                break
            data.extend(chunk)
            while b"\n" in data:
                raw, _, rest = data.partition(b"\n")
                data = bytearray(rest)
                response = json.loads(raw)
                if "ok" in response:
                    if not response["ok"]:
                        raise RuntimeError("root creation failed")
                    return response
        raise RuntimeError("invalid or oversized backend response")


def measure(pid, seconds=1):
    metrics = evaluation.ProcessTreeMetrics(pid)
    samples = []
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if not metrics.sample():
            raise RuntimeError("backend exited during resource sampling")
        samples.append((metrics.current_rss, metrics.current_pss))
        time.sleep(0.02)
    return {
        "settled_rss_bytes": statistics.median(v[0] for v in samples),
        "settled_pss_bytes": statistics.median(v[1] for v in samples),
        "peak_tree_rss_bytes": metrics.peak_rss,
        "peak_tree_pss_bytes": metrics.peak_pss,
    }


def run(arm, roots, repeat, output):
    with tempfile.TemporaryDirectory(prefix="shared-host-") as directory:
        base = Path(directory)
        checkout = base / "checkout"
        checkout.mkdir()
        runtime = base / "runtime"
        runtime.mkdir()
        home = base / "home"
        home.mkdir()
        fields = {
            "roots": str(roots),
            "checkout": str(checkout),
            "runtime": str(runtime),
            "home": str(home),
            "socket": str(runtime / "bench.sock"),
        }
        env = os.environ.copy()
        for name in [
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "OPENAI_API_KEY",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
        ]:
            env.pop(name, None)
        env.update(template(arm.get("env", {}), fields))
        if "files" in arm:
            for name, body in arm["files"].items():
                path = Path(template(name, fields))
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(template(body, fields))
        started = time.monotonic()
        with (output / f"{arm['label']}-{roots}-{repeat}.stderr").open("w") as err:
            p = subprocess.Popen(
                template(arm["command"], fields),
                cwd=checkout,
                env=env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=err,
                start_new_session=True,
            )
            try:
                if arm["control"] == "stdio":
                    line(p, "READY")
                else:
                    path = Path(template(arm["controlSocket"], fields))
                    deadline = time.monotonic() + 20
                    while not path.exists():
                        if p.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError("control socket unavailable")
                        time.sleep(0.02)
                    for _ in range(roots):
                        rpc(path, template(arm["createRequest"], fields))
                ready_seconds = time.monotonic() - started
                time.sleep(0.2)
                row = measure(p.pid)
                row.update(
                    arm=arm["label"],
                    roots=roots,
                    repeat=repeat,
                    ready_seconds=ready_seconds,
                    model_requests=0,
                    status="passed",
                )
                if arm["control"] == "stdio":
                    p.stdin.write(b"c")
                    p.stdin.flush()
                    line(p, "CLOSED")
                    time.sleep(0.1)
                    row["after_close"] = measure(p.pid, 0.2)
                    p.stdin.write(b"q")
                    p.stdin.flush()
                    p.wait(timeout=10)
                return row
            finally:
                if p.poll() is None:
                    os.killpg(p.pid, signal.SIGTERM)
                    try:
                        p.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        os.killpg(p.pid, signal.SIGKILL)
                        p.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeat", type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.repeat <= 10:
        parser.error("repeat must be 1..10")
    args.output.mkdir(parents=True, exist_ok=False)
    arms = json.loads(args.manifest.read_text())["arms"]
    result = {
        "mode": "idle-shared-host",
        "limitations": "One host per arm; no model turns, active histories or UI. Feature surfaces differ. PSS divides shared pages; external network must be disabled by manifest.",
        "runs": [],
    }
    for repetition in range(args.repeat):
        for n in [1, 2, 5, 10, 20]:
            for arm in (arms if repetition % 2 == 0 else list(reversed(arms))):
                try:
                    row = run(arm, n, repetition, args.output)
                except Exception as e:
                    row = {
                        "arm": arm["label"],
                        "roots": n,
                        "repeat": repetition,
                        "status": "failed",
                        "error": type(e).__name__ + ": " + str(e),
                    }
                result["runs"].append(row)
                (args.output / "report.json").write_text(json.dumps(result, indent=2))
                print(json.dumps(row), flush=True)


if __name__ == "__main__":
    main()
