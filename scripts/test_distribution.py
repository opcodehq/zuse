"""Observable archive verification, native layout and terminal installation contracts."""
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile
import types
import unittest
from unittest.mock import patch

import distribution
import test_package as fixtures


def bootstrap_module():
    source = Path(__file__).with_name('install.sh').read_text().split("<<'PY'\n", 1)[1].rsplit('\nPY\n', 1)[0]
    module = types.ModuleType('installer_bootstrap')
    exec(compile(source, 'install.sh', 'exec'), module.__dict__)
    return module


class DistributionTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.PackagingTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.fixture.destination = self.fixture.root / 'zuse-0.25.0-linux-x86_64'
        self.fixture.assemble()
        self.package = self.fixture.destination
        self.archive = self.fixture.root / (self.package.name + '.tar.gz')
        self.write_archive()
        self.checksums = self.fixture.root / 'SHA256SUMS'
        self.checksums.write_text(hashlib.sha256(self.archive.read_bytes()).hexdigest() + '  ' + self.archive.name + '\n')

    def write_archive(self):
        with tarfile.open(self.archive, 'w:gz') as bundle:
            bundle.add(self.package, arcname=self.package.name)

    def test_archive_and_inventory_are_verified_before_conversion(self):
        distribution.check_archive(self.archive, self.checksums)
        target = self.fixture.root / 'unpacked'
        target.mkdir()
        package, manifest = distribution.unpack(self.archive, target)
        self.assertEqual(manifest['version'], '0.25.0')
        self.assertEqual((package / 'libexec/web/app.wasm').read_bytes(), b'\0asm\1\0\0\0')
        self.archive.write_bytes(self.archive.read_bytes() + b'changed')
        with self.assertRaisesRegex(ValueError, 'Archive checksum'):
            distribution.check_archive(self.archive, self.checksums)

    def test_tampered_file_is_rejected_even_in_a_readable_archive(self):
        (self.package / 'libexec/web/app.js').write_text('tampered')
        self.write_archive()
        target = self.fixture.root / 'unpacked'
        target.mkdir()
        with self.assertRaisesRegex(ValueError, 'Package checksum'):
            distribution.unpack(self.archive, target)

    def test_traversal_and_links_are_rejected(self):
        for name, link in (('../escape', False), ('package/link', True)):
            with self.subTest(name=name), tarfile.open(self.archive, 'w:gz') as bundle:
                member = tarfile.TarInfo(name)
                if link:
                    member.type = tarfile.SYMTYPE
                    member.linkname = '/etc/passwd'
                bundle.addfile(member, io.BytesIO())
            target = self.fixture.root / ('unpacked-link' if link else 'unpacked-path')
            target.mkdir()
            with self.assertRaises(ValueError):
                distribution.unpack(self.archive, target)
        self.assertFalse((self.fixture.root / 'escape').exists())

    def test_debian_payload_has_system_launchers_and_prerelease_ordering(self):
        output = self.fixture.root / 'deb'
        output.mkdir()
        manifest = json.loads((self.package / 'manifest.json').read_text())
        manifest['version'] = '0.25.0-preview.1'
        distribution.deb(self.package, manifest, output)
        archive = next(output.glob('*.deb')).read_bytes()
        self.assertEqual(archive[:8], b'!<arch>\n')
        members = {}
        offset = 8
        while offset < len(archive):
            header = archive[offset:offset + 60]
            name, size = header[:16].decode().strip().rstrip('/'), int(header[48:58])
            members[name] = archive[offset + 60:offset + 60 + size]
            offset += 60 + size + size % 2
        self.assertEqual(members['debian-binary'], b'2.0\n')
        with tarfile.open(fileobj=io.BytesIO(members['control.tar.gz'])) as bundle:
            control = bundle.extractfile('control').read().decode()
            self.assertIn('Version: 0.25.0~preview.1', control)
            self.assertIn('Architecture: amd64', control)
        with tarfile.open(fileobj=io.BytesIO(members['data.tar.gz'])) as bundle:
            link = bundle.getmember('usr/bin/zuse')
            self.assertTrue(link.issym())
            self.assertEqual((link.uid, link.gid), (0, 0))
            self.assertEqual(link.linkname, '/opt/zuse/bin/zuse')
            self.assertIn(b'Exec=/opt/zuse/bin/zuse %u', bundle.extractfile('usr/share/applications/sh.zuse.app.desktop').read())
            self.assertEqual(bundle.extractfile('opt/zuse/libexec/web/app.wasm').read(), b'\0asm\1\0\0\0')

    def test_terminal_install_and_uninstall_preserve_profiles(self):
        bootstrap = bootstrap_module()
        data, bin_dir = self.fixture.root / 'user data', self.fixture.root / 'user bin'
        profile = self.fixture.root / 'profile'
        profile.mkdir()
        (profile / 'keep').write_text('conversation')
        def download(url):
            return self.checksums.read_bytes() if url.endswith('SHA256SUMS') else self.archive.read_bytes()
        with patch.dict(os.environ, XDG_DATA_HOME=str(data), ZUSE_BIN_HOME=str(bin_dir), ZUSE_DATA_DIR=str(profile)):
            bootstrap.install('0.25.0', download)
            installed = (bin_dir / 'zuse').resolve()
            self.assertTrue(installed.is_file())
            self.assertTrue((bin_dir / 'zuse-server').is_symlink())
            bootstrap.uninstall()
        self.assertFalse((bin_dir / 'zuse').exists())
        self.assertTrue(installed.is_file())
        self.assertEqual((profile / 'keep').read_text(), 'conversation')

    def test_terminal_install_rejects_bad_checksum_without_executing_anything(self):
        bootstrap = bootstrap_module()
        with patch.object(bootstrap.subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'checksum'):
                bootstrap.install('0.25.0', lambda url: self.checksums.read_bytes() if url.endswith('SHA256SUMS') else b'bad archive')
            run.assert_not_called()

    def test_terminal_install_rejects_traversal_even_with_matching_archive_checksum(self):
        bootstrap = bootstrap_module()
        with tarfile.open(self.archive, 'w:gz') as bundle:
            bundle.addfile(tarfile.TarInfo('../escape'), io.BytesIO())
        checksum = hashlib.sha256(self.archive.read_bytes()).hexdigest() + '  ' + self.archive.name
        def download(url):
            return checksum.encode() if url.endswith('SHA256SUMS') else self.archive.read_bytes()
        with patch.object(bootstrap.subprocess, 'run') as run, self.assertRaisesRegex(ValueError, 'Unsafe package'):
            bootstrap.install('0.25.0', download)
        run.assert_not_called()

    def test_version_cannot_escape_the_release_url(self):
        bootstrap = bootstrap_module()
        for version in ('../../main', 'v0.26.0/escape', '0.26.0\n'):
            with self.subTest(version=version), self.assertRaises(ValueError):
                bootstrap.install(version, lambda url: self.fail('must not download'))


if __name__ == '__main__':
    unittest.main()
