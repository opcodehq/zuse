#!/usr/bin/env python3
"""Install this extracted Linux package for the current user; never alter profiles."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import tempfile
import uuid

NAMES = ("zuse", "zuse-server", "zuse-relay")


def verify(package):
    manifest = json.loads((package / "manifest.json").read_text())
    if manifest.get("format") != 1 or manifest.get("platform") != "linux":
        raise ValueError("Expected a Linux Zuse package")
    listed = set(manifest["files"]) | {"manifest.json"}
    actual = {str(path.relative_to(package)) for path in package.rglob("*") if path.is_file()}
    if actual != listed or any(path.is_symlink() for path in package.rglob("*")):
        raise ValueError("Package file inventory changed or contains symlinks")
    for relative, expected in manifest["files"].items():
        path = Path(relative)
        if path.is_absolute() or ".." in path.parts:
            raise ValueError("Invalid package path")
        with (package / path).open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != expected:
                raise ValueError(f"Package checksum mismatch: {relative}")
    return manifest


def owned_link(path, packages, relative):
    if not path.is_symlink():
        return False
    target = path.resolve()
    try:
        tail = target.relative_to(packages)
        owner = packages / tail.parts[0]
        return tail.parts[1:] == Path(relative).parts and (owner / ".installed-by-zuse").is_file()
    except (ValueError, IndexError):
        return False


def desktop_entry(destination):
    # Desktop Exec has its own quoting and %-field syntax, not shell syntax.
    executable = str(destination / "bin/zuse").replace("%", "%%")
    for character in ("\\", '"', "`", "$"):
        executable = executable.replace(character, "\\" + character)
    executable = executable.replace("\\", "\\\\")
    return ("[Desktop Entry]\nType=Application\nName=Zuse\nComment=Local coding agents\n"
            f'Exec="{executable}" %u\nIcon=sh.zuse.app\nTerminal=false\n'
            "Categories=Development;\nMimeType=x-scheme-handler/zuse;\n")


def links_for(data, bin_dir):
    return [(bin_dir / name, f"bin/{name}") for name in NAMES] + [
        (data / "applications/sh.zuse.app.desktop", "share/applications/sh.zuse.app.desktop"),
        (data / "icons/hicolor/1024x1024/apps/sh.zuse.app.png", "share/icons/hicolor/1024x1024/apps/sh.zuse.app.png"),
    ]


def install(source, data, bin_dir):
    if any(ord(character) < 32 for character in str(data) + str(bin_dir) + source.name):
        raise ValueError("Install locations must not contain control characters")
    manifest = verify(source)
    architecture = {"x86_64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}.get(platform.machine())
    if platform.system() != "Linux" or architecture != manifest.get("arch"):
        raise ValueError("This package does not match this Linux machine's architecture")
    packages = data / "zuse/packages"
    links = links_for(data, bin_dir)
    for link, relative in links:
        if os.path.lexists(link) and not owned_link(link, packages, relative):
            raise ValueError(f"Refusing to replace a file not installed by Zuse: {link}")
    packages.mkdir(parents=True, exist_ok=True)
    digest = hashlib.sha256((source / "manifest.json").read_bytes()).hexdigest()[:16]
    destination = packages / f"{source.name}-{digest}"
    if destination.exists():
        # An existing install may have local edits; never silently reuse it.
        raise ValueError(f"Package is already installed: {destination}")
    with tempfile.TemporaryDirectory(prefix=".install-", dir=packages) as temporary:
        staged = Path(temporary) / "package"
        shutil.copytree(source, staged)
        verify(staged)
        applications = staged / "share/applications"
        applications.mkdir(parents=True, exist_ok=True)
        (applications / "sh.zuse.app.desktop").write_text(desktop_entry(destination))
        (staged / ".installed-by-zuse").write_text("1\n")
        os.rename(staged, destination)
    for link, relative in links:
        link.parent.mkdir(parents=True, exist_ok=True)
        pending = link.with_name(f".zuse-{uuid.uuid4().hex}")
        try:
            pending.symlink_to(destination / relative)
            os.replace(pending, link)
        finally:
            pending.unlink(missing_ok=True)
    print(f"Installed at {destination}\nLaunch: {bin_dir / 'zuse'}")
    print("Node.js 22+ and the selected agent's CLI/login are required for provider execution.")
    print("Previous package versions and all user profiles are preserved.")


def uninstall(data, bin_dir):
    packages = data / "zuse/packages"
    removed = 0
    for link, relative in links_for(data, bin_dir):
        if owned_link(link, packages, relative):
            link.unlink()
            removed += 1
    print(f"Removed {removed} launch/desktop links. Profiles and package files are preserved.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--uninstall", action="store_true", help="Remove only Zuse-owned launch/desktop links")
    args = parser.parse_args()
    data = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share")).expanduser().resolve()
    bin_dir = Path(os.environ.get("ZUSE_BIN_HOME", Path.home() / ".local/bin")).expanduser().resolve()
    if args.uninstall:
        uninstall(data, bin_dir)
    else:
        install(Path(__file__).resolve().parent, data, bin_dir)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        raise SystemExit(str(error)) from error
