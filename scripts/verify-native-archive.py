#!/usr/bin/env python3
"""Verify and start a release executable on a runner without signing secrets."""
from pathlib import Path
import subprocess
import sys
import tempfile
from distribution import check_archive, unpack

archive = Path(sys.argv[1])
check_archive(archive, Path(sys.argv[2]))
with tempfile.TemporaryDirectory(prefix='zuse-startup-') as temporary:
    package, manifest = unpack(archive, Path(temporary))
    executable = package / {'macos': 'Zuse.app/Contents/MacOS/zuse',
                            'linux': 'libexec/zuse', 'windows': 'zuse.exe'}[manifest['platform']]
    subprocess.run([str(executable), '--noop-browser'], check=True, timeout=30)
    print(f'Archive verified and {manifest["platform"]}/{manifest["arch"]} executable startup passed.')
