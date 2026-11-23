# Exercised by guest_start/tests.rs: the in-computer start's waiting,
# acceptance and credential handling, without root, systemd or a provider.
import base64, hashlib, json, os, stat, subprocess, sys, tempfile, threading, time
from pathlib import Path

ns = {'__name__': 'fresh_start_under_test'}
exec(compile(Path(sys.argv[1]).read_text(), 'managed-base-fresh-start.py', 'exec'), ns)
tmp = Path(tempfile.mkdtemp())


def check(condition, message):
    if not condition:
        raise SystemExit('FAILED: ' + message)


# The loader's path rule, for a chosen owner.
uid = os.getuid()
safe = tmp / 'a' / 'b'
safe.mkdir(parents=True)
check(not ns['path_safe'](safe), 'root-owned rule must refuse a non-root tree')
os.chmod(tmp / 'a', 0o775)
check(not ns['path_safe'](safe, owner=uid), 'group-writable ancestor must be unsafe')
os.chmod(tmp / 'a', 0o755)
link = tmp / 'link'
link.symlink_to(safe)
check(not ns['path_safe'](link, owner=uid), 'symlink must be unsafe')
check(not ns['path_safe'](tmp / 'missing', owner=uid), 'missing path is not yet safe')

# Boat's temporary restore mounts: a task directory on another device than the
# root is not settled yet; missing directories do not hold the start back.
same = tmp / 'same'
same.mkdir()
check(ns['disk_settled']((str(same), str(tmp / 'missing')), root=str(tmp)), 'same device is settled')
check(not ns['disk_settled']((str(same), '/proc/self'), root=str(tmp)), 'another device is not settled')
check(ns['SETTLED_PATHS'][:2] == ('/home/zuse', '/home/zuse/.zuse-data'), 'the supervisor directories are checked')

# A real attribute event wakes the wait long before the 5 s fallback interval.
watched = tmp / 'watched'
watched.mkdir()
os.chmod(watched, 0o775)
watch = ns['Watch']([watched])
started = time.monotonic()
threading.Timer(0.3, lambda: os.chmod(watched, 0o755)).start()
ok = ns['wait_until'](lambda: not (watched.stat().st_mode & 0o022), time.monotonic() + 10, watch, fallback=5)
took = time.monotonic() - started
check(ok and took < 2.5, f'event wake took {took:.2f}s')
watch.close()


# Fallback polling and the deadline, on a fake clock: each wait advances it.
class FakeClock:
    def __init__(self):
        self.now, self.waits = 0.0, []
    def __call__(self):
        return self.now
    def wait(self, seconds):
        self.waits.append(seconds)
        self.now += seconds
        return False

# A missed event (nothing watched changes) still ends within the fallback.
clock = FakeClock()
ok = ns['wait_until'](lambda: clock.now >= 0.6, 10, clock, clock, fallback=0.25)
check(ok and clock.now == 0.75 and clock.waits == [0.25] * 3, f'fallback: {clock.now} {clock.waits}')

# The hard deadline ends the wait with False, the last wait cut to what is left.
clock = FakeClock()
ok = ns['wait_until'](lambda: False, 0.6, clock, clock, fallback=0.25)
check(not ok and abs(clock.now - 0.6) < 1e-9 and len(clock.waits) == 3 and abs(clock.waits[-1] - 0.1) < 1e-9,
      f'deadline: {clock.now} {clock.waits}')

# Receipt acceptance mirrors the server.
pins = {k: c * 64 for k, c in (('runtimeSha256', 'a'), ('supervisorSha256', 'b'), ('serviceSha256', 'c'), ('manifestSha256', 'd'), ('helperSha256', 'e'))}
operation = 'op-' + os.urandom(6).hex()
request = {'version': 1, 'provider': 'boat', 'purpose': 'initial-allocation', 'operationId': operation,
           'workspaceId': 'workspace-one', 'generation': 1, 'storageIncarnationId': 'disk-one',
           'sourceMachineId': 'machine-one', 'imagePins': pins}
ready = {**request, 'status': 'ready', 'requestSha256': hashlib.sha256(ns['canonical'](request)).hexdigest(),
         'unitStartSubmitted': True, 'resident': {'ipcVerified': True, 'unbound': True, 'childAlive': False,
         'recoveryAlive': False, 'residentPid': 42, 'residentStartTicks': 7, 'bootNonceSha256': 'f' * 64}}
check(ns['accepted_receipt'](json.dumps(ready), request) is not None, 'valid receipt accepted')
for change in ({'childAlive': True}, {'unbound': False}, {'residentPid': 1}, {'bootNonceSha256': 'x'}):
    bad = json.loads(json.dumps(ready)); bad['resident'].update(change)
    check(ns['accepted_receipt'](json.dumps(bad), request) is None, f'receipt with {change} refused')
check(ns['accepted_receipt'](json.dumps({**ready, 'workspaceId': 'other'}), request) is None, 'binding mismatch refused')
check(ns['accepted_receipt'](json.dumps({**ready, 'requestSha256': '0' * 64}), request) is None, 'digest mismatch refused')
# Binding fields compare JSON types too: `true` is not the generation 1.
check(ns['accepted_receipt'](json.dumps({**ready, 'generation': True}), request) is None, 'boolean generation refused')
check(ns['accepted_receipt'](json.dumps({**ready, 'generation': 1.0}), request) is None, 'float generation refused')
missing = dict(ready); del missing['imagePins']
check(ns['accepted_receipt'](json.dumps(missing), request) is None, 'missing binding refused')
for resident in ([42], 'resident', None, 7):
    check(ns['accepted_receipt'](json.dumps({**ready, 'resident': resident}), request) is None, f'resident {resident!r} refused')


class Result:
    def __init__(self, returncode, stdout):
        self.returncode, self.stdout = returncode, stdout.encode() if isinstance(stdout, str) else stdout


unsafe = Result(1, json.dumps({'version': 1, 'status': 'rejected', 'code': 'fresh_boot_loader_path_unsafe'}))
check(ns['helper_state'](unsafe) == 'retry', 'loader path refusal retries')
check(ns['helper_state'](Result(0, json.dumps({**request, 'status': 'starting'}))) == 'retry', 'starting retries')
check(ns['helper_state'](Result(1, json.dumps({'status': 'rejected', 'code': 'fresh_boot_file_hash_mismatch'}))) == 'rejected', 'hash mismatch rejected')
check(ns['helper_state'](Result(0, json.dumps(ready))) == 'ready', 'ready')

# Dispatch: the credential leaves /tmp at once into the private directory,
# and a repeated dispatch never starts twice.
class Units:
    calls = []
    submit = 'ok'  # 'ok', 'fail', or 'timeout'
    load_state = b'loaded\n'  # or None when systemctl itself times out
    @staticmethod
    def run(argv, **kwargs):
        Units.calls.append(argv)
        if argv[0] == 'systemd-run':
            if Units.submit == 'timeout':
                raise subprocess.TimeoutExpired(argv, kwargs.get('timeout'))
            return Result(0 if Units.submit == 'ok' else 1, '')
        if argv[0] == 'systemctl':
            if Units.load_state is None:
                raise subprocess.TimeoutExpired(argv, kwargs.get('timeout'))
            return Result(0, Units.load_state)
        return Result(0, '')
ns['subprocess'] = type('S', (), {'run': staticmethod(Units.run), 'TimeoutExpired': subprocess.TimeoutExpired})
ns['private_root'] = lambda root: root.mkdir(mode=0o700, exist_ok=True)
root = tmp / 'run'
staged = Path('/tmp/zuse-rust-boot-' + operation)
args = {'version': 1, 'operationId': operation, 'staged': str(staged), 'starter': '# starter', 'loader': '# loader',
        'helperSha256': 'e' * 64, 'request': base64.b64encode(json.dumps(request).encode()).decode(),
        'launch': 'true', 'deadlineSeconds': 60, 'launchSeconds': 60}
staged.write_bytes(b'private-boot-credential')
check(ns['dispatch'](args, root) == {'status': 'accepted'}, 'first dispatch accepted')
directory = root / operation
check(not staged.exists(), 'staged credential removed immediately')
check((directory / 'boot').read_bytes() == b'private-boot-credential', 'credential kept privately')
check(stat.S_IMODE((directory / 'boot').stat().st_mode) == 0o600, 'credential file is 0600')
check(json.loads((directory / 'status.json').read_bytes())['status'] == 'waiting', 'status waiting')
check(len(Units.calls) == 1 and Units.calls[0][0] == 'systemd-run', 'one waiting unit started')
staged.write_bytes(b'repeated-upload')
check(ns['dispatch'](args, root) == {'status': 'accepted', 'existing': 'waiting'}, 'repeat is accepted')
check(not staged.exists() and len(Units.calls) == 1, 'repeat starts nothing and drops its upload')
try:
    ns['dispatch']({**args, 'staged': '/tmp/elsewhere'}, root)
    check(False, 'foreign staged path must be refused')
except ns['Refused'] as error:
    check(str(error) == 'fresh_start_staged_path_invalid', str(error))


def fresh_dispatch(submit, load_state):
    """A new operation's dispatch with the given systemd-run outcome."""
    op = 'op-' + os.urandom(6).hex()
    a = {**args, 'operationId': op, 'staged': '/tmp/zuse-rust-boot-' + op,
         'request': base64.b64encode(json.dumps({**request, 'operationId': op}).encode()).decode()}
    Path(a['staged']).write_bytes(b'private-boot-credential')
    Units.calls, Units.submit, Units.load_state = [], submit, load_state
    try:
        reply = ns['dispatch'](a, root)
    except ns['Refused'] as error:
        reply = str(error)
    finally:
        Units.submit, Units.load_state = 'ok', b'loaded\n'
        Path(a['staged']).unlink(missing_ok=True)
    d = root / op
    return reply, (d / 'boot').exists(), ns['read_status'](d).get('status')

# A lost systemd-run reply: the service manager decides, never a guess.
reply, kept, state = fresh_dispatch('timeout', b'loaded\n')
check(reply == {'status': 'accepted'} and kept and state == 'waiting', f'timeout, unit loaded: {reply} {kept} {state}')
reply, kept, state = fresh_dispatch('timeout', b'not-found\n')
check(reply == 'fresh_start_unit_failed' and not kept and state == 'failed', f'timeout, no unit: {reply} {kept} {state}')
reply, kept, state = fresh_dispatch('timeout', None)
check(reply == 'fresh_start_unit_uncertain' and kept and state == 'waiting', f'timeout, unknown: {reply} {kept} {state}')
reply, kept, state = fresh_dispatch('fail', b'loaded\n')
check(reply == 'fresh_start_unit_failed' and not kept and state == 'failed', f'submit failed: {reply} {kept} {state}')


# The waiting unit: retry while the loader refuses, accept, launch once.
def start(helper_results, launch_code, safe_after=0.0, deadline=60, helper_seconds=0.0, settled_after=0.0):
    (directory / 'arguments.json').write_bytes(ns['canonical']({**args, 'deadlineSeconds': deadline}))
    (directory / 'boot').write_bytes(b'private-boot-credential')
    calls, timeouts = [], []
    clock = FakeClock()
    def run(argv, timeout=None, **_):
        calls.append(argv[1] if argv[0] == '/bin/sh' else 'helper')
        timeouts.append(timeout)
        if argv[0] == '/bin/sh':
            if launch_code == 'timeout':
                raise subprocess.TimeoutExpired(argv, timeout)
            return Result(launch_code, '')
        if helper_seconds > timeout:
            clock.now += timeout
            raise subprocess.TimeoutExpired(argv, timeout)
        clock.now += helper_seconds
        return helper_results.pop(0) if len(helper_results) > 1 else helper_results[0]
    ns['path_safe'] = lambda path, owner=0: clock.now >= safe_after
    ns['disk_settled'] = lambda: clock.now >= settled_after
    ns['run_start'](directory, watch=clock, clock=clock, run=run, sleep=clock.wait)
    start.timeouts = timeouts
    record = json.loads((directory / 'status.json').read_bytes())
    check(not (directory / 'boot').exists(), 'credential removed after the start')
    return record, calls

record, calls = start([unsafe, unsafe, Result(0, json.dumps(ready))], 0)
check(record['status'] == 'launched' and record['receipt']['status'] == 'ready', f'launched: {record}')
check(calls == ['helper', 'helper', 'helper', '-c'], f'calls {calls}')
record, calls = start([Result(0, json.dumps(ready))], 0, safe_after=0.4)
check(record['status'] == 'launched', 'safe after a wait')
# A permanent helper refusal ends the attempt on the first answer.
record, calls = start([Result(1, json.dumps({'status': 'rejected', 'code': 'fresh_boot_file_hash_mismatch'})), Result(0, json.dumps(ready))], 0)
check(record['status'] == 'rejected' and record['helperCode'] == 'fresh_boot_file_hash_mismatch'
      and calls == ['helper'], f'permanent refusal: {record} {calls}')
# The helper only gets what is left of the deadline, and a late ready never launches.
record, calls = start([Result(0, json.dumps(ready))], 0, safe_after=58, deadline=60)
check(record['status'] == 'launched' and start.timeouts[0] <= 2 and start.timeouts[1] == 60, f'capped: {start.timeouts}')
record, calls = start([Result(0, json.dumps(ready))], 0, safe_after=58, deadline=60, helper_seconds=30)
check(record['status'] == 'timeout' and calls == ['helper'], f'helper outlived the deadline: {record} {calls}')
record, calls = start([Result(0, json.dumps(ready))], 0, safe_after=58, deadline=60, helper_seconds=2)
check(record['status'] == 'timeout' and calls == ['helper'], f'ready at the deadline: {record} {calls}')
# A launch that outlives its bound is a visible failure, never a hang.
record, calls = start([Result(0, json.dumps(ready))], 'timeout')
check(record['status'] == 'failed' and record['code'] == 'fresh_start_launch_timeout', f'launch timeout: {record}')
record, calls = start([Result(0, json.dumps(ready))], 78)
check(record['status'] == 'rejected' and record['code'] == 'fresh_start_runtime_hash_mismatch', f'{record}')
bad = json.loads(json.dumps(ready)); bad['resident']['childAlive'] = True
record, calls = start([Result(0, json.dumps(bad))], 0)
check(record['status'] == 'rejected' and '-c' not in calls, 'invalid receipt never launches')
record, calls = start([Result(0, json.dumps(ready))], 0, safe_after=999, deadline=5)
check(record['status'] == 'timeout' and calls == [], f'deadline: {record} {calls}')
# A refusal right after the helper's own start is retried within the deadline...
racing = Result(1, json.dumps({'version': 1, 'status': 'rejected', 'code': 'fresh_boot_resident_not_owned_idle'}))
record, calls = start([racing, Result(0, json.dumps(ready))], 0)
check(record['status'] == 'launched' and calls == ['helper', 'helper', '-c'], f'retried refusal: {record} {calls}')
# ...and a lasting refusal ends at the deadline with the helper's code, never launching.
record, calls = start([racing], 0, deadline=5)
check(record['status'] == 'rejected' and record['helperCode'] == 'fresh_boot_resident_not_owned_idle'
      and '-c' not in calls and len(calls) >= 5, f'lasting refusal: {record} {len(calls)}')
# Nothing starts while Boat still serves the task directories from its temporary
# restore mount: the supervisor's lock would stay behind on the retired mount.
record, calls = start([Result(0, json.dumps(ready))], 0, settled_after=0.6)
check(record['status'] == 'launched' and calls == ['helper', '-c'], f'started after the swap: {record} {calls}')
record, calls = start([Result(0, json.dumps(ready))], 0, settled_after=999, deadline=5)
check(record['status'] == 'timeout' and record['code'] == 'fresh_start_disk_unsettled_until_deadline'
      and calls == [], f'swap never came: {record} {calls}')
# Between systemd's fork and exec the resident still has the service manager's
# command line: retried like the uid refusal, within the deadline.
forked = Result(1, json.dumps({'version': 1, 'status': 'rejected', 'code': 'fresh_boot_resident_program_changed'}))
record, calls = start([forked, forked, Result(0, json.dumps(ready))], 0)
check(record['status'] == 'launched' and calls == ['helper', 'helper', 'helper', '-c'], f'retried exec race: {record} {calls}')
print('ok')
