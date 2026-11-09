#!/usr/bin/env python3
"""Rebuild and relaunch the native UI while its development engine stays alive."""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time

from cargo_dev import build_environment


ROOT = Path(__file__).resolve().parent.parent


def snapshot():
    files = [ROOT / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    for name in ("crates", "apps/desktop", ".cargo"):
        for directory, _, names in os.walk(ROOT / name):
            files.extend(Path(directory) / name for name in names)
    result = {}
    for path in files:
        try:
            stat = path.stat()
            result[path] = (stat.st_mtime_ns, stat.st_size)
        except FileNotFoundError:
            pass
    return result


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def interrupted(*_):
    raise KeyboardInterrupt()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", default=os.environ.get("ZUSE_DATA_DIR", str(ROOT / ".context/native-profile")))
    parser.add_argument("--ipc-port", type=int, default=int(os.environ.get("ZUSE_IPC_PORT", "27664")))
    args = parser.parse_args()
    if not 0 < args.ipc_port < 65536:
        parser.error("--ipc-port must be between 1 and 65535")
    env = dict(build_environment(), ZUSE_DATA_DIR=str(Path(args.data_dir).resolve()), ZUSE_IPC_PORT=str(args.ipc_port))
    # Use a separate local development profile unless the caller opts into sync.
    env.setdefault("ZUSE_WORKOS_CLIENT_ID", "")
    engine = ui = None
    build = ["cargo", "build", "--locked", "-p", "zuse"]
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)
    try:
        # Never attach a development UI to an unrelated engine on the same port.
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", args.ipc_port))
        seen = snapshot()
        if subprocess.call(build, cwd=ROOT, env=env):
            return 1
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"], cwd=ROOT, env=env))
        binary = str(Path(metadata["target_directory"]) / "debug/zuse")
        engine = subprocess.Popen([binary, "headless"], cwd=ROOT, env=env, start_new_session=True)
        deadline = time.monotonic() + 30
        while True:
            if engine.poll() is not None:
                raise RuntimeError("Development engine exited before opening IPC")
            try:
                with socket.create_connection(("127.0.0.1", args.ipc_port), timeout=0.2):
                    break
            except OSError:
                if time.monotonic() > deadline:
                    raise RuntimeError("Development engine did not open IPC within 30 seconds")
                time.sleep(0.1)
        ui = subprocess.Popen([binary], cwd=ROOT, env=env, start_new_session=True)
        print("Watching native sources and assets. UI reloads keep the engine running. Ctrl+C stops both.", flush=True)
        while engine.poll() is None:
            time.sleep(0.5)
            current = snapshot()
            if current == seen:
                continue
            # Coalesce editor saves; preserve later edits made during a build.
            time.sleep(0.25)
            seen = snapshot()
            print("Rebuilding native UI…", flush=True)
            if subprocess.call(build, cwd=ROOT, env=env):
                print("Build failed; keeping the current UI. Save a fix to retry.", flush=True)
                continue
            stop(ui)
            ui = subprocess.Popen([binary], cwd=ROOT, env=env, start_new_session=True)
        raise RuntimeError("Development engine exited")
    except KeyboardInterrupt:
        return 0
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Native development stopped: {error}", flush=True)
        return 1
    finally:
        stop(ui)
        stop(engine)


if __name__ == "__main__":
    raise SystemExit(main())
