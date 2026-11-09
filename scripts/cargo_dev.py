#!/usr/bin/env python3
"""Run Cargo with a build cache shared by this repository's worktrees."""
import hashlib
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def icon_fingerprint(env):
    # Only this digest enters Cargo fingerprints, never the license itself.
    return hashlib.sha256(env.get("HUGEICONS_LICENSE_KEY", "").strip().encode()).hexdigest()


def target_directory(root, env):
    if env.get("CARGO_TARGET_DIR"):
        return Path(env["CARGO_TARGET_DIR"]).absolute()
    if env.get("CI"):
        return root / "target"
    common = subprocess.check_output(
        ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
        cwd=root, text=True,
    ).strip()
    identity = hashlib.sha256(os.fsencode(Path(common).resolve())).hexdigest()[:16]
    cache = Path(env.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    return cache / "zuse" / "cargo" / identity


def build_environment(root=ROOT, env=None):
    result = dict(os.environ if env is None else env)
    result["CARGO_TARGET_DIR"] = str(target_directory(root, result))
    result["HUGEICONS_BUILD_KEY"] = icon_fingerprint(result)
    return result


def main():
    args = sys.argv[1:]
    if args == ["--github-env"]:
        with open(os.environ["GITHUB_ENV"], "a") as output:
            output.write(f"HUGEICONS_BUILD_KEY={icon_fingerprint(os.environ)}\n")
        return
    env = build_environment()
    if args == ["--print-target-dir"]:
        print(env["CARGO_TARGET_DIR"])
    elif args == ["--print-icon-fingerprint"]:
        print(env["HUGEICONS_BUILD_KEY"])
    else:
        os.execvpe("cargo", ["cargo", *args], env)


if __name__ == "__main__":
    main()
