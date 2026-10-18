"""Packaging contracts; these fixtures do not establish native runtime acceptance."""
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


package = load("package", "package.py")
installer = load("installer", "install-linux.py")


class PackagingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="zuse package's ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        self.source.mkdir()
        # Minimal, format-labelled inputs exercise assembly, never app execution.
        self.binaries = []
        for name in package.BINS:
            binary = self.source / name
            binary.write_bytes(b"\x7fELF\x02\x01" + bytes(12) + struct.pack("<H", 62) + bytes(12))
            self.binaries.append(binary)
        self.assets = self.source / "assets"
        self.assets.mkdir()
        (self.assets / "index.html").write_text('<html><script src="app.js"></script></html>')
        (self.assets / "app.js").write_text("// fixture")
        (self.assets / "app.wasm").write_bytes(b"\0asm\1\0\0\0")
        self.destination = self.root / "extracted package"

    def assemble(self, system="linux"):
        package.assemble(self.destination, self.binaries, self.assets, system,
                         "x86_64", "0.25.0", {"inputs": "test fixture"})

    def test_linux_inventory_and_legal(self):
        self.assemble()
        manifest = installer.verify(self.destination)
        self.assertEqual(manifest["arch"], "x86_64")
        for relative in ("libexec/web/app.wasm", "share/zuse/legal/LICENSE",
                         "share/zuse/legal/crates/icons/LICENSE.Hugeicons",
                         "share/zuse/legal/vendor/zkit/NOTICE",
                         "share/zuse/legal/crates/ui/assets/fonts/licenses/Geist-OFL.txt"):
            self.assertIn(relative, manifest["files"])
        (self.destination / "libexec/web/app.js").write_text("changed")
        with self.assertRaisesRegex(ValueError, "checksum"):
            installer.verify(self.destination)

    def test_reject_wrong_target_and_short_binary(self):
        with self.assertRaisesRegex(ValueError, "target"):
            self.assemble("macos")
        self.assertFalse(self.destination.exists())
        self.binaries[0].write_bytes(b"\x7fELF\x02\x01")
        with self.assertRaisesRegex(ValueError, "Truncated"):
            self.assemble()

    def test_reject_symlinked_assets(self):
        (self.assets / "secret").symlink_to(self.binaries[0])
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.assemble()

    def test_relocated_launchers_preserve_arguments_and_overrides(self):
        self.assemble()
        moved = self.root / 'moved "$` package'
        self.destination.rename(moved)
        for name in package.BINS:
            package.write_executable(moved / "libexec" / name,
                '#!/usr/bin/env python3\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\n')
        alias = self.root / "link dir/zuse-server"
        alias.parent.mkdir()
        alias.symlink_to(moved / "bin/zuse-server")
        def invoke(executable, *arguments):
            return json.loads(subprocess.check_output([str(executable), *arguments], cwd="/", text=True))
        special = ['hello \' " $`', "two\nlines", ""]
        self.assertEqual(invoke(moved / "bin/zuse", *special), special)
        self.assertEqual(invoke(alias, *special), ["--assets", str(moved / "libexec/web"), *special])
        self.assertEqual(invoke(alias, "--assets", "other assets", *special), ["--assets", "other assets", *special])
        self.assertEqual(invoke(moved / "bin/zuse-relay", "--assets=elsewhere"), ["--assets=elsewhere"])
        with patch.dict("os.environ", {"ZUSE_WEB_ASSETS": "/custom assets"}):
            self.assertEqual(invoke(alias), ["--assets", "/custom assets"])

    def test_install_upgrade_and_uninstall_preserve_profiles_and_unrelated_files(self):
        self.assemble()
        data, bin_dir = self.root / "user's data", self.root / "user's bin"
        profile = self.root / ".zuse-native"
        profile.mkdir()
        (profile / "keep").write_text("profile")
        installer.install(self.destination, data, bin_dir)
        first = (bin_dir / "zuse").resolve()
        self.assertTrue(first.is_file())
        with self.assertRaisesRegex(ValueError, "already installed"):
            installer.install(self.destination, data, bin_dir)
        # A new, verified package activates without removing the old package.
        original = self.destination
        self.destination = self.root / "second package"
        self.assemble()
        installer.install(self.destination, data, bin_dir)
        self.assertNotEqual((bin_dir / "zuse").resolve(), first)
        self.assertTrue(first.is_file())
        (bin_dir / "zuse-relay").unlink()
        (bin_dir / "zuse-relay").write_text("unrelated replacement")
        installer.uninstall(data, bin_dir)
        self.assertFalse((bin_dir / "zuse").is_symlink())
        self.assertEqual((bin_dir / "zuse-relay").read_text(), "unrelated replacement")
        self.assertEqual((profile / "keep").read_text(), "profile")
        self.assertTrue(original.is_dir())

    def test_collision_preflight_leaves_all_links_untouched(self):
        self.assemble()
        data, bin_dir = self.root / "data", self.root / "bin"
        bin_dir.mkdir()
        (bin_dir / "zuse-relay").write_text("owned by user")
        with self.assertRaisesRegex(ValueError, "not installed"):
            installer.install(self.destination, data, bin_dir)
        self.assertFalse((bin_dir / "zuse").exists())
        self.assertFalse(data.exists())

    def test_symlinked_install_locations_can_be_uninstalled(self):
        self.assemble()
        data, bin_dir = self.root / "data", self.root / "bin"
        for alias in (data, bin_dir):
            actual = alias.with_name(alias.name + " actual")
            actual.mkdir()
            alias.symlink_to(actual, target_is_directory=True)
        environment = dict(os.environ, XDG_DATA_HOME=str(data), ZUSE_BIN_HOME=str(bin_dir))
        script = self.destination / "install.py"
        subprocess.run([sys.executable, str(script)], env=environment, check=True, capture_output=True)
        installed = (bin_dir / "zuse").resolve()
        self.assertTrue((bin_dir / "zuse").is_symlink())
        subprocess.run([sys.executable, str(script), "--uninstall"], env=environment, check=True, capture_output=True)
        self.assertFalse((bin_dir / "zuse").is_symlink())
        self.assertTrue(installed.is_file())

    def test_macos_metadata_resources_and_icon(self):
        for binary in self.binaries:
            binary.write_bytes(b"\xcf\xfa\xed\xfe" + struct.pack("<I", 0x1000007) + bytes(24))
        self.assemble("macos")
        contents = self.destination / "Zuse.app/Contents"
        info = plistlib.loads((contents / "Info.plist").read_bytes())
        self.assertEqual(info["CFBundleShortVersionString"], "0.25.0")
        self.assertEqual(info["CFBundleExecutable"], "zuse")
        self.assertTrue((contents / "Resources/web/app.wasm").is_file())
        icon = (contents / "Resources/zuse.icns").read_bytes()
        self.assertEqual(icon[:4], b"icns")
        self.assertEqual(struct.unpack_from(">I", icon, 4)[0], len(icon))
        self.assertEqual(icon[8:12], b"ic10")


if __name__ == "__main__":
    unittest.main()
