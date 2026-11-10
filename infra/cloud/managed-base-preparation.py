#!/usr/bin/env python3
"""Account-free provider build preparation and read-only template readiness.

The build step owns and reaps a temporary Supervisor. The template's foreground
start command owns the final resident Supervisor. A successful layout is never
reported as prepared; only the ordinary image/native gates produce this receipt.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import time

DIGEST = re.compile(r'[a-f0-9]{64}\Z')


def require(ok, code):
    if not ok:
        raise ValueError(code)


def file_hash(path):
    value = hashlib.sha256()
    with path.open('rb') as source:
        while block := source.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


def protected(path, owner=0):
    m = path.lstat()
    require(stat.S_ISREG(m.st_mode) and m.st_uid == owner and not m.st_mode & 0o022,
            'managed_base_file_unsafe')
    return dict(inode=str(m.st_ino), device=str(m.st_dev), bytes=m.st_size,
                modifiedNs=str(m.st_mtime_ns))


def load(path, owner=0):
    protected(path, owner)
    require(path.stat().st_size <= 65536, 'managed_base_document_too_large')
    return json.loads(path.read_bytes())


def atomic(path, value):
    fd, name = tempfile.mkstemp(prefix='.base-prepared-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as output:
            os.fchmod(output.fileno(), 0o400)
            output.write(json.dumps(value, sort_keys=True, separators=(',', ':')).encode() + b'\n')
            output.flush()
            os.fsync(output.fileno())
        os.replace(name, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        Path(name).unlink(missing_ok=True)


class Paths:
    def __init__(self, prefix=Path('/')):
        self.root = prefix
        self.bundle = prefix / 'opt/zuse-base-input'
        self.home = prefix / 'home/zuse'
        self.data = self.home / '.zuse-data'
        self.preparation = prefix / 'opt/zuse/preparation'
        self.manifest = self.preparation / 'status/manifest.json'
        self.receipt = self.preparation / 'base-prepared.json'
        self.layout = self.preparation / 'layout-receipt.json'
        self.runtime = prefix / 'usr/local/bin/zuse-cloud-runtime'
        self.supervisor = prefix / 'usr/local/lib/zuse/runtime-supervisor.py'
        self.installer = prefix / 'usr/local/lib/zuse/managed-base-install.sh'


def pid_identity(pid):
    text = Path(f'/proc/{pid}/stat').read_text()
    uid = Path(f'/proc/{pid}').stat().st_uid
    return uid, text.rsplit(')', 1)[1].split()[19]


def owned_processes(uid):
    found = []
    for p in Path('/proc').iterdir():
        if not p.name.isdigit():
            continue
        try:
            if p.stat().st_uid == uid:
                found.append(int(p.name))
        except FileNotFoundError:
            pass
    return sorted(found)


def ping(paths):
    result = subprocess.run(
        ['/usr/bin/sudo', '-n', '-u', 'zuse', '/usr/bin/env', 'HOME=/home/zuse',
         '/usr/bin/python3', str(paths.supervisor), 'ping'],
        stdin=subprocess.DEVNULL, capture_output=True, timeout=3,
        env={'PATH': '/usr/bin:/bin', 'HOME': '/root'})
    require(result.returncode == 0 and len(result.stdout) <= 8192,
            'managed_base_resident_supervisor_unavailable')
    return json.loads(result.stdout)


def clean_home(paths, uid=10001):
    require(not paths.data.is_symlink(), 'managed_base_data_symlink')
    if paths.data.exists():
        m=paths.data.lstat()
        require(stat.S_ISDIR(m.st_mode) and m.st_uid==uid and not m.st_mode&0o077,
                'managed_base_data_unsafe')
        require(all(p.name == '.runtime-supervision' for p in paths.data.iterdir()),
                'managed_base_contains_task')
    for relative in ('.codex/auth.json', '.claude/.credentials.json', '.grok/auth.json',
                     '.config/gh', '.zuse-github-cli', '.git-credentials', '.netrc'):
        require(not (paths.home / relative).exists() and not (paths.home / relative).is_symlink(),
                'managed_base_contains_credentials')


def validate_manifest(value, expected):
    require(value.get('managedBase') is True and value.get('credentialsRemoved') is True
            and value.get('repositories') == [], 'managed_base_manifest_not_clean')
    require(value.get('rustRuntimeSupervisorVersion') == 1,
            'managed_base_supervisor_unattested')
    require(value.get('rustCommandAdmissionVersion') == 2
            and value.get('commandAdmission', {}).get('runtimeSha256') == expected['runtimeSha256'],
            'managed_base_command_admission_unattested')
    require(value.get('rustGithubCliVersion') == 1
            and value.get('githubCli', {}).get('loginShellResolvedManaged') is True,
            'managed_base_github_cli_unattested')


def check_ping(value, uid=10001, expected_pid=None):
    require(value.get('version') == 1 and type(value.get('residentPid')) is int,
            'managed_base_resident_supervisor_invalid')
    require(value.get('bound') is False and value.get('childAlive') is False
            and value.get('recoveryAlive') is False, 'managed_base_supervisor_not_idle')
    pid = value['residentPid']
    require(expected_pid is None or pid == expected_pid, 'managed_base_supervisor_owner_changed')
    require(pid_identity(pid)[0] == uid, 'managed_base_supervisor_uid_invalid')
    return pid


def stop_owned(process):
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=3)
        raise ValueError('managed_base_preparation_owner_stop_uncertain')


def prepare(paths, expected, *, owner=0, uid=10001, ping_fn=ping,
            processes=owned_processes, spawn=subprocess.Popen, run=subprocess.run):
    require(not paths.receipt.exists() and not paths.receipt.is_symlink(), 'managed_base_preparation_already_attempted')
    layout = load(paths.layout, owner)
    require(layout.get('version') == 1 and layout.get('status') == 'layout-only',
            'managed_base_layout_receipt_invalid')
    for key in ('bundleSha256', 'runtimeSha256', 'harnessRelease'):
        require(layout.get(key) == expected[key], 'managed_base_layout_binding_mismatch')
    protected(paths.runtime, owner)
    protected(paths.supervisor, owner)
    require(file_hash(paths.runtime) == expected['runtimeSha256']
            and file_hash(paths.supervisor) == expected['supervisorSha256'],
            'managed_base_installed_artifact_mismatch')
    clean_home(paths, uid)
    require(processes(uid) == [], 'managed_base_preparation_foreign_process')
    # Persist before any process: a failed/uncertain attempt is never retried
    # automatically by a readiness poll or repeated publication invocation.
    atomic(paths.receipt, dict(version=1, status='preparing', **expected))
    environment = {'PATH': '/usr/bin:/bin', 'HOME': '/home/zuse',
                   'ZUSE_RUNTIME_PROVIDER': expected['providerId']}
    process = spawn(['/usr/bin/python3', str(paths.supervisor), 'serve'],
                    env=environment, stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    start_new_session=True)
    try:
        deadline = time.monotonic() + 10
        while True:
            require(process.poll() is None, 'managed_base_preparation_owner_exited')
            try:
                check_ping(ping_fn(paths), uid, process.pid)
                break
            except (ValueError, OSError, subprocess.SubprocessError):
                require(time.monotonic() < deadline, 'managed_base_preparation_owner_unavailable')
                time.sleep(.1)
        result = run(['/bin/bash', str(paths.installer), str(paths.bundle), 'prepare'],
                     env=environment, stdin=subprocess.DEVNULL,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=600)
        require(result.returncode == 0, 'managed_base_image_preparation_failed')
        check_ping(ping_fn(paths), uid, process.pid)
        require(processes(uid) == [process.pid], 'managed_base_preparation_unowned_process')
        require(not paths.bundle.exists(), 'managed_base_input_not_removed')
        clean_home(paths, uid)
        manifest = load(paths.manifest, uid)
        validate_manifest(manifest, expected)
    finally:
        stop_owned(process)
    require(processes(uid) == [], 'managed_base_preparation_processes_remain')
    atomic(paths.receipt, dict(version=1, status='prepared-uncertified', **expected,
                              manifestSha256=file_hash(paths.manifest),
                              runtimeIdentity=protected(paths.runtime, owner),
                              supervisorIdentity=protected(paths.supervisor, owner)))


def ready(paths, expected, *, owner=0, uid=10001, ping_fn=ping):
    receipt = load(paths.receipt, owner)
    require(receipt.get('version') == 1 and receipt.get('status') == 'prepared-uncertified',
            'managed_base_preparation_incomplete')
    require(all(receipt.get(k) == v for k, v in expected.items()), 'managed_base_ready_binding_mismatch')
    # OCI unpacking can change inode/device. Bind exact bytes, and compare
    # identities only within this read, never across the build/restore boundary.
    for path,key in ((paths.runtime,'runtimeSha256'),(paths.supervisor,'supervisorSha256')):
        before=protected(path,owner)
        require(before['bytes'] <= 512*1024*1024,'managed_base_artifact_too_large')
        require(file_hash(path)==expected[key] and protected(path,owner)==before,
                'managed_base_ready_artifact_changed')
    require(file_hash(paths.manifest) == receipt.get('manifestSha256'), 'managed_base_ready_manifest_changed')
    validate_manifest(load(paths.manifest, uid), expected)
    require(not paths.bundle.exists(), 'managed_base_input_not_removed')
    clean_home(paths, uid)
    value = ping_fn(paths)
    check_ping(value, uid)
    return dict(version=1, status='resident-ready', certified=False,
                residentPid=value['residentPid'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('prepare', 'ready'))
    parser.add_argument('--provider', choices=('e2b','daytona'), required=True)
    for key in ('bundle', 'runtime', 'supervisor'):
        parser.add_argument('--' + key + '-sha256', required=True)
    parser.add_argument('--harness-release', required=True)
    args = parser.parse_args()
    require(os.geteuid() == 0, 'managed_base_preparation_requires_root')
    expected = dict(bundleSha256=args.bundle_sha256, runtimeSha256=args.runtime_sha256,
                    supervisorSha256=args.supervisor_sha256, harnessRelease=args.harness_release, providerId=args.provider)
    require(all(isinstance(v, str) and DIGEST.fullmatch(v) for k,v in expected.items() if k!='providerId'),
            'managed_base_preparation_pin_invalid')
    if args.action == 'prepare':
        prepare(Paths(), expected)
    else:
        print(json.dumps(ready(Paths(), expected), separators=(',', ':')))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        code = str(error) if isinstance(error, ValueError) and str(error).startswith('managed_base_') else 'managed_base_preparation_failed'
        print(json.dumps(dict(version=1, status='not-ready', code=code)), file=sys.stderr)
        raise SystemExit(1)
