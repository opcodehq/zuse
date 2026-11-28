#!/usr/bin/env python3
"""Undo an in-place runtime update this computer's image can no longer launch.

The server runs this as root through the provider's exec, and only after a
retained launch failed (crates/cloud/src/runtime/launch_repair.rs). Every
launch passes the image's harness gate (harness-release.py exec), which
refuses a runtime whose `--version` differs from the harness release's
`runtimeVersion`. An in-place swap re-execs the new binary without that gate,
so a computer updated to another version serves normally until its first cold
launch, which then fails on every attempt.

Repair happens only when all of these hold; anything else is `not-applicable`
and nothing is touched:
- the root helper's journal says `committed`, the installed binary is that
  release and `previous` is the binary it replaced;
- the installed runtime's version is not the harness release's, and the
  previous runtime's version is.

Then, as the task user, staged releases are discarded (the next start would
install them in place again) together with any pending swap request, and the
standard `rollback.json` is written. The root helper restores `previous`
(normally started by its path unit; run here too so the result is known before
this exits). Prints one JSON object:
{"outcome":"rolled-back"|"not-applicable"|"failed","code",...}.

Paths can be overridden with ZUSE_LAUNCH_REPAIR_* environment variables for
the test suite; the server sets none.
"""
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import time
from pathlib import Path

SHA = re.compile(r'[a-f0-9]{64}\Z')
SAFE = re.compile(r'[A-Za-z0-9_.:-]{1,160}\Z')
TOKEN = re.compile(r'[A-Za-z0-9._+:-]{1,128}\Z')
JSON_LIMIT = 65536
BINARY_LIMIT = 512 * 1024 * 1024
# The server bounds the whole run (launch_repair.rs REPAIR_TIMEOUT) inside the
# lifecycle job's lease; the rollback is one local copy and rename.
HELPER_WAIT_SECONDS = 30


def env(name, default):
    return os.environ.get('ZUSE_LAUNCH_REPAIR_' + name, default)


STATE = Path(env('STATE_DIR', '/var/lib/zuse/runtime-update'))
INSTALLED = Path(env('INSTALLED', '/usr/local/bin/zuse-cloud-runtime'))
PREVIOUS = Path(env('PREVIOUS', '/opt/zuse/runtime/previous'))
DATA = Path(env('DATA_DIR', '/home/zuse/.zuse-data'))
HARNESS = Path(env('HARNESS_ROOT', '/opt/zuse/harnesses'))
HELPER = Path(env('HELPER', '/usr/local/libexec/zuse/runtime-update.py'))
TASK_UID = int(env('TASK_UID', '10001'))
TASK_GID = int(env('TASK_GID', '10001'))


class Stop(Exception):
    def __init__(self, outcome, code):
        super().__init__(code)
        self.outcome = outcome
        self.code = code


def not_applicable(code):
    return Stop('not-applicable', code)


def failed(code):
    return Stop('failed', code)


def read_json(path):
    """A small regular file, never through a final symlink."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    except OSError:
        return None
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            return None
        raw = os.read(fd, JSON_LIMIT + 1)
    finally:
        os.close(fd)
    if len(raw) > JSON_LIMIT:
        return None
    try:
        return json.loads(raw)
    except ValueError:
        return None


def sha256(path):
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        return None
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            return None
        digest, size = hashlib.sha256(), 0
        while block := os.read(fd, 1048576):
            size += len(block)
            if size > BINARY_LIMIT:
                return None
            digest.update(block)
        return digest.hexdigest()
    finally:
        os.close(fd)


def as_task():
    """subprocess arguments that drop root to the task user."""
    if os.geteuid() == 0 and TASK_UID != 0:
        return {'user': TASK_UID, 'group': TASK_GID, 'extra_groups': []}
    return {}


def version(binary):
    """The last word of `<binary> --version`, exactly as the launcher reads it."""
    try:
        out = subprocess.run([str(binary), '--version'], capture_output=True, timeout=10,
                             env={'PATH': '/usr/bin:/bin'}, cwd='/', check=False, **as_task())
    except (OSError, subprocess.SubprocessError):
        return None
    words = out.stdout.decode('utf-8', 'replace').split()
    if out.returncode != 0 or not words or not TOKEN.fullmatch(words[-1]):
        return None
    return words[-1]


def harness_version():
    """`runtimeVersion` of the harness release this workspace is pinned to."""
    pin = read_json(DATA / 'cloud-harness-release.json')
    release = pin.get('release') if isinstance(pin, dict) else None
    if not isinstance(release, str) or not SHA.fullmatch(release):
        return None
    manifest = read_json(HARNESS / 'releases' / release / 'manifest.json')
    value = manifest.get('runtimeVersion') if isinstance(manifest, dict) else None
    return value if isinstance(value, str) and TOKEN.fullmatch(value) else None


# Runs as the task user: it only touches the task user's own update files.
TASK_STEP = r'''
import json,os,shutil,stat,sys
root,update,sha=sys.argv[1:4]
def directory(path):
    fd=os.open('/',os.O_RDONLY|os.O_DIRECTORY|os.O_CLOEXEC)
    for part in path.split('/')[1:]:
        if not part:continue
        nxt=os.open(part,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=fd)
        os.close(fd);fd=nxt
    return fd
fd=directory(root)
try:
    try:staged=os.open('staged',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=fd)
    except FileNotFoundError:staged=None
    if staged is not None:
        try:
            for name in os.listdir(staged):
                meta=os.stat(name,dir_fd=staged,follow_symlinks=False)
                if not stat.S_ISDIR(meta.st_mode):
                    os.unlink(name,dir_fd=staged);continue
                inner=os.open(name,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=staged)
                try:
                    for child in os.listdir(inner):os.unlink(child,dir_fd=inner)
                finally:os.close(inner)
                os.rmdir(name,dir_fd=staged)
        finally:os.close(staged)
    for name in ('request.json',):
        try:os.unlink(name,dir_fd=fd)
        except FileNotFoundError:pass
    raw=json.dumps({'updateId':update,'sha256':sha},separators=(',',':')).encode()
    tmp='.rollback-repair.tmp'
    try:os.unlink(tmp,dir_fd=fd)
    except FileNotFoundError:pass
    out=os.open(tmp,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,0o600,dir_fd=fd)
    try:
        os.write(out,raw);os.fsync(out)
    finally:os.close(out)
    os.rename(tmp,'rollback.json',src_dir_fd=fd,dst_dir_fd=fd);os.fsync(fd)
finally:os.close(fd)
'''


def task_step(update, sha):
    try:
        out = subprocess.run([sys.executable, '-I', '-B', '-c', TASK_STEP, str(DATA / 'runtime-updates'), update, sha],
                             capture_output=True, timeout=30, env={'PATH': '/usr/bin:/bin'}, cwd='/', check=False,
                             **as_task())
    except (OSError, subprocess.SubprocessError):
        raise failed('runtime_launch_repair_request_failed') from None
    if out.returncode != 0:
        raise failed('runtime_launch_repair_request_failed')


def run_helper(update, previous):
    """Start the root helper and wait until the rollback is recorded."""
    deadline = time.monotonic() + HELPER_WAIT_SECONDS
    try:
        subprocess.run([sys.executable, '-I', '-B', str(HELPER), 'run'], capture_output=True,
                       timeout=HELPER_WAIT_SECONDS, cwd='/', check=False)
    except (OSError, subprocess.SubprocessError):
        pass  # its path unit may own the run; the result file decides
    while True:
        result = read_json(STATE / 'result.json')
        if (isinstance(result, dict) and result.get('updateId') == update
                and result.get('outcome') in ('rolled-back', 'failed')):
            if result['outcome'] != 'rolled-back' or sha256(INSTALLED) != previous:
                raise failed('runtime_launch_repair_rollback_failed')
            return
        if time.monotonic() > deadline:
            raise failed('runtime_launch_repair_rollback_unconfirmed')
        time.sleep(0.25)


def repair():
    journal = read_json(STATE / 'journal.json')
    if not isinstance(journal, dict) or journal.get('phase') != 'committed':
        raise not_applicable('runtime_update_not_committed')
    sha, previous, update = journal.get('sha256'), journal.get('previousSha256'), journal.get('updateId')
    if not all(isinstance(v, str) for v in (sha, previous, update)) or not (
            SHA.fullmatch(sha) and SHA.fullmatch(previous) and SAFE.fullmatch(update)):
        raise not_applicable('runtime_update_journal_invalid')
    if sha256(INSTALLED) != sha:
        raise not_applicable('runtime_update_not_installed')
    if sha256(PREVIOUS) != previous:
        raise not_applicable('runtime_update_previous_missing')
    expected = harness_version()
    if expected is None:
        raise not_applicable('harness_release_unknown')
    installed = version(INSTALLED)
    if installed == expected:
        raise not_applicable('runtime_launchable')
    if version(PREVIOUS) != expected:
        raise not_applicable('previous_runtime_unlaunchable')
    task_step(update, sha)
    run_helper(update, previous)
    return {'outcome': 'rolled-back', 'updateId': update, 'fromVersion': installed, 'toVersion': expected,
            'fromSha256': sha, 'toSha256': previous}


def main():
    os.umask(0o022)
    try:
        result = repair()
    except Stop as stop:
        result = {'outcome': stop.outcome, 'code': stop.code}
    except Exception:  # never leave without a result line
        result = {'outcome': 'failed', 'code': 'runtime_launch_repair_failed'}
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
