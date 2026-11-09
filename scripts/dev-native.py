#!/usr/bin/env python3
"""Rebuild and relaunch the native UI while its development engine stays alive."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
IGNORED_DIRECTORIES = {"target", "tests", "benches", "examples", "docs", "node_modules",
                       ".git", ".context", "__pycache__", ".pytest_cache"}


def package_roots(metadata, package="zuse"):
    """Runtime/build dependency closure; test-only and unrelated workspace crates stay out."""
    packages = {Path(item["manifest_path"]).resolve().parent: item for item in metadata["packages"]}
    selected = next((directory for directory, item in packages.items() if item["name"] == package), None)
    if selected is None:
        raise RuntimeError(f"Cargo package {package} is missing")
    pending, visited = [selected], set()
    while pending:
        directory = pending.pop()
        if directory in visited:
            continue
        try:
            directory.relative_to(ROOT.resolve())
        except ValueError:
            continue
        visited.add(directory)
        for dependency in packages[directory].get("dependencies", []):
            if dependency.get("kind") == "dev" or not dependency.get("path"):
                continue
            child = Path(dependency["path"]).resolve()
            if child in packages:
                pending.append(child)
    return sorted(visited)


def snapshot(roots=None):
    files = [ROOT / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    files += [ROOT / "dist/macos/Info-unbundled.plist", ROOT / "dist/windows/zuse.rc", ROOT / "dist/windows/zuse.ico", ROOT / "scripts/prepare-icons.mjs"]
    roots = list(roots) if roots is not None else [ROOT / "crates", ROOT / "apps/desktop"]
    for root in [*roots, ROOT / ".cargo"]:
        for directory, directories, names in os.walk(root):
            directories[:] = [name for name in directories if name not in IGNORED_DIRECTORIES]
            for name in names:
                # Package documentation is not executable input. Embedded assets,
                # including Markdown prompts under src/assets, remain watched.
                if Path(directory) == root and name in {"README.md", "CHANGELOG.md", "LICENSE", "AGENTS.md", "CLAUDE.md"}:
                    continue
                if roots and root == ROOT / "crates" and name in {"README.md", "CHANGELOG.md", "AGENTS.md", "CLAUDE.md"}:
                    continue
                files.append(Path(directory) / name)
    result = {}
    for filename in files:
        try:
            stat = filename.stat()
            result[filename] = (stat.st_mtime_ns, stat.st_ctime_ns, stat.st_size, stat.st_ino)
        except FileNotFoundError:
            pass
    return result


class SourceWatcher:
    """Hash changed inputs so file-sync timestamp touches don't trigger a rebuild."""
    def __init__(self, roots):
        self.roots = roots
        self.cache = {}

    def scan(self):
        updated, content = {}, {}
        for filename, stamp in snapshot(self.roots).items():
            previous = self.cache.get(filename)
            try:
                digest = previous[1] if previous and previous[0] == stamp else hashlib.blake2b(filename.read_bytes(), digest_size=16).digest()
            except FileNotFoundError:
                continue
            updated[filename] = (stamp, digest)
            content[filename] = digest
        self.cache = updated
        return content


def cargo_build_command(full_debug=False, environment=None, packages=("zuse",)):
    environment = os.environ if environment is None else environment
    command = ["cargo"]
    if not full_debug:
        # Only first-party package overrides: changing the global profile or its
        # wildcard also changes proc-macro/build-tool profiles and can cascade
        # through the entire dependency graph.
        debug = environment.get("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
        value = debug if debug in {"0", "1", "2", "true", "false"} else json.dumps(debug)
        for package in sorted(set(packages)):
            command += ["--config", f'profile.dev.package.{json.dumps(package)}.debug={value}']
    return [*command, "build", "--locked", "-p", "zuse", "--timings"]


UI_ONLY_PACKAGES = ("crates/ui/", "crates/viewport/")


def ui_only_input(path):
    """True for inputs only the windowed UI compiles in; the engine never runs them."""
    relative = Path(path).resolve().as_posix()
    root = ROOT.resolve().as_posix() + "/"
    relative = relative[len(root):] if relative.startswith(root) else relative
    return relative.startswith(UI_ONLY_PACKAGES)


def watched_package_names(cargo, roots):
    roots = set(roots)
    return [item["name"] for item in cargo["packages"]
            if Path(item["manifest_path"]).resolve().parent in roots]


def metadata(environment):
    return json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"], cwd=ROOT, env=environment))


def build_environment(environment):
    environment = dict(environment)
    environment.setdefault("ZUSE_ICON_STYLE", "solid" if environment.get("HUGEICONS_LICENSE_KEY", "").strip() else "free")
    return environment


def binary_stamp(binary):
    try:
        info = Path(binary).stat()
        return info.st_mtime_ns, info.st_size
    except FileNotFoundError:
        return None


def build(command, environment, target):
    storage = target if target.exists() else ROOT
    free = shutil.disk_usage(storage).free
    if free < 1024 ** 3:
        print(f"Build paused: only {free / 1024 ** 3:.2f} GiB free on the build volume. Free space and retry; no automatic cache deletion.", flush=True)
        return False
    started = time.monotonic()
    success = subprocess.call(command, cwd=ROOT, env=environment) == 0
    print(f"Native build {'finished' if success else 'failed'} in {time.monotonic() - started:.1f}s.", flush=True)
    if not success and shutil.disk_usage(storage).free < 1024 ** 3:
        print("The build volume is almost full. Generated artifacts are under the Cargo target directory; avoid cargo clean if you want to keep dependency caches.", flush=True)
    return success


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
    parser.add_argument("--full-debug", action="store_true", help="Use the original full-debug Cargo profile for variable/type inspection")
    args = parser.parse_args()
    if not 0 < args.ipc_port < 65536:
        parser.error("--ipc-port must be between 1 and 65535")
    env = build_environment(dict(os.environ, ZUSE_DATA_DIR=str(Path(args.data_dir).resolve()), ZUSE_IPC_PORT=str(args.ipc_port)))
    # Disable optional workspace/relay auth by default. General account sign-in
    # has an independent public-client flow; an explicit staging client is kept.
    env.setdefault("ZUSE_WORKOS_CLIENT_ID", "")
    engine = ui = None
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)
    try:
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", args.ipc_port))
        cargo = metadata(env)
        roots = package_roots(cargo)
        command = cargo_build_command(args.full_debug, env, watched_package_names(cargo, roots))
        watcher = SourceWatcher(roots)
        seen = watcher.scan()
        target = Path(cargo["target_directory"])
        binary = str(target / "debug/zuse")
        if not build(command, env, target):
            return 1
        def start_engine():
            process = subprocess.Popen([binary, "headless"], cwd=ROOT, env=env, start_new_session=True)
            deadline = time.monotonic() + 30
            while True:
                if process.poll() is not None:
                    raise RuntimeError("Development engine exited before opening IPC")
                try:
                    with socket.create_connection(("127.0.0.1", args.ipc_port), timeout=0.2):
                        return process
                except OSError:
                    if time.monotonic() > deadline:
                        raise RuntimeError("Development engine did not open IPC within 30 seconds")
                    time.sleep(0.1)
        engine = start_engine()
        ui = subprocess.Popen([binary], cwd=ROOT, env=env, start_new_session=True)
        print(f"Watching {len(seen)} runtime/build inputs across {len(roots)} local packages. UI-only changes keep the engine running; engine changes restart it. Ctrl+C stops both.", flush=True)
        while engine.poll() is None:
            time.sleep(0.5)
            current = watcher.scan()
            if current == seen:
                continue
            time.sleep(0.25)
            current = watcher.scan()
            changed = {name for name in set(current) | set(seen) if current.get(name) != seen.get(name)}
            if any(name.name == "Cargo.toml" for name in changed):
                cargo = metadata(env)
                roots = package_roots(cargo)
                command = cargo_build_command(args.full_debug, env, watched_package_names(cargo, roots))
                watcher = SourceWatcher(roots)
            seen = watcher.scan()
            previous_binary = binary_stamp(binary)
            print(f"Rebuilding native UI ({len(changed)} changed input(s))…", flush=True)
            if not build(command, env, target):
                print("Build failed; keeping the current UI. Save a fix to retry.", flush=True)
                continue
            if binary_stamp(binary) == previous_binary:
                print("Executable unchanged; keeping the current UI.", flush=True)
                continue
            stop(ui)
            if any(not ui_only_input(name) for name in changed):
                # Engine code changed: a UI reload alone would keep running
                # the old engine.
                print("Restarting the development engine for engine changes…", flush=True)
                stop(engine)
                engine = start_engine()
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
