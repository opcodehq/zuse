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


def directories(root, files):
    """Fingerprint fully tracked trees, including their entry names and types."""
    candidates = {str(parent) for name in files for parent in Path(name).parents
                  if str(parent) != "."}
    result = {}
    for name in sorted(candidates, key=lambda name: len(Path(name).parts), reverse=True):
        path = root / name
        if path.is_symlink():
            continue
        entries = []
        for child in sorted(path.iterdir()):
            key = str(child.relative_to(root))
            if child.is_symlink():
                break
            if key in files:
                entries.append((child.name, "file", files[key]["sha256"]))
            elif key in result:
                entries.append((child.name, "directory", result[key]["sha256"]))
            else:
                # Untracked/generated entries must retain their real timestamps.
                break
        else:
            result[name] = {
                "sha256": hashlib.sha256(json.dumps(entries).encode()).hexdigest(),
                "mtime_ns": path.stat().st_mtime_ns,
            }
    return result


def save(root=ROOT):
    files = {}
    for name, path in sources(root):
        files[name] = {"sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                       "mtime_ns": path.stat().st_mtime_ns}
    receipt = root / RECEIPT
    receipt.parent.mkdir(parents=True, exist_ok=True)
    receipt.write_text(json.dumps({"version": 1, "files": files,
                                   "directories": directories(root, files)}))
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
    current = {}
    # Iterate current tracked files, never arbitrary paths from a cache receipt.
    for name, path in sources(root):
        current[name] = {"sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        previous = receipt["files"].get(name, {})
        timestamp = previous.get("mtime_ns") if isinstance(previous, dict) else None
        if (isinstance(timestamp, int) and timestamp > 0
                and current[name]["sha256"] == previous.get("sha256")):
            info = path.stat()
            os.utime(path, ns=(info.st_atime_ns, timestamp))
            restored += 1
    # Legacy receipts cannot prove that an untracked directory input disappeared.
    # Restore directories only when their original entry fingerprints were saved.
    previous_directories = receipt.get("directories")
    if isinstance(previous_directories, dict):
        for name, fingerprint in directories(root, current).items():
            previous = previous_directories.get(name, {})
            if (isinstance(previous, dict) and fingerprint["sha256"] == previous.get("sha256")
                    and isinstance(previous.get("mtime_ns"), int) and previous["mtime_ns"] > 0):
                path = root / name
                os.utime(path, ns=(path.stat().st_atime_ns, previous["mtime_ns"]))
    print(f"Restored timestamps for {restored} unchanged source files")
    return restored


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("save", "restore"))
    args = parser.parse_args()
    {"save": save, "restore": restore}[args.mode]()
