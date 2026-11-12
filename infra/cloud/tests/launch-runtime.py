#!/usr/bin/env python3
"""A provider exec completion must not retire the supervised guest with its group."""
import ctypes
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[3]


def provider_keys():
    """Every ProviderId::api_key_env value, read from the Rust source of truth."""
    source = (Path(__file__).resolve().parents[3] / "crates/protocol/src/cloud_computers.rs").read_text()
    body = source[source.index("fn api_key_env"):]
    keys = re.findall(r'Self::\w+ => "([A-Z0-9_]+)"', body[:body.index("\n    }\n")])
    assert "PRIZED_API_KEY" in keys and len(keys) == len(set(keys)) >= 5, keys
    return keys


@unittest.skipUnless(sys.platform == 'linux' and shutil.which('setsid'), 'Linux provider process groups')
class RuntimeLaunchTest(unittest.TestCase):
    def test_runtime_survives_provider_exec_group_retirement(self):
        # Adopt/reap the fixture grandchild instead of leaving it to PID1.
        self.assertEqual(ctypes.CDLL(None).prctl(36, 1, 0, 0, 0), 0)
        with tempfile.TemporaryDirectory(prefix='zuse-launch-group-') as directory:
            root = Path(directory)
            for name in ('data', 'checkout', 'status'):
                (root / name).mkdir()
            launcher = root / 'launch-runtime'
            shutil.copyfile(ROOT / 'infra/cloud/launch-runtime.sh', launcher)
            launcher.chmod(0o700)
            binary = root / 'fixture-runtime'
            binary.write_text('''#!/usr/bin/python3
import json,os,signal,time
assert not any(k in os.environ for k in %r)
''' % ((*provider_keys(), 'ZUSE_WORKSPACE_RUNTIME_LOCK_FD'),) + '''with open(os.environ['ZUSE_TEST_PID_FILE'],'w') as f:
 json.dump({'pid':os.getpid(),'group':os.getpgrp()},f)
while True:time.sleep(1)
''')
            binary.chmod(0o700)
            boot = root / 'boot-token'; boot.write_text('isolated-fixture-boot')
            environment = dict(os.environ,
                ZUSE_CLOUD_WORKSPACE_ID='test-launch-only', ZUSE_API_URL='http://127.0.0.1',
                ZUSE_RUNTIME_GENERATION='1', ZUSE_STORAGE_INCARNATION_ID='test-disk',
                ZUSE_RUNTIME_BOOT_TOKEN_FILE=str(boot), ZUSE_RUNTIME_BINARY=str(binary),
                ZUSE_USER_DATA=str(root / 'data'), ZUSE_CLOUD_WORKSPACE_ROOT=str(root / 'checkout'),
                ZUSE_WORKSPACE_RUNTIME_STATUS_DIR=str(root / 'status'),
                ZUSE_TEST_PID_FILE=str(root / 'runtime-pid.json'), ZUSE_RUNTIME_PROVIDER='boat',
                ZUSE_RUNTIME_PERSISTENT_EXEC_CERTIFIED='1', ZUSE_WORKSPACE_RUNTIME_LOCK_FD='42',
                **{key: 'fixture-only' for key in provider_keys()})
            for name in ('ZUSE_HARNESS_ROOT','ZUSE_RUNTIME_RETAINED','ZUSE_RUNTIME_RESUME_OPERATION_ID'):
                environment.pop(name, None)
            process = subprocess.Popen([str(launcher)], env=environment, start_new_session=True,
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            pid = None
            try:
                _, error = process.communicate(timeout=5)
                self.assertEqual(process.returncode, 0, error.decode())
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    try:
                        identity = json.loads((root / 'runtime-pid.json').read_text())
                        pid = identity['pid']; break
                    except (OSError, ValueError):time.sleep(.01)
                self.assertIsNotNone(pid, 'Fixture runtime did not start')
                # The SDK retires only the original freshly created exec group.
                try:os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:pass
                time.sleep(.05)
                self.assertNotEqual(identity['group'], process.pid,
                    'The runtime inherited the provider command group')
                self.assertEqual(os.waitpid(pid, os.WNOHANG), (0, 0),
                    'Provider completion killed the supervised runtime')
            finally:
                if process.poll() is None:
                    process.kill(); process.wait(timeout=5)
                if pid is not None:
                    try:os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:pass
                    try:os.waitpid(pid, 0)
                    except ChildProcessError:pass

if __name__ == '__main__':unittest.main()
