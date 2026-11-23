# Exercised by bootstrap.rs: Boat's settle step before the fresh boot helper,
# without root, systemd or a provider.
import json, os, subprocess, sys, tempfile
from pathlib import Path

source = Path(sys.argv[1]).read_text()
ns = {'__name__': 'settle_under_test'}
exec(compile(source, 'boat-machine-settle.py', 'exec'), ns)
tmp = Path(tempfile.mkdtemp())


def check(condition, message):
    if not condition:
        raise SystemExit('FAILED: ' + message)


class Clock:
    def __init__(self):
        self.now = 0.0
    def __call__(self):
        return self.now
    def sleep(self, seconds):
        self.now += seconds


class Units:
    def __init__(self, active=True, pid=41, new_pid=77):
        self.active, self.pid, self.new_pid, self.restarts = active, pid, new_pid, 0
    def state(self):
        if not self.active:
            return {'MainPID': '0', 'ActiveState': 'inactive', 'SubState': 'dead', 'Job': ''}
        return {'MainPID': str(self.pid), 'ActiveState': 'active', 'SubState': 'running', 'Job': ''}
    def restart(self):
        self.restarts += 1
        self.pid = self.new_pid


def run(units, settled_after=0.0, wait=15, stale=False, children=(), published_after=0.0):
    clock = Clock()
    return ns['settle'](wait, units=units, settled=lambda: clock.now >= settled_after,
                        stale=lambda pid: stale, child_pids=lambda pid: list(children),
                        ready=lambda pid: clock.now >= published_after, clock=clock, sleep=clock.sleep), clock


# Still served by the temporary restore mount: not ready, after the bounded wait.
try:
    run(Units(), settled_after=999, wait=3)
    check(False, 'unsettled disk must not be ready')
except ns['NotReady'] as error:
    check(str(error) == 'machine_disk_settling', str(error))
# The swap arrives within the wait: the helper may run now.
result, clock = run(Units(active=False), settled_after=2)
check(result == 'unchanged' and 2 <= clock.now < 3, f'waited for the swap: {result} {clock.now}')
# A healthy running supervisor is adopted by the helper, never restarted.
units = Units()
result, _ = run(units, stale=False)
check(result == 'unchanged' and units.restarts == 0, 'healthy supervisor left alone')
# Its lock was taken before the swap: restart that unit once, wait for the new one.
units = Units()
result, clock = run(units, stale=True, published_after=1)
check(result == 'restarted' and units.restarts == 1 and units.pid == 77 and clock.now >= 1,
      f'stale supervisor restarted: {result} {units.restarts} {clock.now}')
# A slow new resident does not hold the command past its own bound.
units = Units()
result, clock = run(units, stale=True, published_after=999)
check(result == 'restarted' and clock.now <= ns['PUBLISH_SECONDS'] + 1, f'publish wait bounded: {clock.now}')
# A runtime or recovery is still running under it: nothing is interrupted.
units = Units()
try:
    run(units, stale=True, children=(55,))
    check(False, 'stale supervisor with a child must be refused')
except ns['Refused'] as error:
    check(str(error) == 'supervisor_lock_stale_with_child' and units.restarts == 0, str(error))

# Stale-lock detection against this very process's descriptors.
ipc = tmp / 'ipc'
ipc.mkdir()
(ipc / 'supervisor.json').write_text(json.dumps({'version': 1, 'residentPid': os.getpid(), 'bootNonce': 'a' * 32}))
lock = os.open(ipc / 'supervisor.lock', os.O_RDWR | os.O_CREAT, 0o600)
check(not ns['stale_lock'](os.getpid(), ipc), 'holding the visible lock is not stale')
check(not ns['stale_lock'](os.getpid() + 1, ipc), 'an unpublished (still starting) resident is not judged')
# The swap: the visible path is now a different file than the one held.
os.replace(ipc / 'supervisor.lock', tmp / 'retired.lock')
(ipc / 'supervisor.lock').write_bytes(b'')
check(ns['stale_lock'](os.getpid(), ipc), 'a lock held on the retired file is stale')
os.close(lock)
(ipc / 'supervisor.lock').unlink()
check(ns['stale_lock'](os.getpid(), ipc), 'a missing visible lock is stale')
(ipc / 'supervisor.json').write_text('not json')
check(not ns['stale_lock'](os.getpid(), ipc), 'an unreadable record is never proof')

# Real device check: missing paths are ignored, another device is not settled.
check(ns['disk_settled']((str(tmp), str(tmp / 'missing')), root=str(tmp)), 'same device settled')
check(not ns['disk_settled']((str(tmp), '/proc/self'), root=str(tmp)), 'other device not settled')

# The program refuses without root and prints only its fixed JSON code.
output = subprocess.run(['/usr/bin/python3', '-I', '-B', '-c', source, '15'], capture_output=True, text=True,
                        env={'PATH': '/usr/bin:/bin'})
if os.geteuid() != 0:
    check(output.returncode == 1 and json.loads(output.stdout) == {'version': 1, 'status': 'rejected',
          'code': 'settle_root_required'}, f'non-root: {output.stdout!r}')
print('ok')
