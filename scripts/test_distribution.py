"""Observable archive verification, native layout and terminal installation contracts."""
import hashlib
from contextlib import chdir
import io
import json
import os
import platform
from pathlib import Path
import tarfile
import subprocess
import sys
import types
import unittest
from unittest.mock import patch

import distribution
import publish_distributions
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
        for name, value in (('system', 'Linux'), ('machine', 'x86_64')):
            patcher = patch.object(platform, name, return_value=value)
            patcher.start()
            self.addCleanup(patcher.stop)

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
        for index, (name, link) in enumerate((('../escape', False), ('package/link', True), ('C:/escape', False), ('package/C:/escape', False), ('package/file:stream', False), ('package/NUL', False))):
            with self.subTest(name=name), tarfile.open(self.archive, 'w:gz') as bundle:
                member = tarfile.TarInfo(name)
                if link:
                    member.type = tarfile.SYMTYPE
                    member.linkname = '/etc/passwd'
                bundle.addfile(member, io.BytesIO())
            target = self.fixture.root / f'unpacked-{index}'
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
            self.assertTrue(all(not member.pax_headers for member in bundle.getmembers()))
            control = bundle.extractfile('control').read().decode()
            self.assertIn('Version: 0.25.0~preview.1', control)
            self.assertIn('Architecture: amd64', control)
        with tarfile.open(fileobj=io.BytesIO(members['data.tar.gz'])) as bundle:
            self.assertTrue(all(not member.pax_headers for member in bundle.getmembers()))
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

    def test_release_notes_keep_existing_changes_and_do_not_label_stable_as_preview(self):
        notes = self.fixture.root / 'notes.md'
        notes.write_text('### Added\n\n- Existing feature.\n')
        script = Path(__file__).with_name('distribution-notes.py')
        preview = subprocess.check_output([sys.executable, str(script), 'v0.26.0-preview.1', str(notes)], text=True)
        self.assertIn('Existing feature.', preview)
        self.assertIn('This is a preview release for testing.', preview)
        notes.write_text(preview)
        repeated = subprocess.check_output([sys.executable, str(script), 'v0.26.0-preview.1', str(notes)], text=True)
        self.assertEqual(repeated.count('<!-- zuse-installers:start -->'), 1)
        stable = subprocess.check_output([sys.executable, str(script), 'v0.26.0', str(notes)], text=True)
        self.assertNotIn('This is a preview release for testing.', stable)
        self.assertIn('Existing feature.', stable)
        self.assertNotIn('Command line only', stable)
        current = subprocess.check_output([sys.executable, str(script), 'v0.26.0', str(notes), '--cli-archives'], text=True)
        self.assertIn('zuse-cli-0.26.0-<platform>-<arch>.tar.gz', current)

    def test_cached_native_inputs_must_match_a_clean_release_tag(self):
        prepare = fixtures.load('prepare_distribution', 'prepare-distribution.py')
        revision = 'a' * 40
        manifest_path = self.package / 'manifest.json'
        manifest = json.loads(manifest_path.read_text())
        manifest['provenance'] = {'source_revision': revision, 'working_tree_dirty': False}
        manifest_path.write_text(json.dumps(manifest))
        self.write_archive()
        incoming = self.fixture.root / 'incoming'
        incoming.mkdir()
        cached = incoming / self.archive.name
        cached.write_bytes(self.archive.read_bytes())
        checksums = incoming / 'SHA256SUMS'
        checksums.write_text(hashlib.sha256(cached.read_bytes()).hexdigest() + '  ' + cached.name + '\n')
        output = self.fixture.root / 'outputs'
        argv = ['prepare-distribution.py', '--tag', 'v0.25.0', '--platform', 'linux', '--arch', 'x86_64', '--cached']
        with chdir(self.fixture.root), patch.dict(os.environ, GITHUB_OUTPUT=str(output)), patch.object(sys, 'argv', argv), \
                patch.object(prepare.subprocess, 'check_output', return_value=revision + '\n'):
            prepare.main()
            self.assertIn('found=true', output.read_text())
            self.assertIn('filename=zuse-0.25.0-linux-x86_64.tar.gz', output.read_text())
            with patch.object(prepare.subprocess, 'check_output', return_value='b' * 40 + '\n'), \
                    self.assertRaisesRegex(ValueError, 'clean immutable release tag'):
                prepare.main()

    def test_staged_archive_from_this_run_is_verified_like_a_cached_one(self):
        prepare = fixtures.load('prepare_distribution', 'prepare-distribution.py')
        revision = 'a' * 40
        manifest_path = self.package / 'manifest.json'
        manifest = json.loads(manifest_path.read_text())
        manifest['provenance'] = {'source_revision': revision, 'working_tree_dirty': False}
        manifest_path.write_text(json.dumps(manifest))
        self.write_archive()
        incoming = self.fixture.root / 'incoming'
        incoming.mkdir()
        (incoming / self.archive.name).write_bytes(self.archive.read_bytes())
        output = self.fixture.root / 'outputs'
        argv = ['prepare-distribution.py', '--tag', 'v0.25.0', '--platform', 'linux', '--arch', 'x86_64', '--staged']
        with chdir(self.fixture.root), patch.dict(os.environ, GITHUB_OUTPUT=str(output)), patch.object(sys, 'argv', argv), \
                patch.object(prepare.subprocess, 'check_output', return_value=revision + '\n'):
            prepare.main()
        self.assertIn('found=true', output.read_text())
        self.assertIn(self.archive.name, (incoming / 'SHA256SUMS').read_text())
        with chdir(self.fixture.root), patch.dict(os.environ, GITHUB_OUTPUT=str(output)), patch.object(sys, 'argv', argv), \
                patch.object(prepare.subprocess, 'check_output', return_value='b' * 40 + '\n'), \
                self.assertRaisesRegex(ValueError, 'clean immutable release tag'):
            prepare.main()

    def test_apple_service_drops_are_retried_then_surface(self):
        calls, naps = [], []
        def flaky(command, **options):
            calls.append(command)
            if len(calls) < 3:
                raise subprocess.CalledProcessError(68, command)
            return 'stapled'
        self.assertEqual(distribution.retrying(flaky, sleep=naps.append)(['xcrun', 'stapler', 'validate', 'Zuse.app'], check=True), 'stapled')
        self.assertEqual((len(calls), naps), (3, [30, 60]))
        def broken(command, **options):
            raise subprocess.CalledProcessError(65, command)
        with self.assertRaises(subprocess.CalledProcessError):
            distribution.retrying(broken, attempts=2, sleep=naps.append)(['xcrun', 'stapler', 'staple', 'Zuse.app'])

    def test_every_format_is_required_before_anything_is_published(self):
        version = '0.26.0-preview.3'
        complete = ['install.sh'] + [f'zuse-{version}-{name}' for name in (
            'linux-x86_64.tar.gz', 'linux-x86_64.deb', 'linux-x86_64.rpm',
            'linux-aarch64.tar.gz', 'linux-aarch64.deb', 'linux-aarch64.rpm',
            'macos-aarch64.tar.gz', 'macos-aarch64.dmg',
            'macos-x86_64.tar.gz', 'macos-x86_64.dmg',
            'windows-x86_64.zip', 'windows-x86_64-setup.exe')]
        self.assertEqual(publish_distributions.missing_assets('v' + version, complete), [])
        archives_only = [name for name in complete if name.endswith(('.tar.gz', '.zip'))]
        missing = publish_distributions.missing_assets('v' + version, archives_only)
        self.assertIn(f'zuse-{version}-macos-aarch64.dmg', missing)
        self.assertIn(f'zuse-{version}-windows-x86_64-setup.exe', missing)
        with self.assertRaisesRegex(ValueError, 'macos-x86_64.dmg'):
            publish_distributions.require_complete('v' + version, archives_only)

    def test_version_cannot_escape_the_release_url(self):
        bootstrap = bootstrap_module()
        for version in ('../../main', 'v0.26.0/escape', '0.26.0\n'):
            with self.subTest(version=version), self.assertRaises(ValueError):
                bootstrap.install(version, lambda url: self.fail('must not download'))

    def test_publishing_keeps_existing_bytes_and_commits_checksums_last(self):
        old_name = 'zuse-0.25.0-linux-x86_64.tar.gz'
        new_name = 'zuse-0.25.0-linux-x86_64.deb'
        for interrupted, fail_upload in ((False, False), (True, False), (False, True)):
            with self.subTest(interrupted=interrupted, fail_upload=fail_upload):
                assets = {old_name: b'original archive', 'install.sh': b'old installer'}
                sums = ''.join(f'{hashlib.sha256(data).hexdigest()}  {name}\n' for name, data in sorted(assets.items())).encode()
                assets['SHA256SUMS'] = sums
                if interrupted:
                    assets[new_name] = b'previously uploaded package'
                output = self.fixture.root / f'publish-{interrupted}-{fail_upload}'
                output.mkdir()
                (output / old_name).write_bytes(b'resigned archive must not replace original')
                (output / new_name).write_bytes(b'new package')
                (output / 'install.sh').write_bytes(b'new installer')
                uploads = []
                def command(action, tag, *args):
                    if action == 'view':
                        return json.dumps({'assets': [{'name': name} for name in assets]})
                    if action == 'download':
                        name = args[args.index('--pattern') + 1]
                        destination = Path(args[args.index('--dir') + 1]) / name
                        destination.write_bytes(assets[name])
                    elif action == 'upload':
                        path = Path(args[0])
                        if fail_upload and path.name == new_name:
                            raise OSError('Upload interrupted')
                        if path.name in assets:
                            self.assertIn('--clobber', args)
                            self.assertIn(path.name, ('install.sh', 'SHA256SUMS'))
                        uploads.append(path.name)
                        assets[path.name] = path.read_bytes()
                    else:
                        self.fail('Unexpected release operation')
                    return ''
                if fail_upload:
                    with self.assertRaisesRegex(OSError, 'Upload interrupted'):
                        publish_distributions.publish('v0.25.0', output, command)
                    self.assertEqual(assets['SHA256SUMS'], sums)
                    self.assertEqual(assets['install.sh'], b'old installer')
                else:
                    publish_distributions.publish('v0.25.0', output, command)
                    self.assertEqual(uploads[-1], 'SHA256SUMS')
                    self.assertEqual(assets['install.sh'], b'new installer')
                    for line in assets['SHA256SUMS'].decode().splitlines():
                        digest, name = line.split()
                        self.assertEqual(digest, hashlib.sha256(assets[name]).hexdigest())
                self.assertEqual(assets[old_name], b'original archive')


if __name__ == '__main__':
    unittest.main()
