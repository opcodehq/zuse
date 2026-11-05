#!/usr/bin/env python3
"""Exercise the real Windows setup executable, app startup and uninstallation."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

setup = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix='Zuse installer check ') as temporary:
    root = Path(temporary)
    install = root / 'application'
    profile = root / 'profile'
    profile.mkdir()
    (profile / 'keep').write_text('conversation data')
    environment = dict(os.environ, ZUSE_DATA_DIR=str(profile))
    subprocess.run([str(setup), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART',
                    f'/DIR={install}', f'/LOG={root / "setup.log"}'], check=True, timeout=120, env=environment)
    manifest = json.loads((install / 'manifest.json').read_text())
    for relative, digest in manifest['files'].items():
        path = install.joinpath(*relative.replace('\\', '/').split('/'))
        with path.open('rb') as stream:
            assert hashlib.file_digest(stream, 'sha256').hexdigest() == digest, relative
    subprocess.run([str(install / 'zuse.exe'), '--noop-browser'], check=True, timeout=30, env=environment)
    for helper in ('zuse-server', 'zuse-relay'):
        subprocess.run(['cmd.exe', '/d', '/c', str(install / 'bin' / f'{helper}.cmd'), '--help'],
                       check=True, timeout=30, env=environment)
    subprocess.run([str(install / 'unins000.exe'), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'],
                   check=True, timeout=120, env=environment)
    deadline = time.monotonic() + 20
    while (install / 'zuse.exe').exists() and time.monotonic() < deadline:
        time.sleep(0.2)
    assert not (install / 'zuse.exe').exists(), 'Uninstall left the app installed'
    assert (profile / 'keep').read_text() == 'conversation data'
print('Windows setup, inventory, app/helper startup, uninstall and profile preservation passed.')
