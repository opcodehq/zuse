#!/usr/bin/env python3
"""Sample the release harness workload; build it before running this command.

Example: cargo build --release -p zuse-harness --example workload --locked
python3 scripts/benchmark-harness.py --output .context/efficiency/before
"""
import argparse
import hashlib
import importlib.util
import json
import os
import platform
import signal
import subprocess
import tempfile
import time
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "native_sampler", Path(__file__).with_name("benchmark-native.py")
)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


def run(binary, threads, events, rounds, output, timeout):
    result_file = output.with_suffix(".workload.json")
    command = [str(binary), str(threads), str(events), str(rounds), str(result_file)]
    samples = []
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="zuse-harness-workload-") as temporary_root, \
            output.with_suffix(".log").open("w") as log:
        environment = dict(os.environ)
        environment["TMPDIR"] = temporary_root
        process = subprocess.Popen(
            command, stdout=log, stderr=subprocess.STDOUT, env=environment,
            start_new_session=os.name == "posix",
        )
        try:
            while process.poll() is None:
                if time.monotonic() - started > timeout:
                    raise TimeoutError(f"Workload exceeded {timeout}s: {command}")
                measurement = native.sample(process.pid)
                measurement["elapsed_seconds"] = time.monotonic() - started
                samples.append(measurement)
                time.sleep(0.1)
            if process.returncode:
                raise RuntimeError(f"Workload exited {process.returncode}; see {log.name}")
        finally:
            if os.name == "posix":
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            elif process.poll() is None:
                process.kill()
            process.wait()
    result = json.loads(result_file.read_text())
    with binary.open("rb") as stream:
        hasher = hashlib.sha256()
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
        digest = hasher.hexdigest()
    result.update(
        platform=platform.platform(),
        architecture=platform.machine(),
        binary=str(binary),
        binary_sha256=digest,
        binary_modified_unix=binary.stat().st_mtime,
        rss_peak_mib=max((s["rss_mib"] for s in samples), default=None),
        sample_interval_seconds=0.1,
        total_seconds=time.monotonic() - started,
        samples=samples,
    )
    output.with_suffix(".json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "samples"}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path,
                        default=Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "release/examples/workload")
    parser.add_argument("--threads", type=int, nargs="+", default=[20, 50, 100, 125])
    parser.add_argument("--events", type=int, default=10_000)
    parser.add_argument("--rounds", type=int, default=1)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if min(*args.threads, args.events, args.rounds, args.repeats, args.timeout) < 1:
        parser.error("Counts and timeout must be positive")
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=True)
    for threads in args.threads:
        for repeat in range(args.repeats):
            output = args.output / f"threads-{threads}-events-{args.events}-run-{repeat + 1}"
            run(binary, threads, args.events, args.rounds, output, args.timeout)


if __name__ == "__main__":
    main()
