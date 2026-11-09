"""Check development artifact size before launcher builds; clean only on request."""
import argparse
import json
import os
import shlex
from pathlib import Path
import stat
import subprocess


ROOT = Path(__file__).resolve().parent.parent
DEV_CACHE_BUDGET = 20 * 1024**3


def directory_bytes(path):
    """Count regular files without following links outside the cache."""
    if not path.exists():
        return 0
    total = 0
    seen = set()

    def unreadable(error):
        raise error

    for directory, _, files in os.walk(path, onerror=unreadable):
        for name in files:
            try:
                info = (Path(directory) / name).lstat()
            except FileNotFoundError:
                continue  # Another build may have just replaced this artifact.
            identity = (info.st_dev, info.st_ino)
            if stat.S_ISREG(info.st_mode) and identity not in seen:
                seen.add(identity)
                total += info.st_size
    return total


def prepare(env=None, target=None, root=ROOT, budget=DEV_CACHE_BUDGET, clean=False):
    """Reject oversized dev caches; explicit cleanup retains release outputs."""
    if target is None:
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--offline", "--no-deps",
             "--format-version=1"], cwd=root, env=env))
        target = metadata["target_directory"]
    target = Path(target).resolve()
    size = directory_bytes(target / "debug")
    if size > budget:
        message = (f"Development build cache is {size / 1024**3:.1f} GiB "
                   f"(budget {budget / 1024**3:.1f} GiB): {target / 'debug'}.")
        if not clean:
            command = shlex.join(["python3", str(ROOT / "scripts/build_cache.py"),
                                  "--target-dir", str(target), "--clean"])
            raise RuntimeError(f"{message} Stop builds using this target, then run: {command}")
        print(f"{message} Cleaning development artifacts.", flush=True)
        # Cargo validates its cache marker before removing generated files.
        # Cleanup is explicit: cargo clean does not take the build artifact lock.
        subprocess.run(["cargo", "clean", "--locked", "--offline", "--profile", "dev",
                        "--target-dir", str(target)], cwd=root, env=env, check=True)
    return target


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--clean", action="store_true",
                        help="clean an oversized dev cache; stop concurrent builds first")
    args = parser.parse_args()
    try:
        from cargo_dev import build_environment
        prepare(env=build_environment(), target=args.target_dir, clean=args.clean)
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"{error}\n")
