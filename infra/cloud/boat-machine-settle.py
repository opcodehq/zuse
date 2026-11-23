"""Boat: settle the computer before the fresh boot helper starts or adopts the supervisor.

Runs as root via sudo, right before the verified loader, in the same provider
command (`settle && loader`). It prints nothing when the computer is settled,
so the helper's receipt stays the command's only output.

Right after create or a cold resume, Boat serves /home/zuse, /opt/zuse,
/var/lib/zuse and /usr from temporary lazily restored mounts, then swaps each
for the local disk a few seconds later. A supervisor started before the swap
keeps its lock and control socket on the retired mount: the files at the
visible paths are different ones, the helper refuses with
`fresh_boot_visible_lock_not_owned` and nothing can reach the supervisor.

1. Until every task directory is on the root filesystem, answer
   `machine_disk_settling` (exit 1). The control plane treats it like the
   loader's own `path_unsafe`: not yet ready, retried within its deadline.
2. If the supervisor is running but its published lock is not the visible
   file (it was taken before the swap), restart that one unit, but only while
   it runs no child (no runtime, no recovery). The helper then verifies the
   new resident exactly as before. With a child still running, refuse with
   `supervisor_lock_stale_with_child` (permanent; nothing is interrupted).

Arguments: the number of seconds to wait for the swap (0-30).
"""
import json, os, re, subprocess, sys, time
from pathlib import Path

VERSION = 1
UNIT = 'zuse-runtime-supervisor.service'
SETTLED_PATHS = ('/home/zuse', '/home/zuse/.zuse-data', '/home/zuse/workspace', '/opt/zuse',
                 '/var/lib/zuse', '/usr/local/lib/zuse', '/usr/local/bin')
IPC = '/home/zuse/.zuse-data/.runtime-supervision'
POLL_SECONDS = 0.25
RESTART_SECONDS = 20
PUBLISH_SECONDS = 10


class Refused(Exception):
    pass


class NotReady(Exception):
    pass


def disk_settled(paths=SETTLED_PATHS, root='/'):
    """True once every existing task directory is on the same device as `/`."""
    try:
        device = os.stat(root).st_dev
    except OSError:
        return False
    for path in paths:
        try:
            if os.stat(path).st_dev != device:
                return False
        except FileNotFoundError:
            continue
        except OSError:
            return False
    return True


class Units:
    def state(self):
        result = subprocess.run(['systemctl', 'show', UNIT, '--property=MainPID,ActiveState,SubState,Job'],
                                capture_output=True, text=True, timeout=5, env={'PATH': '/usr/bin:/bin'})
        if result.returncode != 0 or len(result.stdout) > 4096:
            raise Refused('settle_unit_state_unavailable')
        return dict(line.split('=', 1) for line in result.stdout.splitlines() if '=' in line)

    def restart(self):
        subprocess.run(['systemctl', 'restart', UNIT], capture_output=True, check=True, timeout=RESTART_SECONDS,
                       env={'PATH': '/usr/bin:/bin'})


def children(pid, proc=Path('/proc')):
    """Direct children of `pid` (the supervisor's runtime or recovery)."""
    try:
        return [int(v) for v in (proc / str(pid) / 'task' / str(pid) / 'children').read_text().split()]
    except FileNotFoundError:
        return []


def holds(pid, identity, proc=Path('/proc')):
    """`pid` has an open file descriptor on the file `identity` (dev, ino)."""
    for fd in (proc / str(pid) / 'fd').iterdir():
        try:
            m = fd.stat()
        except (FileNotFoundError, PermissionError):
            continue
        if (m.st_dev, m.st_ino) == identity:
            return True
    return False


def stale_lock(pid, ipc=Path(IPC), proc=Path('/proc')):
    """The running supervisor `pid` has published itself (supervisor.json names
    it), yet holds no descriptor on the visible lock file. A supervisor that is
    still starting has not published itself and is left alone."""
    try:
        record = json.loads((ipc / 'supervisor.json').read_bytes()[:4097])
    except (OSError, ValueError):
        return False
    if not isinstance(record, dict) or record.get('residentPid') != pid:
        return False
    try:
        m = (ipc / 'supervisor.lock').lstat()
    except FileNotFoundError:
        return True
    return not holds(pid, (m.st_dev, m.st_ino), proc)


def published(pid, ipc=Path(IPC)):
    """The supervisor `pid` has published its identity (lock and socket made)."""
    try:
        record = json.loads((ipc / 'supervisor.json').read_bytes()[:4097])
    except (OSError, ValueError):
        return False
    return isinstance(record, dict) and record.get('residentPid') == pid


def settle(wait_seconds, units=None, settled=disk_settled, stale=stale_lock, child_pids=children,
           ready=published, clock=time.monotonic, sleep=time.sleep):
    units = units or Units()
    deadline = clock() + wait_seconds
    while not settled():
        if clock() >= deadline:
            raise NotReady('machine_disk_settling')
        sleep(POLL_SECONDS)
    state = units.state()
    if state.get('ActiveState') != 'active' or state.get('SubState') != 'running' or state.get('Job'):
        return 'unchanged'
    try:
        pid = int(state.get('MainPID', '0'))
    except ValueError:
        raise Refused('settle_unit_state_unavailable') from None
    if pid <= 1 or not stale(pid):
        return 'unchanged'
    if child_pids(pid):
        raise Refused('supervisor_lock_stale_with_child')
    units.restart()
    print(json.dumps({'event': 'supervisor_restarted_after_disk_swap', 'previousPid': pid}), file=sys.stderr)
    # Save the helper a round trip: give the new resident a moment to publish
    # itself. If it is slower, the helper answers `starting` and is asked again.
    deadline = clock() + PUBLISH_SECONDS
    while clock() < deadline:
        raw = units.state().get('MainPID', '0')
        current = int(raw) if raw.isdigit() else 0
        if current > 1 and current != pid and ready(current):
            break
        sleep(POLL_SECONDS)
    return 'restarted'


def main(argv):
    if os.geteuid() != 0:
        raise Refused('settle_root_required')
    if len(argv) != 2 or not re.fullmatch(r'[0-9]{1,2}', argv[1]) or int(argv[1]) > 30:
        raise Refused('settle_arguments_invalid')
    settle(int(argv[1]))


if __name__ == '__main__':
    try:
        main(sys.argv)
    except NotReady as error:
        print(json.dumps({'version': VERSION, 'status': 'rejected', 'code': str(error)}))
        raise SystemExit(1)
    except Exception as error:
        code = str(error) if isinstance(error, Refused) else 'settle_failed'
        print(json.dumps({'version': VERSION, 'status': 'rejected', 'code': code}))
        raise SystemExit(1)
