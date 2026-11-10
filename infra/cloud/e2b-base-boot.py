#!/usr/bin/python3
"""Restore only sealed managed permissions after E2B finalization.

The account-free build seals bytes in /opt/zuse/base, outside the provider's
/usr/local permission rewrite. This is an operational provider trust boundary,
not protection against a provider that can modify the entire guest as root.
"""
import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys

FILES = {
    'usr/local/bin/zuse-cloud-runtime': 0o755,
    'usr/local/lib/zuse/harness-release.py': 0o755,
    'usr/local/lib/zuse/launch-rust-runtime-core': 0o755,
    'usr/local/lib/zuse/launch-rust-recovery': 0o755,
    'usr/local/lib/zuse/launch-rust-runtime': 0o755,
    'usr/local/lib/zuse/runtime-supervisor.py': 0o555,
    'usr/local/lib/zuse/runtime-repair.py': 0o555,
    'usr/local/lib/zuse/runtime-recreation.py': 0o555,
    'usr/local/lib/zuse/managed-base-install.sh': 0o555,
}
DIRECTORIES = ('usr/local', 'usr/local/bin', 'usr/local/lib', 'usr/local/lib/zuse')
TRUSTED = ('opt/zuse/base/e2b-base-boot.py', 'opt/zuse/base/managed-base-preparation.py')
ALIAS = 'usr/local/bin/zuse-grok-auth'
SEAL = 'opt/zuse/base/boot-seal.json'


def require(value, code):
    if not value:
        raise ValueError('managed_base_boot_' + code)


def identity(info):
    return info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns


def digest(fd):
    result = hashlib.sha256()
    os.lseek(fd, 0, os.SEEK_SET)
    while block := os.read(fd, 1024 * 1024):
        result.update(block)
    return result.hexdigest()


def directory(stack, root, relative, owner, strict=True):
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    stack.callback(os.close, fd)
    info = os.fstat(fd)
    require(info.st_uid == owner and not info.st_mode & 0o7022, 'directory_unsafe')
    parts = []
    # Walk relative to held directory descriptors. A concurrent rename cannot
    # turn an already checked ancestor into a followed symlink.
    for part in Path(relative).parts:
        parts.append(part)
        fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
        stack.callback(os.close, fd)
        info = os.fstat(fd)
        name = '/'.join(parts)
        require((info.st_uid == owner or name == 'opt/zuse/preparation' and info.st_uid == 10001)
                and not info.st_mode & 0o7000, 'directory_unsafe')
        require(not strict and name in DIRECTORIES or not info.st_mode & 0o022, 'directory_unsafe')
    return fd


def regular(stack, root, relative, owner, strict=True):
    parent = directory(stack, root, str(Path(relative).parent), owner, strict)
    fd = os.open(Path(relative).name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=parent)
    stack.callback(os.close, fd)
    info = os.fstat(fd)
    require(stat.S_ISREG(info.st_mode) and info.st_uid == owner and info.st_nlink == 1
            and not info.st_mode & 0o7000 and info.st_size <= 512 * 1024 * 1024, 'file_unsafe')
    require(not strict or not info.st_mode & 0o022, 'file_unsafe')
    value = digest(fd)
    require(identity(os.fstat(fd)) == identity(info), 'file_changed')
    return fd, value


def library(root, owner, strict=True):
    names = {Path(name).name for name in FILES if name.startswith('usr/local/lib/zuse/')}
    with contextlib.ExitStack() as stack:
        fd = directory(stack, root, 'usr/local/lib/zuse', owner, strict)
        require(set(os.listdir(fd)) == names, 'library_unexpected')


def alias(root, owner, strict=True):
    with contextlib.ExitStack() as stack:
        fd = directory(stack, root, 'usr/local/bin', owner, strict)
        name = Path(ALIAS).name
        info = os.stat(name, dir_fd=fd, follow_symlinks=False)
        require(stat.S_ISLNK(info.st_mode) and info.st_uid == owner
                and os.readlink(name, dir_fd=fd) == '/usr/local/bin/zuse-cloud-runtime', 'alias_unsafe')
        return identity(info)


def trusted_seal(stack, root, expected, owner):
    fd, _ = regular(stack, root, SEAL, owner)
    require(os.fstat(fd).st_size <= 65536, 'seal_too_large')
    os.lseek(fd, 0, os.SEEK_SET)
    value = json.loads(os.read(fd, 65537))
    require(value.get('version') == 1 and value.get('expected') == expected
            and set(value.get('files', {})) == set(FILES)
            and set(value.get('trusted', {})) == set(TRUSTED), 'seal_binding')
    for name in TRUSTED:
        _, actual = regular(stack, root, name, owner)
        require(value['trusted'][name] == actual, 'trusted_changed')
    return value


def seal(root, expected, owner=0):
    with contextlib.ExitStack() as stack:
        require(not (root / SEAL).exists(), 'seal_exists')
        library(root, owner)
        alias(root, owner)
        files = {name: regular(stack, root, name, owner)[1] for name in FILES}
        require(files['usr/local/bin/zuse-cloud-runtime'] == expected['runtimeSha256']
                and files['usr/local/lib/zuse/runtime-supervisor.py'] == expected['supervisorSha256'], 'artifact_binding')
        trusted = {name: regular(stack, root, name, owner)[1] for name in TRUSTED}
        receipt_fd, receipt_hash = regular(stack, root, 'opt/zuse/preparation/base-prepared.json', owner)
        os.lseek(receipt_fd, 0, os.SEEK_SET)
        receipt = json.loads(os.read(receipt_fd, 65537))
        require(receipt.get('version') == 1 and receipt.get('status') == 'prepared-uncertified'
                and all(receipt.get(k) == v for k, v in expected.items()), 'preparation_binding')
        parent = directory(stack, root, 'opt/zuse/base', owner)
        value = dict(version=1, expected=expected, files=files, trusted=trusted, preparationSha256=receipt_hash)
        data = (json.dumps(value, sort_keys=True, separators=(',', ':')) + '\n').encode()
        fd = os.open(Path(SEAL).name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o400, dir_fd=parent)
        with os.fdopen(fd, 'wb') as output:
            os.fchmod(output.fileno(), 0o400)
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.fsync(parent)
        return value


def validate(root, expected, owner=0, restore=False):
    with contextlib.ExitStack() as stack:
        value = trusted_seal(stack, root, expected, owner)
        library(root, owner, strict=not restore)
        before_alias = alias(root, owner, strict=not restore)
        opened = {}
        directories = {}
        for name in DIRECTORIES:
            directories[name] = directory(stack, root, name, owner, strict=not restore)
            require(stat.S_IMODE(os.fstat(directories[name]).st_mode) in ((0o755, 0o777) if restore else (0o755,)), 'mode_changed')
        for name, mode in FILES.items():
            fd, actual = regular(stack, root, name, owner, strict=not restore)
            require(actual == value['files'][name], 'artifact_changed')
            opened[name] = fd
            require(stat.S_IMODE(os.fstat(fd).st_mode) in ((mode, 0o777) if restore else (mode,)), 'mode_changed')
        _, receipt_hash = regular(stack, root, 'opt/zuse/preparation/base-prepared.json', owner)
        require(receipt_hash == value['preparationSha256'], 'preparation_changed')
        # Validate every byte/path before changing any permission. No task or
        # credentials have been admitted at this account-free boot boundary.
        if restore:
            for fd in directories.values():
                os.fchmod(fd, 0o755)
            for name, fd in opened.items():
                os.fchmod(fd, FILES[name])
            library(root, owner)
            require(alias(root, owner) == before_alias, 'alias_changed')
            for name, fd in opened.items():
                require(digest(fd) == value['files'][name]
                        and identity(os.stat(root / name, follow_symlinks=False)) == identity(os.fstat(fd)), 'artifact_changed')
        return value


def ready(root, expected):
    verified = validate(root, expected)
    spec = importlib.util.spec_from_file_location('zuse_preparation', root / TRUSTED[1])
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    def ping(paths):
        result = subprocess.run(['/usr/bin/sudo', '-n', '-u', 'zuse', '/usr/bin/env', 'HOME=/home/zuse',
                                 '/usr/bin/python3', '-I', '-S', '-B', str(paths.supervisor), 'ping'],
                                stdin=subprocess.DEVNULL, capture_output=True, timeout=3,
                                env={'PATH': '/usr/bin:/bin', 'HOME': '/root'})
        require(result.returncode == 0 and len(result.stdout) <= 8192, 'resident_unavailable')
        return json.loads(result.stdout)
    result = module.ready(module.Paths(root), expected, ping_fn=ping)
    return dict(result, bootSealVerified=True, bootSha256=verified['trusted'][TRUSTED[0]])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('seal', 'boot', 'ready'))
    parser.add_argument('--provider', choices=('e2b',), required=True)
    for key in ('bundle', 'runtime', 'supervisor'):
        parser.add_argument('--' + key + '-sha256', required=True)
    parser.add_argument('--harness-release', required=True)
    args = parser.parse_args()
    require(os.geteuid() == 0, 'root_required')
    expected = dict(providerId=args.provider, bundleSha256=args.bundle_sha256, runtimeSha256=args.runtime_sha256,
                    supervisorSha256=args.supervisor_sha256, harnessRelease=args.harness_release)
    require(all(len(v) == 64 and all(c in '0123456789abcdef' for c in v)
                for k, v in expected.items() if k != 'providerId'), 'pin_invalid')
    root = Path('/')
    if args.action == 'seal':
        seal(root, expected)
    elif args.action == 'ready':
        print(json.dumps(ready(root, expected), separators=(',', ':')))
    else:
        require(not any(path.name.isdigit() and path.stat().st_uid == 10001 for path in Path('/proc').iterdir()), 'task_process_exists')
        validate(root, expected, restore=True)
        os.execve('/usr/bin/python3', ['python3', '-I', '-S', '-B', '/usr/local/lib/zuse/runtime-supervisor.py', 'serve'],
                  {'HOME': '/home/zuse', 'PATH': '/opt/zuse/bin:/usr/local/bin:/usr/bin:/bin', 'ZUSE_RUNTIME_PROVIDER': 'e2b'})


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        code = str(error) if isinstance(error, ValueError) and str(error).startswith('managed_base_boot_') else 'managed_base_boot_failed'
        print(json.dumps(dict(version=1, status='not-ready', code=code)), file=sys.stderr)
        raise SystemExit(1)
