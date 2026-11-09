#!/bin/sh
# Install a verified Zuse release for the current user. No sudo or profile edits.
set -eu
zuse_python=$(for candidate in python3 python3.13 python3.12 python3.11; do
  if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 11))' 2>/dev/null; then
    printf '%s' "$candidate"
    break
  fi
done)
if [ -z "$zuse_python" ]; then
  echo 'Python 3.11 or newer is required.' >&2
  exit 1
fi
exec "$zuse_python" - "$@" <<'PY'
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.request

REPOSITORY = 'opcodehq/zuse'
BASE = 'https://github.com/' + REPOSITORY + '/releases'


def read(url):
    request = urllib.request.Request(url, headers={'User-Agent': 'zuse-installer'})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def install(tag, download=read):
    if tag:
        tag = tag if tag.startswith('v') else 'v' + tag
        if not re.fullmatch(r'v\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?', tag):
            raise ValueError('Invalid release version')
    else:
        api = 'https://api.github.com/repos/' + REPOSITORY + '/releases'
        try:
            tag = json.loads(download(api + '/latest'))['tag_name']
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            releases = json.loads(download(api))
            published = [release for release in releases if not release['draft']]
            if not published:
                raise ValueError('No published Zuse releases found')
            tag = published[0]['tag_name']
        return install(tag, download)
    arch = {'x86_64': 'x86_64', 'aarch64': 'aarch64', 'arm64': 'aarch64'}.get(platform.machine())
    if platform.system() != 'Linux' or not arch:
        raise ValueError('This installer supports Linux x86_64 and ARM64')
    version = tag[1:]
    name = f'zuse-{version}-linux-{arch}'
    filename = name + '.tar.gz'
    checksums = download(f'{BASE}/download/{tag}/SHA256SUMS').decode()
    matches = [line.split()[0] for line in checksums.splitlines() if len(line.split()) == 2 and line.split()[1].lstrip('*') == filename]
    if len(matches) != 1 or not re.fullmatch('[0-9a-f]{64}', matches[0]):
        raise ValueError('Release has no unique checksum for this architecture')
    archive = download(f'{BASE}/download/{tag}/{filename}')
    if hashlib.sha256(archive).hexdigest() != matches[0]:
        raise ValueError('Downloaded package checksum does not match SHA256SUMS')
    with tempfile.TemporaryDirectory(prefix='zuse-install-') as temporary:
        root = Path(temporary)
        source = root / filename
        source.write_bytes(archive)
        seen = set()
        with tarfile.open(source) as bundle:
            for member in bundle.getmembers():
                path = PurePosixPath(member.name)
                if path.is_absolute() or '..' in path.parts or '\\' in member.name or not path.parts or path.parts[0] != name or str(path) in seen:
                    raise ValueError('Unsafe package path')
                seen.add(str(path))
                if not member.isfile() and not member.isdir():
                    raise ValueError('Package links and special files are not allowed')
            # All members are confined regular files/directories; no archive links.
            bundle.extractall(root)
        package = root / name
        manifest = json.loads((package / 'manifest.json').read_text())
        if manifest.get('version') != version or manifest.get('arch') != arch or manifest.get('platform') != 'linux' or manifest.get('format') != 1:
            raise ValueError('Package metadata does not match the requested release')
        expected = manifest['files']
        actual = {p.relative_to(package).as_posix(): p for p in package.rglob('*') if p.is_file()}
        if set(actual) != set(expected) | {'manifest.json'}:
            raise ValueError('Package inventory mismatch')
        for relative, digest in expected.items():
            if hashlib.sha256(actual[relative].read_bytes()).hexdigest() != digest:
                raise ValueError('Package file checksum mismatch: ' + relative)
        print(f'Installing verified Zuse {version} ({arch})', flush=True)
        subprocess.run([sys.executable, str(package / 'install.py')], check=True)
    bin_dir = Path(os.environ.get('ZUSE_BIN_HOME', Path.home() / '.local/bin')).expanduser()
    if str(bin_dir) not in os.environ.get('PATH', '').split(os.pathsep):
        print(f'Add {bin_dir} to your PATH, then run zuse.')
    else:
        print('Run zuse to start the app.')
    print('Linux runtime libraries and a working Vulkan driver are required; see the release notes.')


def uninstall():
    data = Path(os.environ.get('XDG_DATA_HOME', Path.home() / '.local/share')).expanduser().resolve()
    bin_dir = Path(os.environ.get('ZUSE_BIN_HOME', Path.home() / '.local/bin')).expanduser().resolve()
    packages = data / 'zuse/packages'
    installed = (bin_dir / 'zuse').resolve()
    try:
        relative = installed.relative_to(packages)
    except ValueError:
        raise ValueError('No Zuse-managed installation found; profiles are untouched') from None
    package = packages / relative.parts[0]
    if not (package / '.installed-by-zuse').is_file() or not (package / 'install.py').is_file():
        raise ValueError('No Zuse-managed installation found')
    subprocess.run([sys.executable, str(package / 'install.py'), '--uninstall'], check=True)


def main():
    if sys.version_info < (3, 11):
        raise ValueError('Python 3.11 or newer is required')
    parser = argparse.ArgumentParser(description='Install a checksum-verified Zuse release for the current Linux user.')
    parser.add_argument('--version', help='Version to install, for example 0.26.0-preview.1; defaults to latest stable, or latest preview if no stable release exists')
    parser.add_argument('--uninstall', action='store_true', help='Remove Zuse-owned launchers; preserve packages and profiles')
    args = parser.parse_args()
    if platform.system() != 'Linux':
        raise ValueError('This installer is for Linux')
    if args.uninstall:
        uninstall()
    else:
        install(args.version)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
PY
