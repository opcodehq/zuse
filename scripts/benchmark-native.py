#!/usr/bin/env python3
"""Measure fresh-profile idle process-tree memory and CPU; never starts agents."""
import argparse
import json
import os
from pathlib import Path
import platform
import socket
import statistics
import subprocess
import tempfile
import time


def cpu_seconds(value):
    days, _, value = value.rpartition("-")
    total = 0.0
    for part in value.split(":"):
        total = total * 60 + float(part)
    return total + (int(days) * 86400 if days else 0)


def sample(root):
    rows = {}
    for line in subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,rss=,time="], text=True
    ).splitlines():
        pid, parent, rss, cpu = line.split()
        rows[int(pid)] = (int(parent), int(rss), cpu_seconds(cpu))
    owned = {root}
    while True:
        children = {pid for pid, row in rows.items() if row[0] in owned}
        if children <= owned:
            break
        owned |= children
    live = owned & rows.keys()
    pss = []
    for pid in live:
        if platform.system() == "Linux":
            try:
                fields = Path(f"/proc/{pid}/stat").read_text().rpartition(")")[2].split()
                seconds = (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
                rows[pid] = (*rows[pid][:2], seconds)
            except (OSError, IndexError, ValueError):
                pass
        try:
            for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
                if line.startswith("Pss:"):
                    pss.append(int(line.split()[1]))
        except (OSError, PermissionError):
            pass
    return {
        "rss_mib": sum(rows[pid][1] for pid in live) / 1024,
        "pss_mib": sum(pss) / 1024 if len(pss) == len(live) else None,
        "cpu_seconds": sum(rows[pid][2] for pid in live),
        "cpu_by_pid": {pid: rows[pid][2] for pid in live},
        "process_count": len(live),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--mode", choices=("headless", "desktop"), default="headless")
    parser.add_argument("--warmup", type=int, default=10)
    parser.add_argument("--duration", type=int, default=30)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.duration < 1 or args.warmup < 0:
        parser.error("duration must be positive and warmup nonnegative")
    binary = args.binary.resolve(strict=True)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="zuse-idle-benchmark-") as profile:
        env = {key: value for key, value in os.environ.items() if not key.startswith("ZUSE_")}
        env.update(ZUSE_DATA_DIR=profile, ZUSE_IPC_PORT=str(port), ZUSE_WORKOS_CLIENT_ID="")
        command = [str(binary)] + (["headless"] if args.mode == "headless" else [])
        with tempfile.TemporaryFile() as log:
            started = time.monotonic()
            process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
            try:
                while True:
                    if process.poll() is not None:
                        raise RuntimeError(f"App exited before IPC readiness: {process.returncode}")
                    try:
                        with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                            break
                    except OSError:
                        if time.monotonic() - started > 60:
                            raise RuntimeError("App did not open IPC within 60 seconds")
                        time.sleep(0.05)
                ready_ms = (time.monotonic() - started) * 1000
                time.sleep(args.warmup)
                baseline = sample(process.pid)
                sampled_at = time.monotonic()
                samples = []
                previous_cpu = baseline["cpu_by_pid"]
                cpu_used = 0.0
                for _ in range(args.duration):
                    time.sleep(1)
                    if process.poll() is not None:
                        raise RuntimeError(f"App exited during measurement: {process.returncode}")
                    current = sample(process.pid)
                    cpu_used += sum(max(0, cpu - previous_cpu.get(pid, 0))
                                    for pid, cpu in current["cpu_by_pid"].items())
                    previous_cpu = current["cpu_by_pid"]
                    samples.append(current)
                elapsed = time.monotonic() - sampled_at
                result = {
                    "platform": platform.platform(),
                    "architecture": platform.machine(),
                    "mode": args.mode,
                    "binary": str(binary),
                    "binary_mib": binary.stat().st_size / 1024**2,
                    "binary_modified_unix": binary.stat().st_mtime,
                    "ipc_ready_ms": ready_ms,
                    "warmup_seconds": args.warmup,
                    "measurement_seconds": elapsed,
                    "rss_median_mib": statistics.median(s["rss_mib"] for s in samples),
                    "rss_peak_mib": max(s["rss_mib"] for s in samples),
                    "pss_median_mib": statistics.median(s["pss_mib"] for s in samples)
                    if all(s["pss_mib"] is not None for s in samples) else None,
                    "cpu_percent_one_core": cpu_used / elapsed * 100,
                    "max_process_count": max(s["process_count"] for s in samples),
                    "scope": "Fresh profile, no chats, no running agents, no preview. RSS sums may count shared pages more than once. IPC readiness is not first frame. Sampled CPU may miss short-lived children; CPU is not battery power.",
                    "samples": samples,
                }
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(json.dumps(result, indent=2) + "\n")
                print(json.dumps({k: v for k, v in result.items() if k != "samples"}, indent=2))
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()


if __name__ == "__main__":
    main()
