"""Keep Cargo source timestamps only when cached file contents still match."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parent.parent
RECEIPT = Path(".context/ci-source-cache.json")


def sources(root):
    paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=root)
    for name in paths.decode().split("\0"):
        if not name:
            continue
        path = root / name
        if path.is_file() and not path.is_symlink():
            yield name, path


def save(root=ROOT):
    files = {}
    for name, path in sources(root):
        files[name] = {"sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                       "mtime_ns": path.stat().st_mtime_ns}
    receipt = root / RECEIPT
    receipt.parent.mkdir(parents=True, exist_ok=True)
    receipt.write_text(json.dumps({"version": 1, "files": files}))
    print(f"Recorded {len(files)} source fingerprints")


def restore(root=ROOT):
    try:
        receipt = json.loads((root / RECEIPT).read_text())
        if not isinstance(receipt, dict) or receipt.get("version") != 1 or not isinstance(receipt.get("files"), dict):
            raise ValueError("unsupported source cache")
    except (OSError, ValueError, TypeError):
        print("No usable source fingerprints; Cargo will rebuild normally")
        return 0
    restored = 0
    # Iterate current tracked files, never arbitrary paths from a cache receipt.
    for name, path in sources(root):
        previous = receipt["files"].get(name, {})
        timestamp = previous.get("mtime_ns") if isinstance(previous, dict) else None
        if (isinstance(timestamp, int) and timestamp > 0
                and hashlib.sha256(path.read_bytes()).hexdigest() == previous.get("sha256")):
            info = path.stat()
            os.utime(path, ns=(info.st_atime_ns, timestamp))
            restored += 1
    print(f"Restored timestamps for {restored} unchanged source files")
    return restored


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("save", "restore"))
    args = parser.parse_args()
    {"save": save, "restore": restore}[args.mode]()
