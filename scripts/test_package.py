"""Packaging contracts; these fixtures do not establish native runtime acceptance."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import plistlib
import struct
import subprocess
import sys
import tempfile
import unittest
import zipfile
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
        # Signing tests opt in explicitly; a signing-configured shell must not
        # make the fixture builds call codesign.
        environment = patch.dict(os.environ)
        environment.start()
        self.addCleanup(environment.stop)
        for key in ("ZUSE_MACOS_SIGN_IDENTITY", "ZUSE_MACOS_KEYCHAIN", "APPLE_ID",
                    "APPLE_APP_SPECIFIC_PASSWORD", "APPLE_TEAM_ID"):
            os.environ.pop(key, None)
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


    def test_macos_signing_helpers_before_bundle_then_notarize_and_staple(self):
        app = self.root / "Zuse.app"
        calls = []

        def run(command, **options):
            calls.append(command)
            stdout = json.dumps({"id": "submission", "status": "Accepted"})
            return subprocess.CompletedProcess(command, 0, stdout=stdout)

        self.assertFalse(package.sign_macos(app, run))
        self.assertEqual(calls, [])
        environment = {"ZUSE_MACOS_SIGN_IDENTITY": "Developer ID Application: Test (TEAM)",
                       "APPLE_ID": "dev@example.com", "APPLE_APP_SPECIFIC_PASSWORD": "secret",
                       "APPLE_TEAM_ID": "TEAM"}
        with patch.dict(os.environ, environment):
            self.assertTrue(package.sign_macos(app, run))
        signed = [command[-1] for command in calls if command[:2] == ["codesign", "--force"]]
        self.assertEqual(signed, [str(app / "Contents/MacOS/zuse-server"),
                                  str(app / "Contents/MacOS/zuse-relay"), str(app)])
        bundle = next(command for command in calls if command[-1] == str(app) and "--force" in command)
        self.assertIn("runtime", bundle)
        self.assertIn("--entitlements", bundle)
        tools = [command[1] if command[0] == "xcrun" else command[0] for command in calls]
        self.assertEqual(tools[-5:], ["ditto", "notarytool", "stapler", "stapler", "spctl"])

    def test_macos_notarization_rejection_fails_the_package(self):
        def run(command, **options):
            stdout = json.dumps({"id": "submission", "status": "Invalid"})
            return subprocess.CompletedProcess(command, 0, stdout=stdout)

        environment = {"ZUSE_MACOS_SIGN_IDENTITY": "identity", "APPLE_ID": "id",
                       "APPLE_APP_SPECIFIC_PASSWORD": "password", "APPLE_TEAM_ID": "team"}
        with patch.dict(os.environ, environment), self.assertRaisesRegex(ValueError, "Invalid"):
            package.sign_macos(self.root / "Zuse.app", run)


    def test_build_and_package_stages_split_the_dependency_build_from_assembly(self):
        host = {"Linux": "linux", "Darwin": "macos"}[package.platform.system()]
        arch = {"x86_64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}[package.platform.machine()]
        calls = []

        def run(command, **options):
            calls.append(command[0])
            if command[0] == "git":
                raise subprocess.CalledProcessError(1, command)
            return subprocess.CompletedProcess(command, 0, stdout="")

        argv = ["package.py", "--platform", host, "--arch", arch]
        with patch.object(package.subprocess, "run", run), patch.object(sys, "argv", argv + ["--build-only"]):
            package.main()
        self.assertEqual(calls, ["bash", "cargo"])
        calls.clear()
        with patch.object(package.subprocess, "run", run), patch.object(sys, "argv", argv + ["--skip-build"]), \
                self.assertRaises(subprocess.CalledProcessError):
            package.main()
        self.assertEqual(calls, ["git"])


    def test_windows_layout_puts_the_app_on_top_and_helpers_behind_launchers(self):
        for binary in self.binaries:
            header = bytearray(b"MZ" + bytes(0x3A) + struct.pack("<I", 0x40))
            binary.write_bytes(bytes(header) + b"PE\0\0" + struct.pack("<H", 0x8664) + bytes(18))
        self.assemble("windows")
        self.assertTrue((self.destination / "zuse.exe").is_file())
        for helper in ("zuse-server", "zuse-relay"):
            self.assertTrue((self.destination / f"libexec/{helper}.exe").is_file())
            launcher = (self.destination / f"bin/{helper}.cmd").read_bytes().decode()
            self.assertIn("\r\n", launcher)
            self.assertIn(f'"%root%\\libexec\\{helper}.exe" --assets "%assets%" %*', launcher)
        # Beside zuse.exe, where the desktop app's Browser Access looks for it.
        self.assertTrue((self.destination / "web/app.wasm").is_file())
        self.assertIn('set "assets=%root%\\web"', launcher)
        self.assertFalse((self.destination / "install.py").exists())
        manifest = json.loads((self.destination / "manifest.json").read_text())
        self.assertEqual(manifest["platform"], "windows")
        self.assertIn("zuse.exe", manifest["files"])

    def test_preview_zip_has_matching_version_and_verifiable_inventory(self):
        for binary in self.binaries:
            binary.write_bytes(b"MZ" + bytes(0x3A) + struct.pack("<I", 0x40)
                               + b"PE\0\0" + struct.pack("<H", 0x8664) + bytes(18))
            binary.rename(binary.with_suffix(".exe"))
        version = "0.26.0-preview.1"
        output = self.root / "archives"
        argv = ["package.py", "--platform", "windows", "--arch", "x86_64",
                "--binaries", str(self.source), "--assets", str(self.assets),
                "--output", str(output), "--version", version]
        with patch.object(sys, "argv", argv):
            package.main()
        archive = output / f"zuse-{version}-windows-x86_64.zip"
        with zipfile.ZipFile(archive) as bundle:
            self.assertIsNone(bundle.testzip())
            bundle.extractall(self.root / "unpacked")
        unpacked = self.root / f"unpacked/zuse-{version}-windows-x86_64"
        manifest = json.loads((unpacked / "manifest.json").read_text())
        for relative, digest in manifest["files"].items():
            self.assertEqual(hashlib.sha256((unpacked / relative).read_bytes()).hexdigest(), digest)
        self.assertEqual(manifest["version"], version)
        self.assertIn("zuse.exe", manifest["files"])
        self.assertIn("web/app.wasm", manifest["files"])

    def test_release_version_rejects_wrong_base_and_path_characters_before_building(self):
        for version in ("0.27.0-preview.1", "0.26.0-../../escape", "0.26.0-", "0.26.0-preview\n"):
            with self.subTest(version=version), patch.object(sys, "argv", ["package.py", "--version", version]), \
                    patch.object(package.subprocess, "run") as run, self.assertRaises(SystemExit):
                package.main()
            run.assert_not_called()

    def test_preview_macos_bundle_keeps_numeric_apple_versions(self):
        for binary in self.binaries:
            binary.write_bytes(b"\xcf\xfa\xed\xfe" + struct.pack("<I", 0x1000007) + bytes(24))
        package.assemble(self.destination, self.binaries, self.assets, "macos", "x86_64",
                         "0.26.0-preview.1", {"inputs": "test fixture"})
        info = plistlib.loads((self.destination / "Zuse.app/Contents/Info.plist").read_bytes())
        self.assertEqual(info["CFBundleVersion"], "0.26.0")
        self.assertEqual(info["CFBundleShortVersionString"], "0.26.0")
        self.assertEqual(json.loads((self.destination / "manifest.json").read_text())["version"],
                         "0.26.0-preview.1")


if __name__ == "__main__":
    unittest.main()
