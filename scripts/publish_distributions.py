#!/usr/bin/env python3
"""Add distribution assets without replacing existing versioned downloads."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

from distribution import REPOSITORY, release_version


def gh(*arguments):
    return subprocess.check_output(['gh', 'release', *arguments, '-R', REPOSITORY], text=True)


def missing_assets(tag, names):
    """Every release ships every format; name what is absent from `names`."""
    version = release_version(tag)
    suffixes = {
        'linux-x86_64': ('.tar.gz', '.deb', '.rpm'),
        'linux-aarch64': ('.tar.gz', '.deb', '.rpm'),
        'macos-aarch64': ('.tar.gz', '.dmg'),
        'macos-x86_64': ('.tar.gz', '.dmg'),
        'windows-x86_64': ('.zip', '-setup.exe'),
    }
    required = {'install.sh'} | {f'zuse-{version}-{target}{suffix}' for target, kinds in suffixes.items() for suffix in kinds}
    return sorted(required - set(names))


def require_complete(tag, names):
    missing = missing_assets(tag, names)
    if missing:
        raise ValueError('Release is missing: ' + ', '.join(missing))


def publish(tag, output, command=gh, complete=False):
    release_version(tag)
    output = output.resolve()
    assets = {asset['name'] for asset in json.loads(command('view', tag, '--json', 'assets'))['assets']}
    if complete:
        require_complete(tag, assets | {path.name for path in output.iterdir()})
    checksums = {}
    if 'SHA256SUMS' in assets:
        command('download', tag, '--pattern', 'SHA256SUMS', '--dir', str(output), '--clobber')
        for line in (output / 'SHA256SUMS').read_text().splitlines():
            digest, name = line.split(maxsplit=1)
            name = name.lstrip('*')
            if name in checksums or not re.fullmatch('[0-9a-f]{64}', digest):
                raise ValueError('Invalid published checksum list')
            checksums[name] = digest
    if set(checksums) - assets:
        raise ValueError('Published checksums reference missing assets')
    # Recover a previous interrupted upload, before its checksum update landed.
    with tempfile.TemporaryDirectory(prefix='zuse-published-') as temporary:
        for name in sorted(assets - {'SHA256SUMS'} - set(checksums)):
            if name != 'install.sh' and not re.fullmatch(r'zuse-[0-9A-Za-z._-]+', name):
                raise ValueError('Unexpected published asset name')
            command('download', tag, '--pattern', name, '--dir', temporary)
            checksums[name] = hashlib.sha256((Path(temporary) / name).read_bytes()).hexdigest()
    # Upload new versioned files first. Retries retain previously published bytes,
    # including signatures and compression timestamps from an earlier attempt.
    for path in sorted(output.glob('zuse-*')):
        if path.name in assets:
            print('Keeping published asset:', path.name)
            continue
        command('upload', tag, str(path))
        checksums[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
    installer = output / 'install.sh'
    digest = hashlib.sha256(installer.read_bytes()).hexdigest()
    command('upload', tag, str(installer), '--clobber')
    checksums[installer.name] = digest
    sums = output / 'SHA256SUMS'
    sums.write_text(''.join(f'{digest}  {name}\n' for name, digest in sorted(checksums.items())))
    command('upload', tag, str(sums), '--clobber')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('tag')
    parser.add_argument('output', type=Path)
    parser.add_argument('--check-only', action='store_true', help='Verify the directory holds every format and publish nothing')
    args = parser.parse_args()
    if args.check_only:
        require_complete(args.tag, [path.name for path in args.output.iterdir()])
    else:
        publish(args.tag, args.output, complete=True)
