#!/usr/bin/env python3
"""Verify a standalone CLI archive, then serve its web files from a fresh profile.

Usage: verify-cli-archive.py ARCHIVE
"""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import queue
import subprocess
import sys
import tarfile
import tempfile
import threading
import urllib.request
import zipfile


def unpack(archive, root):
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as bundle:
            for name in bundle.namelist():
                if PurePosixPath(name).is_absolute() or ".." in PurePosixPath(name).parts:
                    raise ValueError(f"Unsafe archive path: {name}")
            bundle.extractall(root)
    else:
        with tarfile.open(archive) as bundle:
            for member in bundle.getmembers():
                path = PurePosixPath(member.name)
                if path.is_absolute() or ".." in path.parts:
                    raise ValueError(f"Unsafe archive path: {member.name}")
                # The only link is bin/zuse-server, next to its target.
                if member.issym() and (path.parts[1:] != ("bin", "zuse-server") or member.linkname != "zuse"):
                    raise ValueError(f"Unexpected archive link: {member.name}")
                if not (member.isfile() or member.isdir() or member.issym()):
                    raise ValueError(f"Special file in archive: {member.name}")
            bundle.extractall(root, filter="tar")
    (package,) = Path(root).iterdir()
    return package


def verify(package):
    manifest = json.loads((package / "manifest.json").read_text())
    if manifest.get("kind") != "cli" or package.name != (
            f"zuse-cli-{manifest['version']}-{manifest['platform']}-{manifest['arch']}"):
        raise ValueError("Archive directory and manifest disagree")
    links = manifest.get("links", {})
    actual = {path.relative_to(package).as_posix() for path in package.rglob("*")
              if path.is_file() and not path.is_symlink()}
    if actual != set(manifest["files"]) | {"manifest.json"}:
        raise ValueError("Archive inventory differs from its manifest")
    for relative, digest in manifest["files"].items():
        with (package / relative).open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != digest:
                raise ValueError(f"Checksum mismatch: {relative}")
    for relative, target in links.items():
        if os.readlink(package / relative) != target:
            raise ValueError(f"Link changed: {relative}")
    return manifest


def run(*command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=60)
    if result.returncode != 0:
        raise ValueError(f"{command} failed:\n{result.stdout}{result.stderr}")
    return result.stdout


def serve(zuse, profile):
    """Start `zuse serve` outside any checkout and fetch the page it serves."""
    environment = {key: value for key, value in os.environ.items()
                   if key not in ("ZUSE_WEB_ASSETS", "ZUSE_BROWSER_TOKEN", "ZUSE_PUBLIC_ORIGIN",
                                  "ZUSE_TLS_CERT", "ZUSE_TLS_KEY", "ZUSE_DATA_DIR")}
    child = subprocess.Popen([str(zuse), "--language", "en", "serve", "--listen", "127.0.0.1:0",
                              "--ipc-port", "0", "--data-dir", str(profile)],
                             cwd=profile.parent, env=environment, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
    lines = queue.Queue()
    threading.Thread(target=lambda: [lines.put(line) for line in child.stdout], daemon=True).start()
    try:
        while True:
            line = lines.get(timeout=60)
            if line.startswith("Open "):
                origin = line.removeprefix("Open ").split("/#token=")[0]
                break
        with urllib.request.urlopen(origin + "/", timeout=30) as response:
            return response.read()
    finally:
        child.terminate()
        child.wait(timeout=30)


def main():
    archive = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="zuse-cli-check-") as temporary:
        root = Path(temporary)
        (root / "archive").mkdir()
        package = unpack(archive, root / "archive")
        manifest = verify(package)
        windows = manifest["platform"] == "windows"
        zuse = package / "bin" / ("zuse.exe" if windows else "zuse")
        if manifest["platform"] == {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}.get(platform.system()):
            run(str(zuse), "--version")
            if "serve" not in run(str(zuse), "--language", "en", "--help"):
                raise ValueError("zuse --help does not list serve")
            server = (["cmd.exe", "/d", "/c", str(package / "bin/zuse-server.cmd")] if windows
                      else [str(package / "bin/zuse-server")])
            if "Usage: zuse-server" not in run(*server, "--language", "en", "--help"):
                raise ValueError("zuse-server lost its old interface")
            page = serve(zuse, root / "profile")
            if page != (package / "share/zuse/web/index.html").read_bytes():
                raise ValueError("zuse serve did not serve the packaged web files")
    print(f"CLI archive verified; zuse serve found its packaged web files ({archive.name}).")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, queue.Empty) as error:
        raise SystemExit(str(error)) from error
