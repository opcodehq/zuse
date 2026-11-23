"""Boat first start, waited for inside the computer. Runs as root via sudo.

Boat attaches the image lazily. For several seconds after create its lazy
filesystem shows /usr/local and /usr/local/lib owned by the provider's exec
user, and the fresh boot loader refuses (`fresh_boot_loader_path_unsafe`).
The control plane sends this program once, right after create, instead of
asking again and again from the server:

1. Take the uploaded boot credential out of /tmp at once into a root-only
   directory on /run (tmpfs, not lazily attached), so the exec user can
   neither read nor replace it while we wait.
2. Hand the wait to a transient systemd unit and answer immediately. The unit
   needs no network or server from here on.
3. Wait for the loader's own path rule to hold and for Boat's lazy restore to
   hand the task directories over to the local disk (inotify on the path and
   mount-table changes, a 250 ms fallback check, a hard deadline).
4. Run the unchanged loader and fresh boot helper, accept only a ready receipt
   that passes the same checks the server applies, then run the server's
   launch lines, which install the credential for the task user privately.

Status is kept in <root>/<operation>/status.json (root 0600). The server
reads it only as a fallback, if the runtime has not enrolled in time. Every
step is bounded: the wait and the helper by `deadlineSeconds`, the launch by
`launchSeconds`, so the unit has finished by their sum. The server's last
read also stops the unit and removes the credential before it starts the
runtime itself.
"""
import base64, ctypes, hashlib, json, os, re, select, stat, subprocess, sys, time
from pathlib import Path

VERSION = 1
ROOT = Path('/run/zuse-fresh-start')
HELPER = Path('/usr/local/lib/zuse/managed-base-fresh-boot.py')
FALLBACK_SECONDS = 0.25
REJECTED_RETRY_SECONDS = 0.5
HELPER_SECONDS = 40
UNIT_SUBMIT_SECONDS = 15
# Helper refusals that clear on their own right after the helper's own start:
# between systemd's fork and its exec of the supervisor, the new process has
# the task user's ids but still the service manager's command line.
# Every other refusal (a hash mismatch, for one) ends the attempt at once.
TRANSIENT_REFUSALS = frozenset({'fresh_boot_resident_not_owned_idle',
                                'fresh_boot_resident_program_changed'})
# Directories the supervisor and the runtime keep open files, locks and
# sockets in. Right after create, Boat serves each of them from a temporary
# lazily restored mount, then swaps it for the local disk a few seconds later
# (about 5-10 s after the restore). A lock, socket or working directory taken
# before the swap stays on the retired mount: the visible path is a different
# file. Nothing may start until every one of them is on the root filesystem.
SETTLED_PATHS = ('/home/zuse', '/home/zuse/.zuse-data', '/home/zuse/workspace', '/opt/zuse',
                 '/var/lib/zuse', '/usr/local/lib/zuse', '/usr/local/bin')
SAFE_ID = re.compile(r'[A-Za-z0-9_.:-]{1,160}\Z')
HEX64 = re.compile(r'[a-f0-9]{64}\Z')
BINDING = ('version', 'provider', 'purpose', 'operationId', 'workspaceId', 'generation',
           'storageIncarnationId', 'sourceMachineId', 'imagePins')
# inotify(7) event bits.
IN_ATTRIB, IN_MOVED_TO, IN_CREATE, IN_DELETE_SELF, IN_MOVE_SELF, IN_UNMOUNT = (
    0x4, 0x80, 0x100, 0x400, 0x800, 0x2000)


class Refused(Exception):
    pass


def require(value, code):
    if not value:
        raise Refused(code)


def path_safe(path, owner=0):
    """The fresh boot loader's rule: every ancestor owned by `owner`, no
    symlink, not group/other writable. Missing means not yet."""
    try:
        for ancestor in (path, *path.parents):
            m = ancestor.lstat()
            if m.st_uid != owner or stat.S_ISLNK(m.st_mode) or m.st_mode & 0o022:
                return False
    except OSError:
        return False
    return True


def disk_settled(paths=SETTLED_PATHS, root='/'):
    """True once every existing task directory is on the root filesystem
    (the same device as `/`), i.e. no longer served by a temporary mount."""
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


class Watch:
    """Wakes on attribute/move events on the watched directories and on any
    mount-table change (Boat swaps the lazy mount for the local disk). Every
    wait is also bounded by the fallback interval, so a missed or impossible
    event (a watch on a replaced mount) costs at most that interval."""

    def __init__(self, directories, mountinfo='/proc/self/mountinfo'):
        self.directories = [str(d) for d in directories]
        self.poll = select.poll()
        self.fd = -1
        self.mounts = None
        try:
            self.libc = ctypes.CDLL(None, use_errno=True)
            self.fd = self.libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
        except (OSError, AttributeError):
            self.fd = -1
        if self.fd >= 0:
            self.poll.register(self.fd, select.POLLIN)
            self.arm()
        try:
            self.mounts = open(mountinfo, 'rb')
            self.mounts.read()
            self.poll.register(self.mounts.fileno(), select.POLLPRI | select.POLLERR)
        except OSError:
            self.mounts = None

    def arm(self):
        # Re-adding is cheap and picks up a directory that now lives on a new mount.
        mask = IN_ATTRIB | IN_MOVED_TO | IN_CREATE | IN_DELETE_SELF | IN_MOVE_SELF | IN_UNMOUNT
        for directory in self.directories:
            if os.path.isdir(directory):
                self.libc.inotify_add_watch(self.fd, directory.encode(), mask)

    def wait(self, seconds):
        woke = False
        for fd, _ in self.poll.poll(max(0, int(seconds * 1000))):
            woke = True
            if fd == self.fd:
                try:
                    while os.read(self.fd, 65536):
                        pass
                except BlockingIOError:
                    pass
            elif self.mounts is not None and fd == self.mounts.fileno():
                self.mounts.seek(0)
                self.mounts.read()
        if self.fd >= 0:
            self.arm()
        return woke

    def close(self):
        if self.fd >= 0:
            os.close(self.fd)
        if self.mounts is not None:
            self.mounts.close()


def wait_until(ready, deadline, watch, clock=time.monotonic, fallback=FALLBACK_SECONDS):
    """True as soon as `ready()` holds; False at the deadline."""
    while True:
        if ready():
            return True
        left = deadline - clock()
        if left <= 0:
            return False
        watch.wait(min(fallback, left))


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def same_json(a, b):
    """JSON equality that also compares types (`true` is not `1`)."""
    if type(a) is not type(b):
        return False
    if isinstance(a, dict):
        return a.keys() == b.keys() and all(same_json(a[k], b[k]) for k in a)
    if isinstance(a, list):
        return len(a) == len(b) and all(same_json(x, y) for x, y in zip(a, b))
    return a == b


def accepted_receipt(raw, request):
    """The same acceptance the server applies to a fresh boot receipt."""
    try:
        value = json.loads(raw)
    except ValueError:
        return None
    if not isinstance(value, dict) or any(k not in value or not same_json(value[k], request[k]) for k in BINDING):
        return None
    resident = value.get('resident')
    if not isinstance(resident, dict):
        return None
    pid, ticks = resident.get('residentPid'), resident.get('residentStartTicks')
    nonce = resident.get('bootNonceSha256')
    if (value.get('status') != 'ready'
            or value.get('requestSha256') != hashlib.sha256(canonical(request)).hexdigest()
            or resident.get('ipcVerified') is not True
            or resident.get('unbound') is not True
            or resident.get('childAlive') is not False
            or resident.get('recoveryAlive') is not False
            or type(pid) is not int or pid <= 1
            or type(ticks) is not int or ticks <= 0
            or not isinstance(nonce, str) or not HEX64.fullmatch(nonce)
            or type(value.get('unitStartSubmitted')) is not bool):
        return None
    return value


def helper_state(result):
    """`ready`, `retry` (loader path unsafe, or resident still starting) or `rejected`."""
    try:
        value = json.loads(result.stdout)
    except ValueError:
        return 'rejected'
    if (result.returncode == 1 and len(result.stdout) <= 256 and isinstance(value, dict)
            and value.get('status') == 'rejected' and value.get('code') == 'fresh_boot_loader_path_unsafe'):
        return 'retry'
    if result.returncode != 0 or len(result.stdout) > 65536 or not isinstance(value, dict):
        return 'rejected'
    if value.get('status') in ('pending', 'starting'):
        return 'retry'
    return 'ready'


def write_private(path, data):
    tmp = path.with_name('.write-' + os.urandom(8).hex())
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        with os.fdopen(fd, 'wb') as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    finally:
        tmp.unlink(missing_ok=True)


def helper_code(result):
    """The helper's or loader's fixed refusal code, when it printed one."""
    try:
        code = json.loads(result.stdout).get('code')
    except (ValueError, AttributeError):
        return 'unparsed'
    return code if isinstance(code, str) and re.fullmatch(r'[a-z0-9_]{1,80}', code) else 'unknown'


def status(directory, operation, state, code=None, receipt=None, helper=None):
    value = {'version': VERSION, 'operationId': operation, 'status': state,
             'updatedAtMs': int(time.time() * 1000)}
    if code:
        value['code'] = code
    if helper:
        value['helperCode'] = helper
    if receipt is not None:
        value['receipt'] = receipt
    write_private(directory / 'status.json', canonical(value) + b'\n')


def read_status(directory):
    try:
        return json.loads((directory / 'status.json').read_bytes())
    except (OSError, ValueError):
        return {}


def validated(args):
    require(isinstance(args, dict) and args.get('version') == VERSION, 'fresh_start_arguments_invalid')
    for key in ('operationId', 'staged', 'starter', 'loader', 'helperSha256', 'request', 'launch'):
        require(isinstance(args.get(key), str) and args[key], 'fresh_start_arguments_invalid')
    require(SAFE_ID.fullmatch(args['operationId']) and HEX64.fullmatch(args['helperSha256']), 'fresh_start_arguments_invalid')
    require(args['staged'] == '/tmp/zuse-rust-boot-' + args['operationId'], 'fresh_start_staged_path_invalid')
    require(type(args.get('deadlineSeconds')) is int and 5 <= args['deadlineSeconds'] <= 300, 'fresh_start_arguments_invalid')
    require(type(args.get('launchSeconds')) is int and 5 <= args['launchSeconds'] <= 300, 'fresh_start_arguments_invalid')
    request = json.loads(base64.b64decode(args['request'], validate=True))
    require(isinstance(request, dict) and request.get('provider') == 'boat'
            and request.get('purpose') == 'initial-allocation'
            and request.get('operationId') == args['operationId'], 'fresh_start_request_invalid')
    return args, request


def private_root(root):
    """A root-only directory on a root-owned tmpfs path."""
    require(path_safe(root.parent), 'fresh_start_run_unsafe')
    try:
        root.mkdir(mode=0o700)
    except FileExistsError:
        pass
    m = root.lstat()
    require(stat.S_ISDIR(m.st_mode) and m.st_uid == 0 and stat.S_IMODE(m.st_mode) == 0o700, 'fresh_start_root_unsafe')


def take_credential(staged, target):
    """Move the uploaded credential into the root-only directory now."""
    try:
        fd = os.open(staged, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        raise Refused('fresh_start_credential_missing') from None
    try:
        m = os.fstat(fd)
        require(stat.S_ISREG(m.st_mode) and m.st_nlink == 1 and 0 < m.st_size <= 4096, 'fresh_start_credential_invalid')
        data = os.read(fd, 4097)
        require(0 < len(data) <= 4096, 'fresh_start_credential_invalid')
    finally:
        os.close(fd)
    os.unlink(staged)
    write_private(target, data)


def unit_name(operation):
    return 'zuse-fresh-start-' + hashlib.sha256(operation.encode()).hexdigest()[:16]


def unit_loaded(unit, run=None):
    """True/False when the service manager answers, None when it does not."""
    run = run or subprocess.run
    try:
        shown = run(['systemctl', 'show', '--property=LoadState', '--value', unit],
                    capture_output=True, timeout=UNIT_SUBMIT_SECONDS, env={'PATH': '/usr/bin:/bin'})
    except subprocess.TimeoutExpired:
        return None
    if shown.returncode != 0:
        return None
    return shown.stdout.strip() == b'loaded'


def dispatch(args, root=ROOT):
    """The provider command: secure the credential, start the waiter, answer."""
    args, _ = validated(args)
    private_root(root)
    directory = root / args['operationId']
    try:
        directory.mkdir(mode=0o700)
    except FileExistsError:
        prior = read_status(directory).get('status')
        if prior in ('waiting', 'launching', 'launched'):
            # A repeated dispatch after an uncertain reply: the first is
            # already running and holds its own copy of the credential.
            Path(args['staged']).unlink(missing_ok=True)
            return {'status': 'accepted', 'existing': prior}
        for child in directory.iterdir():
            child.unlink()
    take_credential(Path(args['staged']), directory / 'boot')
    write_private(directory / 'arguments.json', canonical(args))
    # This same program, from the root-only directory, for the waiting unit.
    write_private(directory / 'start.py', args['starter'].encode())
    status(directory, args['operationId'], 'waiting')
    unit = unit_name(args['operationId'])
    try:
        started = subprocess.run(
            ['systemd-run', '--quiet', '--collect', '--unit=' + unit, '/usr/bin/python3', '-I', '-B',
             str(directory / 'start.py'), '--wait', str(directory)],
            capture_output=True, timeout=UNIT_SUBMIT_SECONDS, env={'PATH': '/usr/bin:/bin'}).returncode == 0
    except subprocess.TimeoutExpired:
        # The reply was lost, not necessarily the unit: ask the service manager.
        loaded = unit_loaded(unit, subprocess.run)
        if loaded is None:
            # Unknown either way. Keep the credential and the `waiting` record:
            # a repeat finds it, and the server's last read stops whatever
            # runs and removes the credential before its own start.
            raise Refused('fresh_start_unit_uncertain') from None
        # A unit that already finished is collected; its status moved on.
        started = loaded or read_status(directory).get('status') != 'waiting'
    if not started:
        (directory / 'boot').unlink(missing_ok=True)
        status(directory, args['operationId'], 'failed', 'fresh_start_unit_failed')
        raise Refused('fresh_start_unit_failed')
    return {'status': 'accepted'}


def run_start(directory, watch=None, clock=time.monotonic, run=subprocess.run, sleep=time.sleep):
    args, request = validated(json.loads((directory / 'arguments.json').read_bytes()))
    operation = args['operationId']
    deadline = clock() + args['deadlineSeconds']
    boot = directory / 'boot'
    watch = watch or Watch([HELPER.parent, *HELPER.parents])
    try:
        while True:
            if not wait_until(lambda: path_safe(HELPER) and disk_settled(), deadline, watch, clock):
                code = ('fresh_start_path_unsafe_until_deadline' if not path_safe(HELPER)
                        else 'fresh_start_disk_unsettled_until_deadline')
                return status(directory, operation, 'timeout', code)
            # The helper never outlives the deadline, so neither can a launch.
            left = deadline - clock()
            if left <= 0:
                return status(directory, operation, 'timeout', 'fresh_start_helper_pending_until_deadline')
            try:
                result = run(['/usr/bin/python3', '-I', '-B', '-c', args['loader'], args['helperSha256'], args['request']],
                             capture_output=True, timeout=min(HELPER_SECONDS, left), env={'PATH': '/usr/bin:/bin'})
            except subprocess.TimeoutExpired:
                return status(directory, operation, 'timeout', 'fresh_start_helper_pending_until_deadline')
            state = helper_state(result)
            if state == 'rejected':
                # Right after its own `systemctl start` the helper can see the
                # resident before it has switched to the task user
                # (`fresh_boot_resident_not_owned_idle`) or before it has run
                # the supervisor (`fresh_boot_resident_program_changed`). Retry only such
                # refusals, bounded by the deadline. The helper's journal makes
                # a retry observe the original start, never issue another.
                rejected = helper_code(result)
                if rejected not in TRANSIENT_REFUSALS or clock() + REJECTED_RETRY_SECONDS >= deadline:
                    return status(directory, operation, 'rejected', 'fresh_start_helper_rejected',
                                  helper=rejected)
                sleep(REJECTED_RETRY_SECONDS)
                continue
            if state == 'ready' and clock() >= deadline:
                return status(directory, operation, 'timeout', 'fresh_start_helper_pending_until_deadline')
            if state == 'ready':
                receipt = accepted_receipt(result.stdout, request)
                if receipt is None:
                    return status(directory, operation, 'rejected', 'fresh_start_receipt_invalid')
                break
            if clock() >= deadline:
                return status(directory, operation, 'timeout', 'fresh_start_helper_pending_until_deadline')
            watch.wait(min(FALLBACK_SECONDS, max(0, deadline - clock())))
        status(directory, operation, 'launching', receipt=receipt)
        try:
            launched = run(['/bin/sh', '-c', args['launch']], capture_output=True, timeout=args['launchSeconds'],
                           env={'PATH': '/usr/sbin:/usr/bin:/sbin:/bin'})
        except subprocess.TimeoutExpired:
            return status(directory, operation, 'failed', 'fresh_start_launch_timeout', receipt)
        if launched.returncode == 78:
            return status(directory, operation, 'rejected', 'fresh_start_runtime_hash_mismatch', receipt)
        if launched.returncode != 0:
            return status(directory, operation, 'failed', 'fresh_start_launch_failed', receipt)
        return status(directory, operation, 'launched', receipt=receipt)
    finally:
        boot.unlink(missing_ok=True)


def main(argv):
    require(os.geteuid() == 0, 'fresh_start_root_required')
    if len(argv) == 3 and argv[1] == '--wait':
        directory = Path(argv[2])
        require(directory.parent == ROOT and SAFE_ID.fullmatch(directory.name), 'fresh_start_arguments_invalid')
        try:
            run_start(directory)
        except Exception as error:
            code = str(error) if isinstance(error, Refused) else 'fresh_start_failed'
            (directory / 'boot').unlink(missing_ok=True)
            status(directory, directory.name, 'failed', code)
        return
    require(len(argv) == 2 and len(argv[1]) <= 98304, 'fresh_start_arguments_invalid')
    print(json.dumps(dispatch(json.loads(base64.b64decode(argv[1], validate=True)))))


if __name__ == '__main__':
    try:
        main(sys.argv)
    except Exception as error:
        code = str(error) if isinstance(error, Refused) else 'fresh_start_failed'
        print(json.dumps({'version': VERSION, 'status': 'rejected', 'code': code}))
        raise SystemExit(1)
