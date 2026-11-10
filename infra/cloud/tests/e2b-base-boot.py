import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('boot', Path(__file__).parents[1] / 'e2b-base-boot.py')
boot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boot)


class BootTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.owner = os.getuid()
        for name in (*boot.FILES, *boot.TRUSTED, 'opt/zuse/preparation/base-prepared.json'):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(('fixture:' + name).encode())
            path.chmod(boot.FILES.get(name, 0o555))
        for path in self.root.rglob('*'):
            if path.is_dir():
                path.chmod(0o755)
        self.expected = dict(providerId='e2b', bundleSha256='b' * 64, harnessRelease='c' * 64,
                             runtimeSha256=self.hash('usr/local/bin/zuse-cloud-runtime'),
                             supervisorSha256=self.hash('usr/local/lib/zuse/runtime-supervisor.py'))
        receipt = dict(version=1, status='prepared-uncertified', **self.expected)
        path = self.root / 'opt/zuse/preparation/base-prepared.json'
        path.write_text(json.dumps(receipt))
        path.chmod(0o400)
        (self.root / boot.ALIAS).symlink_to('/usr/local/bin/zuse-cloud-runtime')
        boot.seal(self.root, self.expected, self.owner)

    def hash(self, name):
        return hashlib.sha256((self.root / name).read_bytes()).hexdigest()

    def finalize(self):
        # Exact provider finalization regression: chmod -R 777 /usr/local.
        for path in [self.root / 'usr/local', *(self.root / 'usr/local').rglob('*')]:
            if not path.is_symlink():
                path.chmod(0o777)

    def restore(self):
        return boot.validate(self.root, self.expected, self.owner, restore=True)

    def test_provider_rewrite_requires_restore_then_strict_readonly_validation(self):
        self.finalize()
        with self.assertRaises(ValueError):
            boot.validate(self.root, self.expected, self.owner)
        self.restore()
        boot.validate(self.root, self.expected, self.owner)
        for name, mode in boot.FILES.items():
            self.assertEqual(stat.S_IMODE((self.root / name).stat().st_mode), mode)
        for name in boot.DIRECTORIES:
            self.assertEqual(stat.S_IMODE((self.root / name).stat().st_mode), 0o755)

    def test_every_helper_is_hashed_and_bad_bytes_cause_zero_permission_changes(self):
        for name in boot.FILES:
            original = (self.root / name).read_bytes()
            self.finalize()
            (self.root / name).write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'artifact_changed'):
                self.restore()
            self.assertEqual(stat.S_IMODE((self.root / 'usr/local').stat().st_mode), 0o777)
            (self.root / name).write_bytes(original)
        self.restore()

    def test_alias_must_be_exact_and_no_unknown_imported_python_cache(self):
        alias = self.root / boot.ALIAS
        alias.unlink()
        alias.symlink_to('/usr/bin/false')
        with self.assertRaisesRegex(ValueError, 'alias_unsafe'):
            self.restore()
        alias.unlink()
        alias.symlink_to('/usr/local/bin/zuse-cloud-runtime')
        (self.root / 'usr/local/lib/zuse/__pycache__').mkdir()
        with self.assertRaisesRegex(ValueError, 'library_unexpected'):
            self.restore()

    def test_symlink_ancestor_and_regular_file_are_rejected(self):
        self.finalize()
        path = self.root / 'usr/local/lib/zuse/runtime-repair.py'
        path.unlink()
        path.symlink_to(self.root / 'usr/local/lib/zuse/runtime-recreation.py')
        with self.assertRaises(OSError):
            self.restore()
        self.assertEqual(stat.S_IMODE((self.root / 'usr/local').stat().st_mode), 0o777)

    def test_trusted_boot_and_seal_outside_rewritten_tree_stay_protected(self):
        self.finalize()
        (self.root / boot.TRUSTED[0]).chmod(0o777)
        with self.assertRaisesRegex(ValueError, 'file_unsafe'):
            self.restore()
        self.assertEqual(stat.S_IMODE((self.root / 'usr/local').stat().st_mode), 0o777)

    def test_wrong_binding_and_changed_preparation_are_rejected(self):
        with self.assertRaisesRegex(ValueError, 'seal_binding'):
            boot.validate(self.root, dict(self.expected, runtimeSha256='f' * 64), self.owner, restore=True)
        receipt = self.root / 'opt/zuse/preparation/base-prepared.json'
        receipt.chmod(0o600)
        receipt.write_text('{}')
        with self.assertRaisesRegex(ValueError, 'preparation_changed'):
            self.restore()

    def test_hardlinked_payload_is_not_repaired(self):
        self.finalize()
        name = 'usr/local/lib/zuse/runtime-repair.py'
        os.link(self.root / name, self.root / 'unexpected-alias')
        with self.assertRaisesRegex(ValueError, 'file_unsafe'):
            self.restore()

    def test_opened_root_descriptor_is_protected_before_children(self):
        self.root.chmod(0o777)
        with self.assertRaisesRegex(ValueError, 'directory_unsafe'):
            self.restore()
        self.root.chmod(0o700)
        with self.assertRaisesRegex(ValueError, 'directory_unsafe'):
            boot.validate(self.root, self.expected, self.owner + 1, restore=True)

    def test_unrecognized_mode_and_wrong_owner_cause_zero_repairs(self):
        self.finalize()
        path = self.root / 'usr/local/lib/zuse/runtime-supervisor.py'
        path.chmod(0o700)
        with self.assertRaisesRegex(ValueError, 'mode_changed'):
            self.restore()
        self.assertEqual(stat.S_IMODE((self.root / 'usr/local').stat().st_mode), 0o777)
        with self.assertRaisesRegex(ValueError, 'directory_unsafe'):
            boot.validate(self.root, self.expected, self.owner + 1, restore=True)

    def test_symlink_directory_is_not_followed(self):
        self.finalize()
        path = self.root / 'usr/local/lib'
        path.rename(self.root / 'library-real')
        path.symlink_to(self.root / 'library-real')
        with self.assertRaises(OSError):
            self.restore()
        self.assertEqual(stat.S_IMODE((self.root / 'usr/local').stat().st_mode), 0o777)

    def tearDown(self):
        # Test source files are owned by the unprivileged test runner.
        for path in self.root.rglob('*'):
            if not path.is_symlink() and path.is_file():
                path.chmod(0o600)


if __name__ == '__main__':
    unittest.main()
