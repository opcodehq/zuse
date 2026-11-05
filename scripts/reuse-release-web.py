#!/usr/bin/env python3
"""Reuse checksum-verified browser assets from the matching published release."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from distribution import check_archive, release_version, unpack

version = release_version(sys.argv[1])
filename = f'zuse-{version}-linux-x86_64.tar.gz'
incoming = Path('incoming')
subprocess.run(['gh', 'release', 'download', sys.argv[1], '-R', 'opcodehq/zuse', '--pattern', filename, '--dir', str(incoming)], check=True)
check_archive(incoming / filename, incoming / 'SHA256SUMS')
with tempfile.TemporaryDirectory(prefix='zuse-web-') as temporary:
    package, manifest = unpack(incoming / filename, Path(temporary))
    if manifest['version'] != version or manifest['platform'] != 'linux':
        raise ValueError('Unexpected browser asset source')
    shutil.copytree(package / 'libexec/web', Path('source/dist/web'))
(incoming / filename).unlink()
