#!/usr/bin/env python3
"""Build or assemble a relocatable Zuse package using only Python's stdlib."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile
import zipfile
try:
    import tomllib
except ModuleNotFoundError:
    raise SystemExit("Packaging requires Python 3.11 or newer") from None

from distribution import retrying

ROOT = Path(__file__).resolve().parent.parent
BINS = ("zuse", "zuse-server", "zuse-relay")
# zuse-server is the standalone command, which cargo names zuse-cli beside the
# desktop app's zuse. Under its old name it keeps the old interface.
CARGO_NAMES = {"zuse-server": "zuse-cli"}


def built(name):
    """The cargo executable a packaged name comes from."""
    return CARGO_NAMES.get(name, name)
HOST_SYSTEMS = {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}
HOST_ARCHES = {"x86_64": "x86_64", "AMD64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64", "ARM64": "aarch64"}


def toolkit_source():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
    revision = workspace["dependencies"]["gpui"]["rev"]
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    candidates = [ROOT / "vendor/zkit", *cargo_home.glob("git/checkouts/zkit-*/*")]
    for candidate in candidates:
        if not (candidate / "NOTICE").is_file():
            continue
        result = subprocess.run(["git", "-C", str(candidate), "rev-parse", "HEAD"],
                                capture_output=True, text=True, check=False)
        if result.returncode == 0 and result.stdout.strip() == revision:
            return candidate
    raise ValueError("Pinned toolkit legal source is missing; run cargo fetch --locked first")


def bash():
    """Git Bash on Windows, never the System32 WSL launcher that Windows
    process creation would otherwise pick for a bare "bash"."""
    if platform.system() != "Windows":
        return "bash"
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        candidate = Path(directory) / "bash.exe"
        if candidate.is_file() and "system32" not in str(candidate).lower():
            return str(candidate)
    git = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "Git/bin/bash.exe"
    if git.is_file():
        return str(git)
    raise ValueError("Git Bash is required to build the browser assets on Windows")


def binary_target(path):
    with path.open("rb") as stream:
        header = stream.read(32)
    if len(header) < 32:
        raise ValueError(f"Truncated executable: {path}")
    if header[:6] == b"\x7fELF\x02\x01":
        return "linux", {62: "x86_64", 183: "aarch64"}.get(struct.unpack_from("<H", header, 18)[0])
    if header[:4] == b"\xcf\xfa\xed\xfe":
        return "macos", {0x1000007: "x86_64", 0x100000C: "aarch64"}.get(struct.unpack_from("<I", header, 4)[0])
    if header[:2] == b"MZ":
        with path.open("rb") as stream:
            stream.seek(0x3C)
            offset = struct.unpack("<I", stream.read(4))[0]
            stream.seek(offset)
            signature = stream.read(6)
        if len(signature) == 6 and signature[:4] == b"PE\0\0":
            return "windows", {0x8664: "x86_64", 0xAA64: "aarch64"}.get(struct.unpack_from("<H", signature, 4)[0])
    raise ValueError(f"Unsupported executable format: {path}")


def write_executable(path, source):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source)
    path.chmod(0o755)


def launcher(binary, macos=False):
    # These scripts live in bin/; all paths resolve relative to the package,
    # including when invoked through the installer's absolute symlink.
    base = '"$root/Zuse.app/Contents/MacOS"' if macos else '"$root/libexec"'
    assets = '"$root/Zuse.app/Contents/Resources/web"' if macos else '"$root/libexec/web"'
    script = '''#!/bin/sh
set -eu
script=$0
while [ -L "$script" ]; do
  link=$(readlink "$script")
  case "$link" in /*) script=$link ;; *) script=$(dirname "$script")/$link ;; esac
done
root=$(CDPATH= cd -- "$(dirname -- "$script")/.." && pwd)
'''
    script += f"binary_dir={base}\n"
    if binary != "zuse":
        script += f"assets={assets}\n"
        script += '''supplied=false
for argument in "$@"; do
  case "$argument" in --assets|--assets=*) supplied=true ;; esac
done
if [ "$supplied" = false ]; then set -- --assets "${ZUSE_WEB_ASSETS:-$assets}" "$@"; fi
'''
    script += f'exec "$binary_dir/{binary}" "$@"\n'
    return script


def windows_launcher(binary):
    # bin\<name>.cmd supplies the packaged web assets unless the caller passes
    # --assets, mirroring the POSIX launchers.
    return (
        "@echo off\r\n"
        "setlocal\r\n"
        'set "root=%~dp0.."\r\n'
        'set "assets=%ZUSE_WEB_ASSETS%"\r\n'
        'if not defined assets set "assets=%root%\\web"\r\n'
        "for %%A in (%*) do (\r\n"
        '  if "%%~A"=="--assets" goto supplied\r\n'
        '  echo %%~A| findstr /b /c:"--assets=" >nul && goto supplied\r\n'
        ")\r\n"
        f'"%root%\\libexec\\{binary}.exe" --assets "%assets%" %*\r\n'
        "exit /b %ERRORLEVEL%\r\n"
        ":supplied\r\n"
        f'"%root%\\libexec\\{binary}.exe" %*\r\n'
        "exit /b %ERRORLEVEL%\r\n"
    )


def copy_tree(source, destination):
    # Never follow a build asset symlink into a private profile or host path.
    for path in source.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"Package inputs must not contain symlinks: {path}")
    shutil.copytree(source, destination)


def sign_macos(app, run=subprocess.run):
    """Sign the app with a Developer ID and, when Apple credentials are
    present, notarize it and staple the ticket. Does nothing without
    ZUSE_MACOS_SIGN_IDENTITY, so local and preview builds stay unsigned."""
    identity = os.environ.get("ZUSE_MACOS_SIGN_IDENTITY")
    if not identity:
        return False
    keychain = os.environ.get("ZUSE_MACOS_KEYCHAIN")
    apple = retrying(run)
    base = ["codesign", "--force", "--timestamp", "--options", "runtime", "--sign", identity]
    if keychain:
        base += ["--keychain", keychain]
    # Helpers first, then the bundle, whose seal covers the main executable.
    for name in BINS:
        if name != "zuse":
            apple(base + [str(app / "Contents/MacOS" / name)], check=True)
    apple(base + ["--entitlements", str(ROOT / "dist/macos/Zuse.entitlements"), str(app)], check=True)
    run(["codesign", "--verify", "--strict", "--verbose=2", str(app)], check=True)
    apple_id, password, team = (os.environ.get(key) for key in (
        "APPLE_ID", "APPLE_APP_SPECIFIC_PASSWORD", "APPLE_TEAM_ID"))
    if not (apple_id and password and team):
        return True
    with tempfile.TemporaryDirectory(prefix="zuse-notarize-") as temporary:
        upload = Path(temporary) / "Zuse.zip"
        run(["ditto", "-c", "-k", "--keepParent", str(app), str(upload)], check=True)
        submitted = apple(["xcrun", "notarytool", "submit", str(upload), "--apple-id", apple_id,
                           "--password", password, "--team-id", team, "--wait",
                           "--output-format", "json"], check=True, capture_output=True, text=True)
        result = json.loads(submitted.stdout)
        if result.get("status") != "Accepted":
            run(["xcrun", "notarytool", "log", result.get("id", ""), "--apple-id", apple_id,
                 "--password", password, "--team-id", team], check=False)
            raise ValueError(f"Notarization was not accepted: {result.get('status')}")
    apple(["xcrun", "stapler", "staple", str(app)], check=True)
    apple(["xcrun", "stapler", "validate", str(app)], check=True)
    apple(["spctl", "--assess", "--type", "execute", "--verbose=2", str(app)], check=True)
    return True


def assemble(destination, binaries, assets, system, arch, version, provenance):
    for binary in binaries:
        if binary_target(binary) != (system, arch):
            raise ValueError(f"Executable target does not match {system}/{arch}: {binary}")
    if not (assets / "index.html").is_file() or not any(assets.rglob("*.wasm")):
        raise ValueError("Browser assets must contain index.html and a built WASM module")
    destination.mkdir()
    macos = system == "macos"
    resources = destination / "Zuse.app/Contents/Resources" if macos else destination / "share/zuse"
    executable_dir = destination / "Zuse.app/Contents/MacOS" if macos else destination / "libexec"
    executable_dir.mkdir(parents=True)
    resources.mkdir(parents=True)
    windows = system == "windows"
    for name, source in zip(BINS, binaries):
        if windows:
            # The desktop app sits at the top for double-clicking; helpers take
            # --assets through their bin\*.cmd launchers.
            target = destination / "zuse.exe" if name == "zuse" else executable_dir / f"{name}.exe"
            shutil.copyfile(source, target)
            if name != "zuse":
                (destination / "bin").mkdir(exist_ok=True)
                (destination / "bin" / f"{name}.cmd").write_bytes(windows_launcher(name).encode())
            continue
        target = executable_dir / name
        shutil.copyfile(source, target)
        target.chmod(0o755)
        write_executable(destination / "bin" / name, launcher(name, macos))
    # Each layout keeps web/ where the desktop app's asset discovery looks:
    # beside the executable, or ../Resources/web inside the macOS bundle.
    copy_tree(assets, resources / "web" if macos else destination / "web" if windows else executable_dir / "web")
    if macos:
        info = plistlib.loads((ROOT / "dist/macos/Info.plist").read_bytes())
        # Apple bundle versions remain numeric; the archive and manifest retain
        # the full preview version.
        info["CFBundleShortVersionString"] = version.split("-", 1)[0]
        info["CFBundleVersion"] = version.split("-", 1)[0]
        (destination / "Zuse.app/Contents/Info.plist").write_bytes(plistlib.dumps(info))
        png = (ROOT / "dist/app-icon.png").read_bytes()
        if png[:8] != b"\x89PNG\r\n\x1a\n" or struct.unpack_from(">II", png, 16) != (1024, 1024):
            raise ValueError("The macOS app icon must be a 1024px PNG")
        # The standard ic10 element contains a full 1024px PNG representation.
        icon = b"ic10" + struct.pack(">I", len(png) + 8) + png
        (resources / "zuse.icns").write_bytes(b"icns" + struct.pack(">I", len(icon) + 8) + icon)
    elif not windows:
        icons = destination / "share/icons/hicolor/1024x1024/apps"
        icons.mkdir(parents=True)
        shutil.copyfile(ROOT / "dist/app-icon.png", icons / "sh.zuse.app.png")
        shutil.copyfile(ROOT / "scripts/install-linux.py", destination / "install.py")
    copy_legal(resources / "legal")
    if macos:
        # Everything inside the app must be in place before it is sealed.
        sign_macos(destination / "Zuse.app")
    (destination / "docs").mkdir()
    shutil.copyfile(ROOT / "internal-docs/development.md", destination / "docs/development.md")
    readme = (ROOT / "internal-docs/packaging.md").read_text().replace("(development.md)", "(docs/development.md)")
    (destination / "README.md").write_text(readme)
    write_manifest(destination, {"format": 1, "version": version, "platform": system, "arch": arch,
                                 "provenance": provenance})


def copy_legal(legal):
    # Preserve the relative locations used by the root notices, including fonts,
    # embedded bridge code, and the toolkit's source notices.
    files = [(ROOT / path, Path(path)) for path in (
        "LICENSE", "THIRD_PARTY_NOTICES.md", "Cargo.lock", "internal-docs/third-party-native-license.txt")]
    for tree, prefix in ((ROOT / "crates", Path("crates")), (toolkit_source(), Path("vendor/zkit"))):
        for path in tree.rglob("*"):
            if path.is_file() and not path.is_symlink() and (
                path.name.upper().startswith(("LICENSE", "NOTICE", "THIRD_PARTY", "OFL"))
                or path.suffix.lower() == ".txt" and "licenses" in path.parts
            ):
                files.append((path, prefix / path.relative_to(tree)))
    for source, relative in sorted(set(files)):
        target = legal / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)


def write_manifest(destination, manifest):
    """Checksum every file; symlinks are listed with their targets instead."""
    manifest["files"] = {}
    for path in sorted(destination.rglob("*")):
        relative = path.relative_to(destination).as_posix()
        if path.is_symlink():
            manifest.setdefault("links", {})[relative] = os.readlink(path)
        elif path.is_file():
            with path.open("rb") as stream:
                manifest["files"][relative] = hashlib.file_digest(stream, "sha256").hexdigest()
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


def cli_server_launcher():
    # A .cmd cannot rename the executable, so it names the old interface.
    return '@"%~dp0zuse.exe" --as-zuse-server %*\r\n'


def sign_macos_binary(binary, run=subprocess.run):
    """Sign a bare executable and, with Apple credentials, notarize it. A bare
    binary cannot be stapled; Gatekeeper looks the ticket up online."""
    identity = os.environ.get("ZUSE_MACOS_SIGN_IDENTITY")
    if not identity:
        return False
    keychain = os.environ.get("ZUSE_MACOS_KEYCHAIN")
    apple = retrying(run)
    command = ["codesign", "--force", "--timestamp", "--options", "runtime", "--sign", identity]
    if keychain:
        command += ["--keychain", keychain]
    apple(command + [str(binary)], check=True)
    run(["codesign", "--verify", "--strict", "--verbose=2", str(binary)], check=True)
    apple_id, password, team = (os.environ.get(key) for key in (
        "APPLE_ID", "APPLE_APP_SPECIFIC_PASSWORD", "APPLE_TEAM_ID"))
    if not (apple_id and password and team):
        return True
    with tempfile.TemporaryDirectory(prefix="zuse-notarize-") as temporary:
        upload = Path(temporary) / "zuse.zip"
        run(["ditto", "-c", "-k", "--keepParent", str(binary), str(upload)], check=True)
        submitted = apple(["xcrun", "notarytool", "submit", str(upload), "--apple-id", apple_id,
                           "--password", password, "--team-id", team, "--wait",
                           "--output-format", "json"], check=True, capture_output=True, text=True)
        result = json.loads(submitted.stdout)
        if result.get("status") != "Accepted":
            raise ValueError(f"Notarization was not accepted: {result.get('status')}")
    return True


def assemble_cli(destination, binary, assets, system, arch, version, provenance):
    """The standalone command for machines without the desktop app: bin/zuse,
    bin/zuse-server for the old interface, and share/zuse/web, where `zuse
    serve` looks for the browser client."""
    if binary_target(binary) != (system, arch):
        raise ValueError(f"Executable target does not match {system}/{arch}: {binary}")
    if not (assets / "index.html").is_file() or not any(assets.rglob("*.wasm")):
        raise ValueError("Browser assets must contain index.html and a built WASM module")
    destination.mkdir()
    bin_dir = destination / "bin"
    bin_dir.mkdir()
    if system == "windows":
        shutil.copyfile(binary, bin_dir / "zuse.exe")
        (bin_dir / "zuse-server.cmd").write_bytes(cli_server_launcher().encode())
    else:
        shutil.copyfile(binary, bin_dir / "zuse")
        (bin_dir / "zuse").chmod(0o755)
        (bin_dir / "zuse-server").symlink_to("zuse")
    share = destination / "share/zuse"
    copy_tree(assets, share / "web")
    copy_legal(share / "legal")
    if system == "macos":
        sign_macos_binary(bin_dir / "zuse")
    readme = (ROOT / "internal-docs/packaging.md").read_text().replace(
        "(development.md)", "(https://github.com/opcodehq/zuse/blob/main/internal-docs/development.md)")
    (destination / "README.md").write_text(readme)
    write_manifest(destination, {"format": 1, "kind": "cli", "version": version, "platform": system,
                                 "arch": arch, "provenance": provenance})


def write_archive(stage, name, archive):
    """Write `stage` as `archive` under the top-level directory `name`."""
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as bundle:
            for path in sorted(stage.rglob("*")):
                bundle.write(path, Path(name) / path.relative_to(stage))
    else:
        with tarfile.open(archive, "w:gz", compresslevel=6) as bundle:
            bundle.add(stage, arcname=name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("linux", "macos", "windows"))
    parser.add_argument("--arch", choices=("x86_64", "aarch64"))
    parser.add_argument("--profile", choices=("release", "dev"), default="release")
    parser.add_argument("--version", help="Release version, optionally with a prerelease suffix")
    parser.add_argument("--binaries", type=Path, help="Assemble prebuilt executables from this directory; skips all builds")
    parser.add_argument("--assets", type=Path, default=ROOT / "dist/web")
    parser.add_argument("--output", type=Path, default=ROOT / "dist/build")
    # Release jobs build without signing credentials, then sign in a separate
    # step that runs no dependency build code.
    stage = parser.add_mutually_exclusive_group()
    stage.add_argument("--build-only", action="store_true", help="Build the web assets and executables, then stop")
    stage.add_argument("--skip-build", action="store_true", help="Assemble what --build-only produced in this checkout")
    parser.add_argument("--skip-web", action="store_true", help="Use the browser assets already in dist/web")
    args = parser.parse_args()
    workspace_version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    version = args.version or workspace_version
    if not re.fullmatch(re.escape(workspace_version) + r"(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?", version):
        parser.error("Release version must match the workspace version, with an optional prerelease suffix")
    system = args.platform or HOST_SYSTEMS.get(platform.system())
    arch = args.arch or HOST_ARCHES.get(platform.machine())
    if not system or not arch:
        parser.error("Unsupported host; specify --platform and --arch for prebuilt inputs")
    if args.binaries is None:
        host = (HOST_SYSTEMS.get(platform.system()), HOST_ARCHES.get(platform.machine()))
        if (system, arch) != host:
            parser.error("Build on the target host, or supply --binaries for an existing target build")
        if args.assets.resolve() != (ROOT / "dist/web").resolve():
            parser.error("Default builds produce dist/web; use --binaries to assemble other assets")
        if not args.skip_build:
            if not args.skip_web:
                subprocess.run([bash(), "scripts/build-web.sh", "--release"], cwd=ROOT, check=True)
            subprocess.run(["cargo", "+stable", "build", "--locked", "--profile", args.profile,
                            "-p", "zuse", "-p", "zuse-cli", "-p", "zuse-relay", "--bins"], cwd=ROOT, check=True)
        if args.build_only:
            return
        target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
        if not target.is_absolute():
            target = ROOT / target
        if os.environ.get("CARGO_BUILD_TARGET"):
            target /= os.environ["CARGO_BUILD_TARGET"]
        args.binaries = target / ("debug" if args.profile == "dev" else "release")
        provenance = {"inputs": "built-in-checkout", "profile": args.profile}
        revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT,
                                  capture_output=True, text=True, check=True)
        dirty = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT,
                               capture_output=True, text=True, check=True)
        provenance.update(source_revision=revision.stdout.strip(), working_tree_dirty=bool(dirty.stdout))
    else:
        provenance = {"inputs": "prebuilt", "profile": "unspecified"}
    name = f"zuse-{version}-{system}-{arch}"
    cli_name = f"zuse-cli-{version}-{system}-{arch}"
    args.output.mkdir(parents=True, exist_ok=True)
    # Windows users expect a zip; Explorer opens it without extra tools.
    extension = ".zip" if system == "windows" else ".tar.gz"
    archive, cli_archive = (args.output / f"{base}{extension}" for base in (name, cli_name))
    for existing in (archive, cli_archive):
        if existing.exists():
            parser.error(f"Refusing to replace {existing}; choose a new --output directory")
    # A failure never publishes a half-assembled archive.
    with tempfile.TemporaryDirectory(prefix=".zuse-package-", dir=args.output) as temporary:
        suffix = ".exe" if system == "windows" else ""
        executables = {binary: args.binaries / f"{built(binary)}{suffix}" for binary in BINS}
        assemble(Path(temporary) / name, list(executables.values()), args.assets,
                 system, arch, version, provenance)
        assemble_cli(Path(temporary) / cli_name, executables["zuse-server"], args.assets,
                     system, arch, version, provenance)
        for base, target in ((name, archive), (cli_name, cli_archive)):
            pending = Path(temporary) / target.name
            write_archive(Path(temporary) / base, base, pending)
        for target in (archive, cli_archive):
            os.replace(Path(temporary) / target.name, target)
    print(archive.resolve())
    print(cli_archive.resolve())


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
