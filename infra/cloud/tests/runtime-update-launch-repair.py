#!/usr/bin/env python3
"""Cold launch after an in-place update the image's harness gate refuses.

End to end with the real launcher (launch-runtime.sh), harness gate
(harness-release.py), root swap helper (runtime-update.py) and the server's
repair step (runtime-update-launch-repair.py), against temp roots owned by the
current user. No credentials, accounts or network.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
REPAIR = ROOT / 'runtime-update-launch-repair.py'


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def runtime(version):
    """A fake runtime: `--version` like the real one, else record the launch."""
    return ('#!/bin/sh\n'
            f'if [ "$1" = --version ]; then echo "zuse-cloud-runtime {version}"; exit 0; fi\n'
            f'echo "{version} $*" > "$ZUSE_TEST_LAUNCHED"\n').encode()


def sha(data):
    return hashlib.sha256(data).hexdigest()


@unittest.skipUnless(sys.platform == 'linux', 'Linux launcher and helper')
class LaunchRepairTest(unittest.TestCase):
    def setUp(self):
        os.umask(0o022)
        self.temp = tempfile.TemporaryDirectory(prefix='zuse-launch-repair-')
        self.addCleanup(self.temp.cleanup)
        root = self.root = Path(self.temp.name)
        uid, gid = str(os.getuid()), str(os.getgid())
        # The image's harness release, built for runtime 1.0.0.
        self.harness = root / 'opt/zuse/harnesses'
        agent = b'#!/bin/sh\nexit 0\n'
        manifest = dict(schema=2, platform='linux-' + platform.machine(), runtimeVersion='1.0.0',
                        adapterRevision='a' * 40, stateFormat='synthetic',
                        harnesses={a: dict(version='1.2.3', sha256=sha(agent)) for a in ('claude', 'codex', 'grok')})
        identifier = sha(canonical(manifest))
        release = self.harness / 'releases' / identifier
        release.mkdir(parents=True)
        for name in manifest['harnesses']:
            (release / name).write_bytes(agent)
            (release / name).chmod(0o555)
        (release / 'manifest.json').write_bytes(canonical(manifest))
        (self.harness / '.release-lock').touch()
        # Task data: identity, harness pin, a staged newer release.
        self.data = root / 'home/zuse/.zuse-data'
        self.updates = self.data / 'runtime-updates'
        (self.updates / 'staged').mkdir(parents=True, mode=0o700)
        for path in (self.data.parent, self.data, self.updates):
            path.chmod(0o700)
        (self.data / 'cloud-runtime-identity.json').write_text('{}')
        (self.data / 'cloud-harness-release.json').write_text(json.dumps({'release': identifier}))
        self.workspace = root / 'home/zuse/workspace'
        self.workspace.mkdir()
        # Root side: the base runtime (1.0.0) kept as `previous`, the in-place
        # update (1.1.0) installed and committed.
        self.state = root / 'var/lib/zuse/runtime-update'
        self.state.mkdir(parents=True)
        self.runtime_root = root / 'opt/zuse/runtime'
        self.runtime_root.mkdir(parents=True)
        (root / 'bin').mkdir()
        self.installed = root / 'bin/zuse-cloud-runtime'
        self.old, self.new = runtime('1.0.0'), runtime('1.1.0')
        (self.runtime_root / 'previous').write_bytes(self.old)
        (self.runtime_root / 'previous').chmod(0o555)
        self.install(self.new)
        self.journal = dict(version=1, updateId='update-1', sha256=sha(self.new),
                            previousSha256=sha(self.old), phase='committed', updatedAt=1)
        (self.state / 'journal.json').write_bytes(canonical(self.journal))
        staged = self.updates / 'staged' / ('d' * 64)
        staged.mkdir(mode=0o700)
        (staged / 'zuse-cloud-runtime').write_bytes(runtime('1.2.0'))
        (staged / 'release.json').write_text('{}')
        self.launched = root / 'launched'
        self.status = root / 'var/lib/zuse/workspace'
        self.status.mkdir(parents=True)
        self.env = {
            'PATH': os.environ['PATH'],
            # The launcher's own environment (launch-rust-runtime + retained launch).
            'ZUSE_HARNESS_ROOT': str(self.harness), 'ZUSE_HARNESS_CHANNEL': 'staging',
            'ZUSE_HARNESS_RELEASE_TOOL': str(ROOT / 'harness-release.py'),
            'ZUSE_RUNTIME_RETAINED': 'true', 'ZUSE_CLOUD_WORKSPACE_ID': 'workspace-fixture',
            'ZUSE_API_URL': 'https://api.example.test', 'ZUSE_RUNTIME_GENERATION': '1',
            'ZUSE_STORAGE_INCARNATION_ID': 'disk-fixture', 'ZUSE_USER_DATA': str(self.data),
            'ZUSE_CLOUD_WORKSPACE_ROOT': str(self.workspace), 'ZUSE_RUNTIME_BINARY': str(self.installed),
            'ZUSE_WORKSPACE_RUNTIME_STATUS_DIR': str(self.status), 'ZUSE_TEST_LAUNCHED': str(self.launched),
            # Repair step and root helper paths.
            'ZUSE_LAUNCH_REPAIR_STATE_DIR': str(self.state), 'ZUSE_LAUNCH_REPAIR_INSTALLED': str(self.installed),
            'ZUSE_LAUNCH_REPAIR_PREVIOUS': str(self.runtime_root / 'previous'),
            'ZUSE_LAUNCH_REPAIR_DATA_DIR': str(self.data), 'ZUSE_LAUNCH_REPAIR_HARNESS_ROOT': str(self.harness),
            'ZUSE_LAUNCH_REPAIR_HELPER': str(ROOT / 'runtime-update.py'),
            'ZUSE_LAUNCH_REPAIR_TASK_UID': uid, 'ZUSE_LAUNCH_REPAIR_TASK_GID': gid,
            'ZUSE_RUNTIME_UPDATE_DATA_DIR': str(self.updates), 'ZUSE_RUNTIME_UPDATE_STATE_DIR': str(self.state),
            'ZUSE_RUNTIME_UPDATE_RUNTIME_ROOT': str(self.runtime_root),
            'ZUSE_RUNTIME_UPDATE_INSTALLED': str(self.installed), 'ZUSE_RUNTIME_UPDATE_OWNER_UID': uid,
            'ZUSE_RUNTIME_UPDATE_TASK_UID': uid, 'ZUSE_RUNTIME_UPDATE_TASK_GID': gid,
        }

    def install(self, data):
        self.installed.write_bytes(data)
        self.installed.chmod(0o755)

    def launch(self):
        self.launched.unlink(missing_ok=True)
        return subprocess.run(['bash', str(ROOT / 'launch-runtime.sh'), '--foreground'], env=self.env,
                              capture_output=True, text=True, timeout=30)

    def repair(self):
        result = subprocess.run([sys.executable, '-I', '-B', str(REPAIR)], env=self.env,
                                capture_output=True, text=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_an_incompatible_in_place_update_is_rolled_back_and_launches(self):
        # The repro: the committed update reports a version the gate refuses,
        # so every cold launch fails before the runtime starts.
        refused = self.launch()
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn('harness release failed: ValueError', refused.stderr)
        self.assertFalse(self.launched.exists())

        repaired = self.repair()
        self.assertEqual(repaired['outcome'], 'rolled-back', repaired)
        self.assertEqual((repaired['fromVersion'], repaired['toVersion']), ('1.1.0', '1.0.0'))
        self.assertEqual(sha(self.installed.read_bytes()), sha(self.old))
        result = json.loads((self.state / 'result.json').read_text())
        self.assertEqual((result['outcome'], result['updateId']), ('rolled-back', 'update-1'))
        self.assertEqual(json.loads((self.state / 'journal.json').read_text())['phase'], 'rolled-back')
        # The staged release is gone (the next start would install it again)
        # and the request was consumed.
        self.assertEqual(list((self.updates / 'staged').iterdir()), [])
        self.assertFalse((self.updates / 'rollback.json').exists())

        launched = self.launch()
        self.assertEqual(launched.returncode, 0, launched.stderr)
        self.assertTrue(self.launched.read_text().startswith('1.0.0 --workspace-id workspace-fixture'))

        # Run again: nothing left to repair, nothing touched.
        self.assertEqual(self.repair(), {'outcome': 'not-applicable', 'code': 'runtime_update_not_committed'})

    def test_nothing_is_touched_unless_the_update_is_the_cause(self):
        def unchanged():
            self.assertEqual(sha(self.installed.read_bytes()), expected)
            self.assertEqual(len(list((self.updates / 'staged').iterdir())), 1)
            self.assertFalse((self.updates / 'rollback.json').exists())
            self.assertFalse((self.state / 'result.json').exists())

        # A compatible update: the launch failure has another cause.
        compatible = runtime('1.0.0') + b'# rebuild\n'
        self.install(compatible)
        self.journal['sha256'] = sha(compatible)
        (self.state / 'journal.json').write_bytes(canonical(self.journal))
        expected = sha(compatible)
        self.assertEqual(self.repair(), {'outcome': 'not-applicable', 'code': 'runtime_launchable'})
        unchanged()

        # The installed binary is not the committed release.
        self.journal['sha256'] = sha(self.new)
        (self.state / 'journal.json').write_bytes(canonical(self.journal))
        self.assertEqual(self.repair(), {'outcome': 'not-applicable', 'code': 'runtime_update_not_installed'})
        unchanged()

        # The previous binary would be refused too.
        self.install(self.new)
        expected = sha(self.new)
        previous = runtime('0.9.0')
        (self.runtime_root / 'previous').chmod(0o755)
        (self.runtime_root / 'previous').write_bytes(previous)
        self.journal['previousSha256'] = sha(previous)
        (self.state / 'journal.json').write_bytes(canonical(self.journal))
        self.assertEqual(self.repair(), {'outcome': 'not-applicable', 'code': 'previous_runtime_unlaunchable'})
        unchanged()

        # No pinned harness release: nothing to compare with.
        (self.data / 'cloud-harness-release.json').unlink()
        self.assertEqual(self.repair(), {'outcome': 'not-applicable', 'code': 'harness_release_unknown'})
        unchanged()

        # No committed update at all.
        (self.state / 'journal.json').unlink()
        self.assertEqual(self.repair(), {'outcome': 'not-applicable', 'code': 'runtime_update_not_committed'})
        unchanged()

    def test_the_server_sends_the_exact_script(self):
        source = (Path(__file__).resolve().parents[3] / 'crates/cloud/src/runtime/launch_repair.rs').read_text()
        self.assertIn('include_str!("../../../../infra/cloud/runtime-update-launch-repair.py")', source)
        self.assertTrue(shutil.which('python3'))


if __name__ == '__main__':
    unittest.main()
