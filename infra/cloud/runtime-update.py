#!/usr/bin/env python3
"""Root helper that swaps a cloud computer's guest runtime in place.

Design: internal-docs/cloud-runtime-updates.md ("Layout and swap").

The task user (`zuse`) downloads and verifies a signed runtime release, stages
it under ~/.zuse-data/runtime-updates/staged/<sha256>/ and then atomically
writes ~/.zuse-data/runtime-updates/request.json. A root systemd path unit
starts this helper (`run`). It never trusts the task-writable files: it copies
them into a root-only directory and checks THE COPY (digest, Ed25519 release
signature, capability probe) before anything is installed. The installed
binary stays a regular, root-owned file with one link, replaced by rename(),
so fresh-boot and retained-resume verification keep holding.

The helper does NOT restart anything (the supervisor never relaunches an
exited child). `committed` means "installed": the running runtime then exec()s
the installed path in place and owns probation. Right before the rename the
helper re-reads request.json; a runtime that stopped waiting deletes it, and
the update is abandoned (`runtime_update_request_withdrawn`).

If the new runtime fails its probation it writes
~/.zuse-data/runtime-updates/rollback.json {"updateId","sha256"}; the same path
unit runs this helper, which restores /opt/zuse/runtime/previous. A new runtime
that dies before it writes healthy.json gets the same request from the resident
supervisor's watchdog (runtime-supervisor.py), which then relaunches it once.

Every attempt ends with /var/lib/zuse/runtime-update/result.json (0644):
{"updateId","outcome":"committed|rolled-back|failed","sha256","previousSha256","errorCode"}.
Journal: /var/lib/zuse/runtime-update/journal.json, phases
staged -> swapped -> committed (or failed / rolled-back).

`recover` runs at boot: a journal left in staged/swapped means the swap was
interrupted before its result was written, so the previous binary is restored.

Paths can be overridden with ZUSE_RUNTIME_UPDATE_* environment variables. The
systemd units set none of them; they exist for the test suite.
"""
import base64
import errno
import fcntl
import hashlib
import json
import os
import platform
import re
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path

VERSION = 1
DOMAIN = b'zuse-runtime-release-v1\n'
# DER SubjectPublicKeyInfo prefix for a raw 32-byte Ed25519 public key.
ED25519_SPKI_PREFIX = bytes.fromhex('302a300506032b6570032100')
SHA = re.compile(r'[a-f0-9]{64}\Z')
SAFE = re.compile(r'[A-Za-z0-9_.:-]{1,160}\Z')
TOKEN = re.compile(r'[A-Za-z0-9._+:-]{1,128}\Z')
BINARY_LIMIT = 512 * 1024 * 1024
JSON_LIMIT = 65536
OUTCOMES = ('committed', 'rolled-back', 'failed')
ACTIVE_PHASES = ('staged', 'swapped')


class Failed(Exception):
    """Nothing was swapped (or the swap was undone before install)."""


def env(name, default):
    return os.environ.get('ZUSE_RUNTIME_UPDATE_' + name, default)


class Paths:
    def __init__(self):
        self.data = Path(env('DATA_DIR', '/home/zuse/.zuse-data/runtime-updates'))
        self.state = Path(env('STATE_DIR', '/var/lib/zuse/runtime-update'))
        self.runtime_root = Path(env('RUNTIME_ROOT', '/opt/zuse/runtime'))
        self.installed = Path(env('INSTALLED', '/usr/local/bin/zuse-cloud-runtime'))
        self.public_key = Path(env('PUBLIC_KEY', '/usr/local/share/zuse/runtime-release-signing.pub'))
        self.openssl = env('OPENSSL', '/usr/bin/openssl')
        self.owner = int(env('OWNER_UID', '0'))
        self.task_uid = int(env('TASK_UID', '10001'))
        self.task_gid = int(env('TASK_GID', '10001'))
        self.probe_timeout = float(env('PROBE_TIMEOUT', '5'))
        self.architecture = env('ARCHITECTURE', platform.machine())

    @property
    def journal(self): return self.state / 'journal.json'
    @property
    def result(self): return self.state / 'result.json'
    @property
    def releases(self): return self.runtime_root / 'releases'
    @property
    def previous(self): return self.runtime_root / 'previous'
    @property
    def work(self): return self.runtime_root / '.work'


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def fsync_dir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try: os.fsync(fd)
    finally: os.close(fd)


def atomic_write(path, data, mode):
    fd, name = tempfile.mkstemp(prefix='.write-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as out:
            os.fchmod(out.fileno(), mode)
            out.write(data); out.flush(); os.fsync(out.fileno())
        os.replace(name, path)
        fsync_dir(path.parent)
    finally:
        if os.path.lexists(name): os.unlink(name)


def root_directory(path, paths, mode):
    """Create or check a directory owned by the helper's owner, never a link."""
    if not os.path.lexists(path):
        path.mkdir(mode=mode, parents=True)
    meta = path.lstat()
    if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != paths.owner or meta.st_mode & 0o022:
        raise Failed('runtime_update_layout_invalid')
    os.chmod(path, mode)
    return path


def sha256_fd(fd, limit=BINARY_LIMIT):
    h = hashlib.sha256(); size = 0
    os.lseek(fd, 0, os.SEEK_SET)
    while block := os.read(fd, 1048576):
        size += len(block)
        if size > limit: raise Failed('runtime_update_artifact_too_large')
        h.update(block)
    return h.hexdigest()


def sha256_path(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try: return sha256_fd(fd)
    finally: os.close(fd)


# ---------------------------------------------------------------- task files

def open_task_directory(path, paths):
    """Open a task-owned directory without following any symlink on the way."""
    fd = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for part in path.parts[1:]:
            try: nxt = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
            except OSError as error:
                if error.errno in (errno.ELOOP, errno.ENOTDIR): raise Failed('runtime_update_request_invalid') from None
                raise
            os.close(fd); fd = nxt
        meta = os.fstat(fd)
        if meta.st_uid != paths.task_uid or meta.st_mode & 0o022:
            raise Failed('runtime_update_request_invalid')
        return fd
    except BaseException:
        os.close(fd); raise


def open_task_file(dir_fd, name, paths, limit):
    """A regular task-owned file with exactly one link; never a FIFO or link."""
    try: fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=dir_fd)
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOENT, errno.ENXIO, errno.ENOTDIR): raise Failed('runtime_update_request_invalid') from None
        raise
    meta = os.fstat(fd)
    if not stat.S_ISREG(meta.st_mode) or meta.st_nlink != 1 or meta.st_uid != paths.task_uid or meta.st_size > limit:
        os.close(fd); raise Failed('runtime_update_request_invalid')
    return fd


def read_task_json(dir_fd, name, paths):
    fd = open_task_file(dir_fd, name, paths, JSON_LIMIT)
    try:
        raw = os.read(fd, JSON_LIMIT + 1)
        if len(raw) > JSON_LIMIT: raise Failed('runtime_update_request_invalid')
        try: return json.loads(raw)
        except ValueError: raise Failed('runtime_update_request_invalid') from None
    finally: os.close(fd)


def unlink_task_file(dir_fd, name):
    try: os.unlink(name, dir_fd=dir_fd)
    except FileNotFoundError: pass


def parse_request(value):
    if not isinstance(value, dict) or not {'sha256', 'updateId'} <= set(value) <= {'sha256', 'updateId', 'requestedAt'}:
        raise Failed('runtime_update_request_invalid')
    sha, update = value['sha256'], value['updateId']
    if not isinstance(sha, str) or not SHA.fullmatch(sha) or not isinstance(update, str) or not SAFE.fullmatch(update):
        raise Failed('runtime_update_request_invalid')
    return sha, update


def signing_bytes(release):
    """Byte-identical to SignedRelease::signing_bytes (keys in declared order)."""
    ordered = {'sha256': release['sha256'], 'version': release['version'],
               'architecture': release['architecture'], 'storageFormats': release['storageFormats']}
    return DOMAIN + json.dumps(ordered, separators=(',', ':'), ensure_ascii=False).encode()


def parse_release(value, sha, update, paths):
    if not isinstance(value, dict) or set(value) != {'release', 'signature', 'updateId'} or value['updateId'] != update:
        raise Failed('runtime_update_artifact_invalid')
    release = value['release']
    if (not isinstance(release, dict) or set(release) != {'sha256', 'version', 'architecture', 'storageFormats'}
            or release['sha256'] != sha
            or not isinstance(release['version'], str) or not TOKEN.fullmatch(release['version'])
            or not isinstance(release['architecture'], str) or not TOKEN.fullmatch(release['architecture'])
            or not isinstance(release['storageFormats'], list) or not 1 <= len(release['storageFormats']) <= 32
            or not all(isinstance(f, str) and TOKEN.fullmatch(f) for f in release['storageFormats'])):
        raise Failed('runtime_update_artifact_invalid')
    if release['architecture'] != paths.architecture:
        raise Failed('runtime_update_architecture_mismatch')
    try: signature = base64.b64decode(value['signature'], validate=True)
    except (TypeError, ValueError): raise Failed('runtime_update_artifact_invalid') from None
    if len(signature) != 64: raise Failed('runtime_update_artifact_invalid')
    return release, signature


# ---------------------------------------------------------------- verification

def public_key(paths):
    try:
        meta = paths.public_key.lstat()
        if not stat.S_ISREG(meta.st_mode) or meta.st_uid != paths.owner or meta.st_mode & 0o022:
            raise Failed('runtime_update_signing_key_invalid')
        key = base64.b64decode(paths.public_key.read_text().strip(), validate=True)
    except FileNotFoundError: raise Failed('runtime_update_signing_key_missing') from None
    except (ValueError, UnicodeDecodeError): raise Failed('runtime_update_signing_key_invalid') from None
    if len(key) != 32: raise Failed('runtime_update_signing_key_invalid')
    return key


def verify_signature(message, signature, key, work, paths):
    """Ed25519 via the base's OpenSSL CLI (python3 stdlib has no Ed25519)."""
    der = ED25519_SPKI_PREFIX + key
    pem = b'-----BEGIN PUBLIC KEY-----\n' + base64.encodebytes(der) + b'-----END PUBLIC KEY-----\n'
    for name, body in (('key.pem', pem), ('message', message), ('signature', signature)):
        fd = os.open(work / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        with os.fdopen(fd, 'wb') as out: out.write(body)
    try:
        result = subprocess.run([paths.openssl, 'pkeyutl', '-verify', '-pubin', '-inkey', str(work / 'key.pem'), '-rawin',
                                 '-in', str(work / 'message'), '-sigfile', str(work / 'signature')],
                                stdin=subprocess.DEVNULL, capture_output=True, timeout=10, env={'PATH': '/usr/bin:/bin'})
    except (OSError, subprocess.TimeoutExpired): raise Failed('runtime_update_signature_unverifiable') from None
    if result.returncode != 0 or b'Signature Verified Successfully' not in result.stdout:
        raise Failed('runtime_update_artifact_invalid')


def probe(binary, sha, paths):
    """`runtime-capabilities` of the stored release, as the task user, 5 s cap."""
    kwargs = {}
    if os.geteuid() == 0:
        kwargs = dict(user=paths.task_uid, group=paths.task_gid, extra_groups=[])
    try:
        result = subprocess.run([str(binary), 'runtime-capabilities'], stdin=subprocess.DEVNULL, capture_output=True,
                                timeout=paths.probe_timeout, env={'PATH': '/usr/bin:/bin'}, cwd='/', **kwargs)
    except (OSError, subprocess.TimeoutExpired, subprocess.SubprocessError): raise Failed('runtime_update_probe_failed') from None
    if result.returncode != 0 or len(result.stdout) > 8192: raise Failed('runtime_update_probe_failed')
    try: receipt = json.loads(result.stdout)
    except ValueError: raise Failed('runtime_update_probe_failed') from None
    if not isinstance(receipt, dict) or receipt.get('version') != 1 or str(receipt.get('runtimeSha256', '')).lower() != sha:
        raise Failed('runtime_update_probe_failed')


# ---------------------------------------------------------------- journal/result

def load_journal(paths):
    try: meta = paths.journal.lstat()
    except FileNotFoundError: return None
    if not stat.S_ISREG(meta.st_mode) or meta.st_uid != paths.owner or meta.st_size > JSON_LIMIT:
        raise Failed('runtime_update_journal_invalid')
    return json.loads(paths.journal.read_bytes())


def save_journal(paths, journal, phase):
    journal.update(phase=phase, updatedAt=int(time.time() * 1000))
    atomic_write(paths.journal, canonical(journal) + b'\n', 0o600)


def write_result(paths, update, outcome, sha, previous, code):
    assert outcome in OUTCOMES
    value = dict(updateId=update, outcome=outcome, sha256=sha, previousSha256=previous, errorCode=code)
    atomic_write(paths.result, canonical(value) + b'\n', 0o644)
    return value


# ---------------------------------------------------------------- install

def copy_into(source_fd, dest_dir, name, mode, paths, limit=BINARY_LIMIT):
    """Copy an open file into a root-owned directory; returns the new path."""
    target = dest_dir / name
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        os.lseek(source_fd, 0, os.SEEK_SET); size = 0
        while block := os.read(source_fd, 1048576):
            size += len(block)
            if size > limit: raise Failed('runtime_update_artifact_too_large')
            os.write(fd, block)
        os.fsync(fd)
        if os.geteuid() == 0: os.fchown(fd, paths.owner, paths.owner)
        os.fchmod(fd, mode)
    except BaseException:
        os.close(fd); target.unlink(missing_ok=True); raise
    os.close(fd)
    return target


def copy_path(source, dest_dir, name, mode, paths):
    fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try: return copy_into(fd, dest_dir, name, mode, paths)
    finally: os.close(fd)


def install_file(source, expected, paths, tag):
    """rename() a verified copy of `source` onto the installed runtime path."""
    temporary = copy_path(source, paths.installed.parent, '.zuse-cloud-runtime.' + tag + '-' + os.urandom(6).hex(), 0o755, paths)
    try:
        if sha256_path(temporary) != expected: raise Failed('runtime_update_install_failed')
        os.rename(temporary, paths.installed)
        fsync_dir(paths.installed.parent)
    finally:
        temporary.unlink(missing_ok=True)


def installed_sha(paths):
    meta = paths.installed.lstat()
    if not stat.S_ISREG(meta.st_mode) or meta.st_uid != paths.owner or meta.st_nlink != 1 or meta.st_mode & 0o022:
        raise Failed('runtime_update_installed_runtime_invalid')
    return sha256_path(paths.installed)


def restore_previous(paths, journal):
    """Put the pre-swap binary back if the installed one is not it already."""
    previous = journal.get('previousSha256')
    if not isinstance(previous, str) or not SHA.fullmatch(previous): raise Failed('runtime_update_journal_invalid')
    try:
        if installed_sha(paths) == previous: return False
    except (FileNotFoundError, Failed): pass
    if sha256_path(paths.previous) != previous: raise Failed('runtime_update_previous_missing')
    install_file(paths.previous, previous, paths, 'rollback')
    return True


# ---------------------------------------------------------------- flows

def lock(paths):
    root_directory(paths.state, paths, 0o755)
    fd = os.open(paths.state / '.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    fcntl.flock(fd, fcntl.LOCK_EX)
    return fd


def recover_interrupted(paths):
    """Roll back a journal left mid-swap (staged/swapped). Returns the result, or None."""
    journal = load_journal(paths)
    if not journal or journal.get('phase') not in ACTIVE_PHASES: return None
    restore_previous(paths, journal)
    save_journal(paths, journal, 'rolled-back')
    return write_result(paths, journal.get('updateId'), 'rolled-back', journal.get('sha256'), journal.get('previousSha256'), 'runtime_update_interrupted')


def read_request_file(paths, name, parse):
    """Parsed value, or None when absent. An invalid file raises Failed."""
    try: dir_fd = open_task_directory(paths.data, paths)
    except FileNotFoundError: return None
    try:
        try: meta = os.stat(name, dir_fd=dir_fd, follow_symlinks=False)
        except FileNotFoundError: return None
        if not stat.S_ISREG(meta.st_mode): raise Failed('runtime_update_request_invalid')
        return parse(read_task_json(dir_fd, name, paths))
    finally: os.close(dir_fd)


def remove_request_file(paths, name):
    try: dir_fd = open_task_directory(paths.data, paths)
    except (OSError, Failed): return
    try: unlink_task_file(dir_fd, name)
    finally: os.close(dir_fd)


def still_requested(paths, sha, update):
    """The runtime withdraws by deleting request.json when it stops waiting."""
    try: return read_request_file(paths, 'request.json', parse_request) == (sha, update)
    except (Failed, OSError): return False


def stage_copy(paths, sha, update, work):
    """Copy staged/<sha>/{zuse-cloud-runtime,release.json} into `work`."""
    fd = open_task_directory(paths.data / 'staged' / sha, paths)
    try:
        release_value = read_task_json(fd, 'release.json', paths)
        source = open_task_file(fd, 'zuse-cloud-runtime', paths, BINARY_LIMIT)
        try: copy = copy_into(source, work, 'zuse-cloud-runtime', 0o500, paths)
        finally: os.close(source)
    finally: os.close(fd)
    release, signature = parse_release(release_value, sha, update, paths)
    # Hash THE COPY, never the task-writable original.
    if sha256_path(copy) != sha: raise Failed('runtime_update_artifact_invalid')
    verify_signature(signing_bytes(release), signature, public_key(paths), work, paths)
    atomic_write(work / 'release.json', canonical(release_value) + b'\n', 0o444)
    return copy


def store_release(paths, sha, work, copy):
    """releases/<sha>/zuse-cloud-runtime (root, 0555). Returns its path."""
    root_directory(paths.releases, paths, 0o755)
    directory = paths.releases / sha
    target = directory / 'zuse-cloud-runtime'
    if os.path.lexists(directory):
        meta = directory.lstat()
        if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != paths.owner or meta.st_mode & 0o022:
            raise Failed('runtime_update_layout_invalid')
        if os.path.lexists(target) and sha256_path(target) == sha: return target
        target.unlink(missing_ok=True)
    else:
        directory.mkdir(mode=0o755)
    os.chmod(copy, 0o555)
    os.replace(work / 'release.json', directory / 'release.json')
    os.rename(copy, target)
    fsync_dir(directory)
    if sha256_path(target) != sha: raise Failed('runtime_update_install_failed')
    return target


def keep_previous(paths, current):
    temporary = copy_path(paths.installed, paths.runtime_root, '.previous-' + os.urandom(6).hex(), 0o555, paths)
    try:
        if sha256_path(temporary) != current: raise Failed('runtime_update_install_failed')
        os.rename(temporary, paths.previous)
        fsync_dir(paths.runtime_root)
    finally: temporary.unlink(missing_ok=True)


def install(paths, sha, update):
    """Verify, store and probe the staged release, then rename it into place.

    `committed` means installed: the running runtime then exec()s the installed
    path in place (same pid, the supervisor never relaunches) and owns
    probation. If the new runtime fails, it writes rollback.json."""
    root_directory(paths.runtime_root, paths, 0o755)
    root_directory(paths.work, paths, 0o700)
    work = Path(tempfile.mkdtemp(prefix='update-', dir=paths.work))
    previous = None
    try:
        try:
            previous = installed_sha(paths)
            if previous == sha: raise Failed('runtime_update_already_installed')
            copy = stage_copy(paths, sha, update, work)
            release = store_release(paths, sha, work, copy)
            probe(release, sha, paths)
            journal = dict(version=VERSION, updateId=update, sha256=sha, previousSha256=previous)
            keep_previous(paths, previous)
        except Failed as error:
            return write_result(paths, update, 'failed', sha, previous, str(error))
        except OSError:
            return write_result(paths, update, 'failed', sha, previous, 'runtime_update_install_failed')
        save_journal(paths, journal, 'staged')
        # A runtime that stopped waiting deleted its request: never swap under it.
        if not still_requested(paths, sha, update):
            save_journal(paths, journal, 'failed')
            return write_result(paths, update, 'failed', sha, previous, 'runtime_update_request_withdrawn')
        save_journal(paths, journal, 'swapped')
        try: install_file(release, sha, paths, 'update')
        except (Failed, OSError):
            restore_previous(paths, journal)
            save_journal(paths, journal, 'failed')
            return write_result(paths, update, 'failed', sha, previous, 'runtime_update_install_failed')
        save_journal(paths, journal, 'committed')
        return write_result(paths, update, 'committed', sha, previous, None)
    finally:
        for child in work.iterdir() if work.exists() else ():
            child.unlink(missing_ok=True)
        if work.exists(): work.rmdir()


def parse_rollback(value):
    if not isinstance(value, dict) or set(value) != {'updateId', 'sha256'}: raise Failed('runtime_update_rollback_invalid')
    try: return parse_request(value)
    except Failed: raise Failed('runtime_update_rollback_invalid') from None


def rollback(paths, sha, update):
    """The new runtime failed its probation: restore the previous binary."""
    journal = load_journal(paths)
    if (not journal or journal.get('phase') != 'committed' or journal.get('sha256') != sha
            or journal.get('updateId') != update):
        return write_result(paths, update, 'failed', sha, journal.get('previousSha256') if journal else None, 'runtime_update_rollback_invalid')
    restore_previous(paths, journal)
    save_journal(paths, journal, 'rolled-back')
    return write_result(paths, update, 'rolled-back', sha, journal['previousSha256'], 'runtime_update_rolled_back')


def run(paths):
    held = lock(paths)
    try:
        result = recover_interrupted(paths)
        # rollback.json first: a failing new runtime must not wait behind a request.
        try: requested = read_request_file(paths, 'rollback.json', parse_rollback)
        except Failed as error:
            requested = None; result = write_result(paths, None, 'failed', None, None, str(error))
            remove_request_file(paths, 'rollback.json')
        if requested is not None:
            try: result = rollback(paths, *requested)
            finally: remove_request_file(paths, 'rollback.json')
        try: requested = read_request_file(paths, 'request.json', parse_request)
        except Failed as error:
            remove_request_file(paths, 'request.json')
            return write_result(paths, None, 'failed', None, None, str(error))
        if requested is None: return result
        try: return install(paths, *requested)
        finally:
            if still_requested(paths, *requested): remove_request_file(paths, 'request.json')
    finally:
        os.close(held)


def recover(paths):
    """At boot: a swap interrupted before its result was written is undone."""
    if not paths.state.exists(): return None
    held = lock(paths)
    try: return recover_interrupted(paths)
    finally: os.close(held)


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv not in (['run'], ['recover']):
        print('usage: runtime-update.py run|recover', file=sys.stderr); return 2
    paths = Paths()
    if paths.owner == 0 and os.geteuid() != 0:
        print(json.dumps({'error': 'runtime_update_root_required'})); return 1
    os.umask(0o022)
    try:
        result = run(paths) if argv == ['run'] else recover(paths)
    except Exception as error:  # never leave without a result record
        code = str(error) if isinstance(error, Failed) else 'runtime_update_helper_failed'
        try:
            # A half-done swap must not outlive this process.
            result = recover(paths)
            if result is None: result = write_result(paths, None, 'failed', None, None, code)
        except Exception: result = {'outcome': 'failed', 'errorCode': code}
        print(json.dumps(result, sort_keys=True)); return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
