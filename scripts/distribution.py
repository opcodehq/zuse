#!/usr/bin/env python3
"""Verify release archives and create native distribution formats without rebuilding."""
import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
REPOSITORY = 'opcodehq/zuse'
VERSION = re.compile(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?')


def release_version(tag):
    value = tag.removeprefix('v')
    if not VERSION.fullmatch(value):
        raise ValueError('Expected a version such as v0.26.0-preview.1')
    return value


def check_archive(archive, checksums):
    entries = {}
    for line in checksums.read_text().splitlines():
        digest, name = line.split(maxsplit=1)
        name = name.lstrip('*')
        if name in entries:
            raise ValueError('Duplicate checksum entry')
        entries[name] = digest
    with archive.open('rb') as stream:
        actual = hashlib.file_digest(stream, 'sha256').hexdigest()
    if entries.get(archive.name) != actual:
        raise ValueError(f'Archive checksum mismatch: {archive.name}')


def unpack(archive, directory):
    """Extract only one package root of regular files/directories, then check its inventory."""
    names = set()
    def destination(name):
        path = PurePosixPath(name)
        if path.is_absolute() or '..' in path.parts or '\\' in name or not path.parts:
            raise ValueError(f'Invalid archive path: {name}')
        if str(path) in names:
            raise ValueError(f'Duplicate archive path: {name}')
        names.add(str(path))
        return directory.joinpath(*path.parts)
    if archive.suffix == '.zip':
        with zipfile.ZipFile(archive) as bundle:
            for member in bundle.infolist():
                target = destination(member.filename)
                if (member.external_attr >> 16) & 0o170000 == 0o120000:
                    raise ValueError('Archive links are not supported')
                if member.is_dir():
                    target.mkdir(parents=True, exist_ok=True)
                else:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with bundle.open(member) as source, target.open('wb') as output:
                        shutil.copyfileobj(source, output)
    else:
        with tarfile.open(archive, 'r|gz') as bundle:
            for member in bundle:
                target = destination(member.name)
                if member.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                elif member.isfile():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with bundle.extractfile(member) as source, target.open('wb') as output:
                        shutil.copyfileobj(source, output)
                    target.chmod(member.mode & 0o777)
                else:
                    raise ValueError('Archive links and special files are not supported')
    roots = list(directory.iterdir())
    if len(roots) != 1 or not roots[0].is_dir():
        raise ValueError('Expected exactly one package directory')
    package = roots[0]
    manifest = json.loads((package / 'manifest.json').read_text())
    version = release_version(manifest['version'])
    if manifest.get('format') != 1 or manifest.get('platform') not in ('macos', 'linux', 'windows') or manifest.get('arch') not in ('x86_64', 'aarch64'):
        raise ValueError('Unsupported package manifest')
    if package.name != f'zuse-{version}-{manifest["platform"]}-{manifest["arch"]}':
        raise ValueError('Package directory and manifest disagree')
    files = {p.relative_to(package).as_posix(): p for p in package.rglob('*') if p.is_file()}
    expected = {name.replace('\\', '/'): digest for name, digest in manifest['files'].items()}
    if set(files) != set(expected) | {'manifest.json'}:
        raise ValueError('Package inventory differs from manifest')
    for name, digest in expected.items():
        with files[name].open('rb') as stream:
            if hashlib.file_digest(stream, 'sha256').hexdigest() != digest:
                raise ValueError(f'Package checksum mismatch: {name}')
    return package, manifest


def refresh_manifest(package, manifest):
    manifest['files'] = {}
    for path in sorted(package.rglob('*')):
        if path.is_file() and path != package / 'manifest.json':
            with path.open('rb') as stream:
                manifest['files'][path.relative_to(package).as_posix()] = hashlib.file_digest(stream, 'sha256').hexdigest()
    (package / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')


def notarize(path):
    identity = os.environ.get('ZUSE_MACOS_SIGN_IDENTITY')
    credentials = [os.environ.get(name) for name in ('APPLE_ID', 'APPLE_APP_SPECIFIC_PASSWORD', 'APPLE_TEAM_ID')]
    if not identity or not all(credentials):
        raise ValueError('Signing identity and all Apple notarization credentials are required')
    command = ['codesign', '--force', '--timestamp', '--sign', identity]
    if os.environ.get('ZUSE_MACOS_KEYCHAIN'):
        command += ['--keychain', os.environ['ZUSE_MACOS_KEYCHAIN']]
    subprocess.run(command + [str(path)], check=True)
    auth = ['--apple-id', credentials[0], '--password', credentials[1], '--team-id', credentials[2]]
    result = subprocess.run(['xcrun', 'notarytool', 'submit', str(path), *auth, '--wait', '--output-format', 'json'], check=True, capture_output=True, text=True)
    submission = json.loads(result.stdout)
    if submission.get('status') != 'Accepted':
        subprocess.run(['xcrun', 'notarytool', 'log', submission.get('id', ''), *auth], check=False)
        raise ValueError(f'Notarization failed: {submission.get("status")}')
    subprocess.run(['xcrun', 'stapler', 'staple', str(path)], check=True)
    subprocess.run(['xcrun', 'stapler', 'validate', str(path)], check=True)


def macos(package, manifest, output, sign_app=False):
    app = package / 'Zuse.app'
    if sign_app:
        spec = importlib.util.spec_from_file_location('zuse_package', ROOT / 'scripts/package.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        if not module.sign_macos(app):
            raise ValueError('Signing is required for the new macOS architecture')
        refresh_manifest(package, manifest)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    subprocess.run(['xcrun', 'stapler', 'validate', str(app)], check=True)
    if sign_app:
        archive = output / f'{package.name}.tar.gz'
        with tarfile.open(archive, 'w:gz') as bundle:
            bundle.add(package, arcname=package.name)
    with tempfile.TemporaryDirectory(prefix='zuse-dmg-') as temporary:
        source = Path(temporary) / 'image'
        source.mkdir()
        subprocess.run(['ditto', str(app), str(source / 'Zuse.app')], check=True)
        (source / 'Applications').symlink_to('/Applications')
        image = output / f'{package.name}.dmg'
        subprocess.run(['hdiutil', 'create', '-volname', 'Zuse', '-srcfolder', str(source), '-format', 'UDZO', '-ov', str(image)], check=True)
        notarize(image)
        subprocess.run(['hdiutil', 'verify', str(image)], check=True)


def linux_tree(package, target):
    shutil.copytree(package, target / 'opt/zuse')
    for binary in ('zuse', 'zuse-server', 'zuse-relay'):
        link = target / 'usr/bin' / binary
        link.parent.mkdir(parents=True, exist_ok=True)
        link.symlink_to(f'/opt/zuse/bin/{binary}')
    desktop = target / 'usr/share/applications/sh.zuse.app.desktop'
    desktop.parent.mkdir(parents=True, exist_ok=True)
    desktop.write_text('[Desktop Entry]\nType=Application\nName=Zuse\nComment=Local coding agents\nExec=/opt/zuse/bin/zuse %u\nIcon=sh.zuse.app\nTerminal=false\nCategories=Development;\nMimeType=x-scheme-handler/zuse;\n')
    icon = target / 'usr/share/icons/hicolor/1024x1024/apps/sh.zuse.app.png'
    icon.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(package / 'share/icons/hicolor/1024x1024/apps/sh.zuse.app.png', icon)


def tar_tree(root):
    stream = io.BytesIO()
    def root_owned(member):
        member.uid = member.gid = 0
        member.uname = member.gname = 'root'
        return member
    with tarfile.open(fileobj=stream, mode='w:gz') as bundle:
        for path in sorted(root.iterdir()):
            bundle.add(path, arcname=path.name, filter=root_owned)
    return stream.getvalue()


def ar_member(stream, name, content):
    header = f'{name + "/":<16}{0:<12}{0:<6}{0:<6}{"100644":<8}{len(content):<10}`\n'
    stream.write(header.encode('ascii'))
    stream.write(content)
    if len(content) % 2:
        stream.write(b'\n')


def deb(package, manifest, output):
    version = manifest['version'].replace('-', '~', 1)
    arch = {'x86_64': 'amd64', 'aarch64': 'arm64'}[manifest['arch']]
    with tempfile.TemporaryDirectory(prefix='zuse-deb-') as temporary:
        root = Path(temporary)
        data, control = root / 'data', root / 'control'
        data.mkdir(); control.mkdir()
        linux_tree(package, data)
        (control / 'control').write_text(f'''Package: zuse
Version: {version}
Architecture: {arch}
Maintainer: Swaraj Bachu <rajswaraj.r@gmail.com>
Section: devel
Priority: optional
Homepage: https://github.com/opcodehq/zuse
Depends: libc6 (>= 2.39), libgcc-s1, libstdc++6, libssl3t64, libasound2t64, libfontconfig1, libfreetype6, libxcb1, libxcb-shape0, libxcb-xfixes0, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libvulkan1, libwebkit2gtk-4.1-0, libjson-glib-1.0-0
Description: Multi-device controller for coding agents
 Native desktop app, server and relay with bundled browser assets.
''')
        with (output / f'{package.name}.deb').open('wb') as stream:
            stream.write(b'!<arch>\n')
            for name, content in [('debian-binary', b'2.0\n'), ('control.tar.gz', tar_tree(control)), ('data.tar.gz', tar_tree(data))]:
                ar_member(stream, name, content)


def rpm(package, manifest, output):
    base, _, suffix = manifest['version'].partition('-')
    release = '0.' + suffix.replace('-', '.') if suffix else '1'
    with tempfile.TemporaryDirectory(prefix='zuse-rpm-') as temporary:
        top = Path(temporary)
        payload = top / 'payload'
        payload.mkdir()
        linux_tree(package, payload)
        source = top / 'SOURCES'
        source.mkdir()
        (source / 'payload.tar.gz').write_bytes(tar_tree(payload))
        spec = top / 'zuse.spec'
        spec.write_text(f'''%global debug_package %{{nil}}
%global __os_install_post %{{nil}}
%global _build_id_links none
Name: zuse
Version: {base}
Release: {release}
Summary: Multi-device controller for coding agents
License: AGPL-3.0-only
URL: https://github.com/opcodehq/zuse
Source0: payload.tar.gz
BuildArch: {manifest['arch']}
Requires: libwebkit2gtk-4.1.so.0()(64bit)
Requires: libjson-glib-1.0.so.0()(64bit)
Requires: libvulkan.so.1()(64bit)
Requires: libfontconfig.so.1()(64bit)
Requires: libfreetype.so.6()(64bit)
Requires: libwayland-client.so.0()(64bit)
Requires: libwayland-cursor.so.0()(64bit)
Requires: libwayland-egl.so.1()(64bit)
Requires: libxcb-shape.so.0()(64bit)
Requires: libxcb-xfixes.so.0()(64bit)
%description
Native desktop app, server and relay with bundled browser assets.
%install
mkdir -p "%{{buildroot}}"
tar -xzf "%{{SOURCE0}}" -C "%{{buildroot}}"
%files
/opt/zuse
/usr/bin/zuse
/usr/bin/zuse-server
/usr/bin/zuse-relay
/usr/share/applications/sh.zuse.app.desktop
/usr/share/icons/hicolor/1024x1024/apps/sh.zuse.app.png
''')
        subprocess.run(['rpmbuild', '-bb', '--define', f'_topdir {top}', '--define', f'_rpmdir {top / "RPMS"}', '--define', '_binary_payload w6.gzdio', str(spec)], check=True)
        archives = list((top / 'RPMS').rglob('*.rpm'))
        if len(archives) != 1:
            raise ValueError('Expected one RPM')
        shutil.copyfile(archives[0], output / f'{package.name}.rpm')


def windows(package, manifest, output):
    compiler = shutil.which('ISCC.exe')
    if not compiler:
        compiler = str(Path(os.environ.get('ProgramFiles(x86)', r'C:\Program Files (x86)')) / 'Inno Setup 6/ISCC.exe')
    subprocess.run([compiler, f'/DSourceDir={package}', f'/DReleaseVersion={manifest["version"]}',
                    f'/DOutputDir={output}', f'/DIconFile={ROOT / "dist/windows/zuse.ico"}', f'/DNumericVersion={manifest["version"].split("-", 1)[0]}', str(ROOT / 'dist/windows/zuse.iss')], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--checksums', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--sign-app', action='store_true')
    args = parser.parse_args()
    check_archive(args.archive, args.checksums)
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='zuse-distribution-') as temporary:
        package, manifest = unpack(args.archive, Path(temporary))
        if manifest['platform'] == 'macos':
            macos(package, manifest, args.output, args.sign_app)
            if not args.sign_app:
                # Preserve the original signed archive byte-for-byte.
                shutil.copyfile(args.archive, args.output / args.archive.name)
        else:
            shutil.copyfile(args.archive, args.output / args.archive.name)
            if manifest['platform'] == 'linux':
                deb(package, manifest, args.output)
                rpm(package, manifest, args.output)
            else:
                windows(package, manifest, args.output)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
