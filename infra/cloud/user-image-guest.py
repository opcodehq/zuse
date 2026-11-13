#!/usr/bin/env python3
"""Guest side of a user image build, run as root through provider exec.

The control plane drives one build through these commands on a fresh builder
VM made from a verified managed base, and checks the result on a verifier VM
made from the captured snapshot:

  pins PINS_JSON       pinned runtime/supervisor/unit/helper/manifest files are
                       byte-identical to the base (same checks as fresh boot)
  setup-start SECONDS  run /opt/zuse/user-image/setup.sh once as `zuse` (who has
                       sudo) under a time limit, detached from the exec session
  setup-status         not-started | running | exited (with exit code and a
                       bounded log tail) | lost
  sanitize PINS_JSON   stop leftover user processes, require the runtime unit
                       never started, remove credentials, shell history, leaky
                       caches and the build journal, empty task directories,
                       then re-verify pins
  check PINS_JSON      read-only: the same cleanliness rules, on the verifier

Nothing here reads or writes a secret, and the setup script never sees one: the
recipe holds no secrets and the builder has no task, agent or Git credential.
The resident supervisor is deliberately never started on the builder, so the
captured image is in the same stopped state as the base itself.
"""
import hashlib, json, os, pwd, shutil, signal, stat, subprocess, sys, time
from pathlib import Path

VERSION = 1
UNIT = 'zuse-runtime-supervisor.service'
SETUP_UNIT = 'zuse-user-image-setup'
USER = 'zuse'
HOME = '/home/zuse'
JOURNAL = '/opt/zuse/user-image'
LOG_TAIL = 16384
PINNED = {
    'runtimeSha256': ('/usr/local/bin/zuse-cloud-runtime', 536870912, False),
    'supervisorSha256': ('/usr/local/lib/zuse/runtime-supervisor.py', 131072, False),
    'serviceSha256': ('/etc/systemd/system/' + UNIT, 16384, False),
    'helperSha256': ('/usr/local/lib/zuse/managed-base-fresh-boot.py', 131072, False),
    'manifestSha256': ('/opt/zuse/preparation/status/manifest.json', 1048576, True),
    'runtimeUpdateHelperSha256': ('/usr/local/libexec/zuse/runtime-update.py', 131072, False),
}
REQUIRED_PINS = {'runtimeSha256', 'supervisorSha256', 'serviceSha256', 'helperSha256', 'manifestSha256'}
# Agent logins and Git/cloud credentials: the same set fresh boot refuses.
CREDENTIALS = (
    '.codex/auth.json', '.claude/.credentials.json', '.grok/auth.json',
    '.config/grok/auth.json', '.config/gh', '.zuse-github-cli', '.git-credentials', '.netrc',
    '.zuse/cloud-auth', '.zuse-image/provider-secrets.json', '.aws', '.docker/config.json',
    '.kube', '.pypirc', '.gnupg',
)
HISTORY = (
    '.bash_history', '.zsh_history', '.python_history', '.node_repl_history', '.psql_history',
    '.mysql_history', '.sqlite_history', '.lesshst', '.viminfo', '.wget-hsts', '.npm/_logs',
    '.local/share/fish/fish_history',
)
TASK_DIRECTORIES = ('/home/zuse/workspace', '/var/lib/zuse/workspace', '/var/lib/zuse/recovery')
TOKEN_LINES = ('_authToken', '_auth=', '_auth =', '_password', 'always-auth')


class Rejected(Exception):
    pass


def require(condition, code):
    if not condition:
        raise Rejected(code)


class Layout:
    """Filesystem view; tests point it at a temporary root."""

    def __init__(self, root='/', uid=None):
        self.root = Path(root)
        self.uid = uid if uid is not None else pwd.getpwnam(USER).pw_uid

    def at(self, path):
        return self.root / path.lstrip('/')

    @property
    def home(self):
        return self.at(HOME)

    @property
    def journal(self):
        return self.at(JOURNAL)


class Host:
    """Process and unit control. Tests replace this with a fake."""

    def unit_state(self, unit):
        result = subprocess.run(
            ['systemctl', 'show', unit, '--property=MainPID,ActiveState,SubState,Result'],
            capture_output=True, text=True, check=True, timeout=5, env={'PATH': '/usr/bin:/bin'})
        return dict(line.split('=', 1) for line in result.stdout.splitlines() if '=' in line)

    def stop_unit(self, unit):
        subprocess.run(['systemctl', 'stop', unit], capture_output=True, timeout=30,
                       env={'PATH': '/usr/bin:/bin'})

    def user_processes(self, uid):
        found = []
        for entry in os.listdir('/proc'):
            if not entry.isdigit():
                continue
            try:
                for line in Path('/proc', entry, 'status').read_text().splitlines():
                    if line.startswith('Uid:') and int(line.split()[1]) == uid:
                        found.append(int(entry))
                        break
            except (FileNotFoundError, ProcessLookupError, PermissionError, ValueError):
                continue
        return found

    def kill(self, pid):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    def launch(self, argv):
        if Path('/usr/bin/systemd-run').exists():
            subprocess.run(['/usr/bin/systemd-run', '--unit=' + SETUP_UNIT, '--collect', '--quiet',
                            '--property=KillMode=control-group', *argv],
                           check=True, capture_output=True, timeout=15, env={'PATH': '/usr/bin:/bin'})
        else:
            subprocess.Popen(argv, start_new_session=True, stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, close_fds=True)

    def running(self, pid):
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False
        except PermissionError:
            return True

    def sync(self):
        os.sync()


def no_symlink(path, stop):
    for current in (path, *path.parents):
        if current == stop:
            break
        if os.path.lexists(current):
            require(not current.is_symlink(), 'user_image_path_symlink')


def protected_sha256(layout, path, limit, user_owned):
    path = layout.at(path)
    no_symlink(path, layout.root)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        before = os.fstat(fd)
        owners = {0, layout.uid} if user_owned else {0}
        if layout.root != Path('/'):
            owners |= {os.getuid()}
        require(stat.S_ISREG(before.st_mode) and before.st_uid in owners and before.st_nlink == 1
                and not before.st_mode & 0o022 and before.st_size <= limit, 'user_image_pinned_file_unsafe')
        digest = hashlib.sha256()
        while part := os.read(fd, 1048576):
            digest.update(part)
        after = os.fstat(fd)
        require((before.st_ino, before.st_size, before.st_mtime_ns) == (after.st_ino, after.st_size, after.st_mtime_ns),
                'user_image_pinned_file_changed')
        return digest.hexdigest()
    finally:
        os.close(fd)


def verify_pins(layout, pins):
    require(isinstance(pins, dict) and REQUIRED_PINS <= set(pins) <= set(PINNED), 'user_image_pins_invalid')
    for name, expected in pins.items():
        require(isinstance(expected, str) and len(expected) == 64, 'user_image_pins_invalid')
        path, limit, user_owned = PINNED[name]
        require(protected_sha256(layout, path, limit, user_owned) == expected, 'user_image_pinned_file_changed')


def remove(path):
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)
    elif os.path.lexists(path):
        path.unlink()


def scrub_token_lines(path):
    """Keep registry settings in .npmrc-style files; drop any token line."""
    if path.is_symlink() or not path.is_file():
        return
    lines = path.read_text(errors='replace').splitlines(keepends=True)
    kept = [line for line in lines if not any(marker in line for marker in TOKEN_LINES)]
    if kept != lines:
        path.write_text(''.join(kept))


def leftovers(layout):
    """Credential, history or journal paths that must not be in an image."""
    found = []
    for name in CREDENTIALS + HISTORY:
        if os.path.lexists(layout.home / name):
            found.append('~/' + name)
    ssh = layout.home / '.ssh'
    if os.path.lexists(ssh) and (ssh.is_symlink() or not ssh.is_dir() or any(ssh.iterdir())):
        found.append('~/.ssh')
    for name in ('/root/.bash_history', '/root/.python_history', '/root/.lesshst', JOURNAL):
        if os.path.lexists(layout.at(name)):
            found.append(name)
    for name in ('.npmrc', '.yarnrc', '.yarnrc.yml'):
        path = layout.home / name
        if path.is_file() and not path.is_symlink():
            if any(marker in path.read_text(errors='replace') for marker in TOKEN_LINES):
                found.append('~/' + name)
    data = layout.home / '.zuse-data'
    if os.path.lexists(data) and (data.is_symlink() or not data.is_dir() or any(data.iterdir())):
        found.append('~/.zuse-data')
    return found


def task_data(layout):
    found = []
    for name in TASK_DIRECTORIES:
        path = layout.at(name)
        if os.path.lexists(path):
            require(not path.is_symlink() and path.is_dir(), 'user_image_task_directory_invalid')
            if any(path.iterdir()):
                found.append(name)
    return found


def runtime_stopped(host):
    state = host.unit_state(UNIT)
    return state.get('ActiveState') == 'inactive' and state.get('SubState') == 'dead' and state.get('MainPID') == '0'


def setup_start(layout, host, seconds):
    require(isinstance(seconds, int) and 60 <= seconds <= 3600, 'user_image_setup_timeout_invalid')
    journal = layout.journal
    require(journal.is_dir() and not journal.is_symlink(), 'user_image_setup_missing')
    script = journal / 'setup.sh'
    require(script.is_file() and not script.is_symlink(), 'user_image_setup_missing')
    started = journal / 'started.json'
    if os.path.lexists(started):
        # One launch per build. A lost reply is answered by setup-status.
        return setup_status(layout, host)
    started.write_text(json.dumps({'version': VERSION, 'startedAt': int(time.time() * 1000)}))
    runner = journal / 'guest.py'
    require(runner.is_file() and not runner.is_symlink(), 'user_image_setup_missing')
    host.launch(['/usr/bin/python3', '-I', '-B', str(runner), 'setup-run', str(seconds)])
    return {'version': VERSION, 'state': 'running'}


def setup_run(layout, seconds):
    """Runs detached as root: the script runs as `zuse`, output to the log."""
    journal = layout.journal
    (journal / 'runner.pid').write_text(str(os.getpid()))
    environment = ['HOME=' + HOME, 'USER=' + USER, 'LOGNAME=' + USER, 'SHELL=/bin/bash',
                   'PATH=/home/zuse/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
                   'LANG=C.UTF-8', 'DEBIAN_FRONTEND=noninteractive', 'CI=1']
    command = ['/usr/bin/timeout', '--kill-after=30', str(seconds), '/usr/bin/sudo', '-n', '-u', USER, '-H',
               '/usr/bin/env', '-i', *environment, '/bin/bash', '--noprofile', '--norc', '-e',
               str(journal / 'setup.sh')]
    with open(journal / 'setup.log', 'wb') as log:
        code = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                              cwd=HOME).returncode
    temporary = journal / '.exit.tmp'
    temporary.write_text(json.dumps({'version': VERSION, 'exitCode': code}))
    os.replace(temporary, journal / 'exit.json')
    return code


def log_tail(layout):
    path = layout.journal / 'setup.log'
    if not path.is_file() or path.is_symlink():
        return ''
    with open(path, 'rb') as log:
        size = log.seek(0, os.SEEK_END)
        log.seek(max(0, size - LOG_TAIL))
        return log.read().decode('utf-8', errors='replace')


def setup_status(layout, host):
    journal = layout.journal
    finished = journal / 'exit.json'
    if finished.is_file() and not finished.is_symlink():
        code = json.loads(finished.read_text())['exitCode']
        return {'version': VERSION, 'state': 'exited', 'exitCode': code, 'log': log_tail(layout)}
    if not os.path.lexists(journal / 'started.json'):
        return {'version': VERSION, 'state': 'not-started'}
    pid_file = journal / 'runner.pid'
    if pid_file.is_file():
        try:
            if host.running(int(pid_file.read_text().strip())):
                return {'version': VERSION, 'state': 'running'}
        except ValueError:
            pass
    else:
        started = json.loads((journal / 'started.json').read_text()).get('startedAt', 0)
        if int(time.time() * 1000) - started < 60_000:
            return {'version': VERSION, 'state': 'running'}
    # Started, no runner and no exit record: the VM restarted or the runner died.
    return {'version': VERSION, 'state': 'lost', 'log': log_tail(layout)}


def sanitize(layout, host, pins):
    verify_pins(layout, pins)
    require((layout.journal / 'exit.json').is_file(), 'user_image_setup_not_finished')
    host.stop_unit(SETUP_UNIT)
    deadline = time.monotonic() + 10
    while processes := host.user_processes(layout.uid):
        require(time.monotonic() < deadline, 'user_image_processes_running')
        for pid in processes:
            host.kill(pid)
        time.sleep(0.05)
    # Starting the resident would leave runtime identity in the image; a base
    # image is captured stopped, and so is a user image.
    require(runtime_stopped(host), 'user_image_runtime_started')
    data = layout.home / '.zuse-data'
    require(not os.path.lexists(data) or (data.is_dir() and not data.is_symlink() and not any(data.iterdir())),
            'user_image_runtime_state_present')
    for name in CREDENTIALS + HISTORY:
        path = layout.home / name
        if os.path.lexists(path):
            remove(path)
    ssh = layout.home / '.ssh'
    if ssh.is_symlink() or ssh.is_file():
        ssh.unlink()
    elif ssh.is_dir():
        for entry in ssh.iterdir():
            remove(entry)
    for name in ('.npmrc', '.yarnrc', '.yarnrc.yml'):
        scrub_token_lines(layout.home / name)
    for name in ('/root/.bash_history', '/root/.python_history', '/root/.lesshst'):
        if os.path.lexists(layout.at(name)):
            remove(layout.at(name))
    for name in ('/tmp', '/var/tmp'):
        directory = layout.at(name)
        if directory.is_dir() and not directory.is_symlink():
            for entry in directory.iterdir():
                if entry.name.startswith(('.X11-unix', '.ICE-unix', 'systemd-private-')):
                    continue
                remove(entry)
    for name in TASK_DIRECTORIES:
        directory = layout.at(name)
        if directory.is_dir() and not directory.is_symlink():
            for entry in directory.iterdir():
                remove(entry)
    remove(layout.journal)
    verify_pins(layout, pins)
    require(not leftovers(layout), 'user_image_sanitize_incomplete')
    require(not task_data(layout), 'user_image_task_data_present')
    host.sync()
    return {'version': VERSION, 'sanitized': True, 'pinsVerified': True, 'runtimeStopped': True,
            'credentialsRemoved': True, 'historyRemoved': True, 'taskDataAbsent': True,
            'userProcessesStopped': True}


def check(layout, pins):
    verify_pins(layout, pins)
    found = leftovers(layout)
    require(not found, 'user_image_not_clean')
    require(not task_data(layout), 'user_image_task_data_present')
    return {'version': VERSION, 'clean': True, 'pinsVerified': True}


def main(argv, layout=None, host=None):
    host = host or Host()
    layout = layout or Layout()
    command = argv[0] if argv else ''
    try:
        if command == 'pins':
            verify_pins(layout, json.loads(argv[1]))
            result = {'version': VERSION, 'pinsVerified': True}
        elif command == 'setup-start':
            result = setup_start(layout, host, int(argv[1]))
        elif command == 'setup-run':
            return setup_run(layout, int(argv[1]))
        elif command == 'setup-status':
            result = setup_status(layout, host)
        elif command == 'sanitize':
            result = sanitize(layout, host, json.loads(argv[1]))
        elif command == 'check':
            result = check(layout, json.loads(argv[1]))
        else:
            raise Rejected('user_image_command_invalid')
    except Rejected as error:
        print(json.dumps({'version': VERSION, 'error': str(error)}, separators=(',', ':')))
        return 3
    except (OSError, ValueError, KeyError, IndexError, subprocess.SubprocessError):
        print(json.dumps({'version': VERSION, 'error': 'user_image_guest_failed'}, separators=(',', ':')))
        return 3
    print(json.dumps(result, separators=(',', ':')))
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
