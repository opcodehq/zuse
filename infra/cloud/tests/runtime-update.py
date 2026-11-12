#!/usr/bin/env python3
"""Root runtime swap helper against temp roots; no restart, the runtime owns probation."""
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'runtime-update.py'
spec = importlib.util.spec_from_file_location('runtime_update', SCRIPT)
helper = importlib.util.module_from_spec(spec); spec.loader.exec_module(helper)

RUNTIME = '''#!/bin/sh
# fake runtime release {label}
if [ "$1" = runtime-capabilities ]; then
  [ {probe_ok} = 1 ] || exit 1
  printf '{{"version":1,"runtimeSha256":"%s"}}' "$(sha256sum "$0" | cut -d' ' -f1)"
  exit 0
fi
exit 3
'''


def runtime_bytes(label, probe_ok=True):
    return RUNTIME.format(label=label, probe_ok=1 if probe_ok else 0).encode()


def sha(data): return hashlib.sha256(data).hexdigest()


def openssl(*args, data=None):
    return subprocess.run(['openssl', *args], input=data, capture_output=True, check=True).stdout


class RuntimeUpdateTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        os.umask(0o022)  # the helper itself runs with this umask
        cls.keys = tempfile.TemporaryDirectory(prefix='zuse-release-key-')
        root = Path(cls.keys.name)
        cls.private = root / 'release.pem'; cls.other = root / 'other.pem'
        openssl('genpkey', '-algorithm', 'ed25519', '-out', str(cls.private))
        openssl('genpkey', '-algorithm', 'ed25519', '-out', str(cls.other))
        cls.public = openssl('pkey', '-in', str(cls.private), '-pubout', '-outform', 'DER')[-32:]

    @classmethod
    def tearDownClass(cls): cls.keys.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='zuse-runtime-update-'); self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name); self.root = root
        self.data = root / 'home/.zuse-data/runtime-updates'; self.data.mkdir(parents=True, mode=0o700)
        self.state = root / 'var/lib/zuse/runtime-update'; self.state.mkdir(parents=True, mode=0o755)
        self.runtime_root = root / 'opt/zuse/runtime'; (self.runtime_root / 'releases').mkdir(parents=True, mode=0o755)
        (root / 'bin').mkdir(mode=0o755); self.installed = root / 'bin/zuse-cloud-runtime'
        self.old = runtime_bytes('old'); self.installed.write_bytes(self.old); self.installed.chmod(0o755)
        self.public_key = root / 'runtime-release-signing.pub'
        self.public_key.write_text(base64.b64encode(self.public).decode() + '\n'); self.public_key.chmod(0o644)
        overrides = dict(DATA_DIR=self.data, STATE_DIR=self.state, RUNTIME_ROOT=self.runtime_root,
                         INSTALLED=self.installed, PUBLIC_KEY=self.public_key, OWNER_UID=os.getuid(),
                         TASK_UID=os.getuid(), TASK_GID=os.getgid(), ARCHITECTURE='x86_64')
        saved = {k: os.environ.get(k) for k in ('ZUSE_RUNTIME_UPDATE_' + n for n in overrides)}
        def restore():
            for k, v in saved.items():
                if v is None: os.environ.pop(k, None)
                else: os.environ[k] = v
        self.addCleanup(restore)
        for name, value in overrides.items(): os.environ['ZUSE_RUNTIME_UPDATE_' + name] = str(value)

    def stage(self, binary, *, request_sha=None, key=None, update='update-1', release_sha=None):
        digest = sha(binary); request_sha = request_sha or digest; release_sha = release_sha or request_sha
        release = {'sha256': release_sha, 'version': '2.0.0', 'architecture': 'x86_64', 'storageFormats': ['zuse-rust-engine-v1']}
        message = self.root / 'message'; message.write_bytes(helper.signing_bytes(release))
        signature = openssl('pkeyutl', '-sign', '-inkey', str(key or self.private), '-rawin', '-in', str(message))
        staged = self.data / 'staged' / request_sha; staged.mkdir(parents=True, exist_ok=True)
        (staged / 'zuse-cloud-runtime').write_bytes(binary)
        (staged / 'release.json').write_text(json.dumps({'release': release, 'signature': base64.b64encode(signature).decode(), 'updateId': update}))
        tmp = self.data / '.request'; tmp.write_text(json.dumps({'sha256': request_sha, 'updateId': update, 'requestedAt': 1}))
        os.replace(tmp, self.data / 'request.json')
        return request_sha

    def run_helper(self, action='run'):
        self.assertEqual(helper.main([action]), 0)
        return json.loads((self.state / 'result.json').read_text())

    def assert_unchanged(self):
        self.assertEqual(self.installed.read_bytes(), self.old)
        self.assertFalse(os.path.lexists(self.data / 'request.json'))

    def test_signing_bytes_match_the_shared_contract(self):
        release = {'sha256': 'ab' * 32, 'version': '1.2.3', 'architecture': 'x86_64', 'storageFormats': ['zuse-rust-engine-v1']}
        self.assertEqual(helper.signing_bytes(dict(reversed(list(release.items())))),
                         ('zuse-runtime-release-v1\n{"sha256":"%s","version":"1.2.3","architecture":"x86_64","storageFormats":["zuse-rust-engine-v1"]}' % ('ab' * 32)).encode())

    def test_good_swap_installs_keeps_previous_and_restarts_nothing(self):
        new = runtime_bytes('new'); digest = self.stage(new)
        result = self.run_helper()
        self.assertEqual(result, {'updateId': 'update-1', 'outcome': 'committed', 'sha256': digest, 'previousSha256': sha(self.old), 'errorCode': None})
        meta = self.installed.lstat()
        self.assertEqual((self.installed.read_bytes(), meta.st_nlink, stat.S_IMODE(meta.st_mode)), (new, 1, 0o755))
        self.assertEqual((self.runtime_root / 'previous').read_bytes(), self.old)
        stored = self.runtime_root / 'releases' / digest / 'zuse-cloud-runtime'
        self.assertEqual((stored.read_bytes(), stat.S_IMODE(stored.stat().st_mode)), (new, 0o555))
        self.assertEqual(json.loads((self.runtime_root / 'releases' / digest / 'release.json').read_text())['updateId'], 'update-1')
        self.assertEqual(stat.S_IMODE((self.state / 'result.json').stat().st_mode), 0o644)
        self.assertEqual(json.loads((self.state / 'journal.json').read_text())['phase'], 'committed')
        self.assertFalse(os.path.lexists(self.data / 'request.json'))
        self.assertEqual(list((self.runtime_root / '.work').iterdir()), [])
        # Boot recovery leaves a committed install alone.
        self.assertEqual(helper.main(['recover']), 0); self.assertEqual(self.installed.read_bytes(), new)

    def test_withdrawn_request_is_never_swapped(self):
        self.stage(runtime_bytes('new'))
        original = helper.probe
        def probe_then_withdraw(*args):
            original(*args); (self.data / 'request.json').unlink()
        helper.probe = probe_then_withdraw; self.addCleanup(setattr, helper, 'probe', original)
        result = self.run_helper()
        self.assertEqual((result['outcome'], result['errorCode']), ('failed', 'runtime_update_request_withdrawn'))
        self.assert_unchanged(); self.assertEqual(json.loads((self.state / 'journal.json').read_text())['phase'], 'failed')

    def test_request_replaced_by_another_update_is_not_removed_or_swapped(self):
        self.stage(runtime_bytes('new'))
        original = helper.probe
        def probe_then_replace(*args):
            original(*args); (self.data / 'request.json').write_text(json.dumps({'sha256': 'c' * 64, 'updateId': 'update-2'}))
        helper.probe = probe_then_replace; self.addCleanup(setattr, helper, 'probe', original)
        self.assertEqual(self.run_helper()['errorCode'], 'runtime_update_request_withdrawn')
        self.assertEqual(self.installed.read_bytes(), self.old)
        self.assertTrue((self.data / 'request.json').exists())

    def test_runtime_rollback_request_restores_previous(self):
        new = runtime_bytes('new'); digest = self.stage(new); self.run_helper()
        (self.data / 'rollback.json').write_text(json.dumps({'updateId': 'update-1', 'sha256': digest}))
        result = self.run_helper()
        self.assertEqual(result, {'updateId': 'update-1', 'outcome': 'rolled-back', 'sha256': digest, 'previousSha256': sha(self.old), 'errorCode': 'runtime_update_rolled_back'})
        self.assertEqual((self.installed.read_bytes(), self.installed.lstat().st_nlink), (self.old, 1))
        self.assertFalse(os.path.lexists(self.data / 'rollback.json'))
        self.assertEqual(json.loads((self.state / 'journal.json').read_text())['phase'], 'rolled-back')

    def test_rollback_for_another_update_or_symlink_is_rejected(self):
        new = runtime_bytes('new'); digest = self.stage(new); self.run_helper()
        (self.data / 'rollback.json').write_text(json.dumps({'updateId': 'other', 'sha256': digest}))
        self.assertEqual(self.run_helper()['errorCode'], 'runtime_update_rollback_invalid')
        self.assertEqual(self.installed.read_bytes(), new)
        target = self.root / 'rollback-target'; target.write_text(json.dumps({'updateId': 'update-1', 'sha256': digest}))
        (self.data / 'rollback.json').symlink_to(target)
        self.assertEqual(self.run_helper()['errorCode'], 'runtime_update_request_invalid')
        self.assertEqual(self.installed.read_bytes(), new); self.assertTrue(target.exists())
        self.assertFalse(os.path.lexists(self.data / 'rollback.json'))

    def test_digest_mismatch_fails_without_swap(self):
        digest = self.stage(runtime_bytes('tampered'), request_sha=sha(runtime_bytes('signed')))
        result = self.run_helper()
        self.assertEqual((result['outcome'], result['errorCode'], result['sha256']), ('failed', 'runtime_update_artifact_invalid', digest))
        self.assert_unchanged()
        self.assertFalse((self.runtime_root / 'releases' / digest).exists())

    def test_bad_signature_fails_without_swap(self):
        self.stage(runtime_bytes('new'), key=self.other)
        result = self.run_helper()
        self.assertEqual((result['outcome'], result['errorCode']), ('failed', 'runtime_update_artifact_invalid'))
        self.assert_unchanged()

    def test_release_for_other_architecture_or_update_is_rejected(self):
        os.environ['ZUSE_RUNTIME_UPDATE_ARCHITECTURE'] = 'aarch64'
        self.stage(runtime_bytes('new'))
        self.assertEqual(self.run_helper()['errorCode'], 'runtime_update_architecture_mismatch')
        os.environ['ZUSE_RUNTIME_UPDATE_ARCHITECTURE'] = 'x86_64'
        digest = self.stage(runtime_bytes('new'), update='update-2')
        release = json.loads((self.data / 'staged' / digest / 'release.json').read_text()); release['updateId'] = 'other'
        (self.data / 'staged' / digest / 'release.json').write_text(json.dumps(release))
        self.assertEqual(self.run_helper()['errorCode'], 'runtime_update_artifact_invalid'); self.assert_unchanged()

    def test_probe_failure_fails_and_nothing_is_swapped(self):
        self.stage(runtime_bytes('broken', probe_ok=False))
        result = self.run_helper()
        self.assertEqual((result['outcome'], result['errorCode'], result['previousSha256']), ('failed', 'runtime_update_probe_failed', sha(self.old)))
        self.assert_unchanged()
        self.assertFalse((self.state / 'journal.json').exists())

    def test_boot_with_journal_in_swapped_rolls_back(self):
        new = runtime_bytes('new')
        (self.runtime_root / 'previous').write_bytes(self.old)
        self.installed.write_bytes(new)
        (self.state / 'journal.json').write_text(json.dumps({'version': 1, 'updateId': 'update-9', 'sha256': sha(new), 'previousSha256': sha(self.old), 'phase': 'swapped'}))
        result = self.run_helper('recover')
        self.assertEqual(result, {'updateId': 'update-9', 'outcome': 'rolled-back', 'sha256': sha(new), 'previousSha256': sha(self.old), 'errorCode': 'runtime_update_interrupted'})
        self.assertEqual(self.installed.read_bytes(), self.old)
        self.assertEqual(json.loads((self.state / 'journal.json').read_text())['phase'], 'rolled-back')
        # A committed journal is left alone.
        (self.state / 'result.json').unlink()
        self.assertEqual(helper.main(['recover']), 0); self.assertFalse((self.state / 'result.json').exists())

    def test_symlinked_or_hardlinked_request_is_rejected(self):
        target = self.root / 'elsewhere.json'
        target.write_text(json.dumps({'sha256': sha(runtime_bytes('new')), 'updateId': 'update-1'}))
        self.stage(runtime_bytes('new')); (self.data / 'request.json').unlink()
        (self.data / 'request.json').symlink_to(target)
        result = self.run_helper()
        self.assertEqual((result['outcome'], result['errorCode'], result['updateId']), ('failed', 'runtime_update_request_invalid', None))
        self.assert_unchanged(); self.assertTrue(target.exists())
        os.link(target, self.data / 'request.json')
        self.assertEqual(self.run_helper()['errorCode'], 'runtime_update_request_invalid')
        self.assert_unchanged(); self.assertTrue(target.exists())

    def test_symlinked_staged_binary_is_rejected(self):
        digest = self.stage(runtime_bytes('new')); staged = self.data / 'staged' / digest / 'zuse-cloud-runtime'
        real = self.root / 'real-binary'; real.write_bytes(staged.read_bytes()); staged.unlink(); staged.symlink_to(real)
        self.assertEqual(self.run_helper()['errorCode'], 'runtime_update_request_invalid'); self.assert_unchanged()


if __name__ == '__main__':
    unittest.main()
