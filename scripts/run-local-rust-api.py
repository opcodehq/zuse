#!/usr/bin/env python3
"""Build and launch the Rust API against the private local staging database."""
import os
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
settings = root / ".context/android-tools/local-rust-api.env"
if not settings.is_file() or settings.stat().st_mode & 0o077:
    raise SystemExit("Create an owner-readable-only local-rust-api.env first")
env = os.environ.copy()
for line in settings.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and (name.startswith("ZUSE_CLOUD_") or name == "WORKOS_CLIENT_ID"):
        env[name] = value
if env.get("ZUSE_CLOUD_LISTEN") != "127.0.0.1:28700":
    raise SystemExit("Local staging API must listen on 127.0.0.1:28700")
subprocess.run(["cargo", "build", "-j", "2", "-p", "zuse-cloud-api"], cwd=root, check=True)
os.chdir(root)
os.execve(root / "target/debug/zuse-cloud-api", ["zuse-cloud-api", "serve"], env)
