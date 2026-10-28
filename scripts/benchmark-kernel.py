#!/usr/bin/env python3
"""Measure an isolated scripted kernel workload; never invokes a model provider."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time


def sample(pid):
    result = {}
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            key, _, value = line.partition(":")
            if key in ("VmRSS", "VmHWM"):
                result[key] = int(value.split()[0]) * 1024
        for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
            if line.startswith("Pss:"):
                result["pss_bytes"] = int(line.split()[1]) * 1024
        for line in Path(f"/proc/{pid}/io").read_text().splitlines():
            key, _, value = line.partition(":")
            if key in ("write_bytes", "read_bytes"):
                result[key] = int(value)
        stat = Path(f"/proc/{pid}/stat").read_text().rpartition(")")[2].split()
        result["cpu_seconds"] = (int(stat[11]) + int(stat[12])) / os.sysconf("SC_CLK_TCK")
    except (OSError, ValueError, IndexError):
        pass
    return result


def run(binary, roots, size, image, iterations, compact):
    command = [str(binary), "--roots", str(roots), "--history-mib", str(size), "--iterations", str(iterations)]
    if image:
        command.append("--image")
    if compact:
        command.append("--compact")
    samples = []
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        process = subprocess.Popen(command, stdout=stdout, stderr=stderr)
        deadline = time.monotonic() + 300
        try:
            while process.poll() is None:
                samples.append(sample(process.pid))
                if time.monotonic() > deadline:
                    raise TimeoutError("Scripted workload exceeded five minutes")
                time.sleep(0.01)
            stdout.seek(0)
            stderr.seek(0)
            if process.returncode:
                return {"status": "failed", "exit_code": process.returncode, "stderr": stderr.read().decode(errors="replace")[-4000:]}
            lines = stdout.read().decode().splitlines()
            result = json.loads(lines[-1])
            result.update(status="passed", peak_rss_bytes=max((s.get("VmHWM", 0) for s in samples), default=0),
                          peak_sampled_pss_bytes=max((s.get("pss_bytes", 0) for s in samples), default=0),
                          post_close_sampled_rss_bytes=statistics.median([s["VmRSS"] for s in samples[-5:] if "VmRSS" in s]) if any("VmRSS" in s for s in samples[-5:]) else None,
                          process_write_bytes=max((s.get("write_bytes", 0) for s in samples), default=0),
                          cpu_seconds=max((s.get("cpu_seconds", 0) for s in samples), default=0))
            return result
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--iterations", type=int, default=2)
    parser.add_argument("--quick", action="store_true")
    args = parser.parse_args()
    if not 1 <= args.repeat <= 10 or not 1 <= args.iterations <= 100:
        parser.error("repeat must be 1..10; iterations must be 1..100")
    if platform.system() != "Linux":
        parser.error("This sampler uses Linux /proc; run on the cloud VM")
    binaries = {"current": args.binary.resolve(strict=True)}
    if args.baseline:
        binaries["baseline"] = args.baseline.resolve(strict=True)
    roots = [1, 5] if args.quick else [1, 2, 5, 10, 20]
    sizes = [0, 1] if args.quick else [0, 1, 10]
    result = {"platform": platform.platform(), "cpu_count": os.cpu_count(), "transport": "scripted",
              "limitations": "Single process, no external tools or UI. RSS uses sampled kernel high-water marks; PSS/IO/CPU polling can miss final activity. The example waits 100 ms after close; post-close RSS is the median of the final five samples. Disk growth is retained bytes, not bytes written.",
              "binaries": {name: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "bytes": path.stat().st_size} for name, path in binaries.items()}, "runs": []}
    workloads = [(n, size, False, False) for n in roots for size in sizes]
    workloads += [(n, 1, True, False) for n in roots]
    workloads += [(1, 1, False, True)]
    for root_count, size, image, compact in workloads:
        for repeat in range(args.repeat):
            # Alternate ordering to reduce systematic warm-cache advantage.
            arms = list(binaries.items())
            if repeat % 2:
                arms.reverse()
            for name, path in arms:
                row = run(path, root_count, size, image, args.iterations, compact)
                row.update(arm=name, repeat=repeat, workload={"roots": root_count, "history_mib": size, "image": image, "compact": compact})
                result["runs"].append(row)
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(json.dumps(result, indent=2) + "\n")
                print(json.dumps({"arm": name, "roots": root_count, "history_mib": size, "image": image, "compact": compact, "status": row["status"], "elapsed_ms": row.get("elapsed_ms")}), flush=True)
    failed = sum(row["status"] != "passed" for row in result["runs"])
    print(f"Saved {len(result['runs'])} runs ({failed} failed) to {args.output}")
    raise SystemExit(bool(failed))


if __name__ == "__main__":
    main()
