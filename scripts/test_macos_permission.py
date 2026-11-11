"""Exercise the real UI build script for both macOS executable architectures."""
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


@unittest.skipUnless(platform.system() == 'Darwin', 'requires the macOS SDK')
class MacPermissionTests(unittest.TestCase):
    def test_permission_archive_matches_cargo_target_and_exports_rust_ffi(self):
        with tempfile.TemporaryDirectory(prefix='zuse-permission-') as directory:
            root = Path(directory)
            script = root / 'build-script'
            subprocess.run(['rustc', '--edition=2024', str(ROOT / 'crates/ui/build.rs'),
                            '-o', str(script)], check=True)
            for target, arch in [('x86_64-apple-darwin', 'x86_64'),
                                 ('aarch64-apple-darwin', 'arm64')]:
                with self.subTest(target=target):
                    output = root / target
                    output.mkdir()
                    env = dict(os.environ, TARGET=target, CARGO_CFG_TARGET_OS='macos',
                               OUT_DIR=str(output), MACOSX_DEPLOYMENT_TARGET='12.0')
                    subprocess.run([str(script)], cwd=ROOT / 'crates/ui', env=env,
                                   check=True, capture_output=True, text=True)
                    archive = output / 'libvoice-permission.a'
                    architectures = subprocess.check_output(
                        ['lipo', '-archs', str(archive)], text=True).strip()
                    self.assertEqual(architectures, arch)
                    symbols = subprocess.check_output(['nm', '-gU', str(archive)], text=True)
                    for name in ['zuse_microphone_permission', 'zuse_microphone_window',
                                 'zuse_request_microphone']:
                        self.assertIn(' _' + name, symbols)


if __name__ == '__main__':
    unittest.main()
