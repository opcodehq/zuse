#!/usr/bin/env python3
"""Guest side of a Prized user image build, run as `prized` through provider exec.

Prized has no root and no custom images: the managed base is a paused sandbox
and a user image is another paused sandbox, forked from the base, that ran the
account's setup script. Computers fork the image. The control plane drives one
build through these commands on the builder (a fork of the base) and checks
the result on a verifier (a fork of the paused image):

  baseline RUNTIME_SHA       on the untouched builder: the runtime is the base's
                             and the base is clean; prints the managed tree digest
  setup-start SECONDS        run ~/.zuse-user-image/setup.sh once, detached, under
                             a time limit
  setup-status               not-started | running | exited (exit code and a
                             bounded log tail) | lost
  sanitize RUNTIME_SHA TREE  stop every leftover process, remove credentials,
                             shell history, task data and the build journal,
                             restart the idle supervisor, then re-check the
                             runtime and the managed tree
  check RUNTIME_SHA TREE     read-only: the same rules, on the verifier

There is no privilege boundary on Prized (agents already run as the user who
owns the runtime), so the pins catch a setup script that breaks the managed
files by accident: the runtime must still be the base's exact binary, and the
managed tree (~/.zuse minus its state folders) must be byte-identical to the
fork it started from. Nothing here reads or writes a secret; the builder has
no task, agent or Git credential.
"""
import hashlib, json, os, shutil, signal, stat, subprocess, sys, time
from pathlib import Path

VERSION = 1
USER = 'prized'
HOME = '/home/prized'
ROOT = HOME + '/.zuse'
JOURNAL = HOME + '/.zuse-user-image'
LOG_TAIL = 16384
RUNTIME = 'bin/zuse-cloud-runtime'
# Runtime state under ~/.zuse: not part of the pinned tree, empty in an image.
STATE = ('status', 'recovery', 'boot', 'state', 'preparation/secrets')
# The supervisor keeps only its control socket folder here when idle.
DATA = '.zuse-data'
DATA_ALLOWED = {'.runtime-supervision'}
CREDENTIALS = (
    '.codex/auth.json', '.claude/.credentials.json', '.grok/auth.json',
    '.config/grok/auth.json', '.config/gh', '.zuse-github-cli', '.git-credentials', '.netrc',
    '.zuse/cloud-auth', '.zuse-image/provider-secrets.json', '.aws', '.docker/config.json',
    '.kube', '.pypirc', '.gnupg', '.claude.json',
)
HISTORY = (
    '.bash_history', '.zsh_history', '.python_history', '.node_repl_history', '.psql_history',
    '.mysql_history', '.sqlite_history', '.lesshst', '.viminfo', '.wget-hsts', '.npm/_logs',
    '.local/share/fish/fish_history',
)
TASK_DIRECTORIES = ('workspace',)
TOKEN_LINES = ('_authToken', '_auth=', '_auth =', '_password', 'always-auth')
PATH = HOME + '/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'


class Rejected(Exception):
    pass


def require(condition, code):
    if not condition:
        raise Rejected(code)


class Layout:
    """Filesystem view; tests point it at a temporary home."""

    def __init__(self, home=HOME):
        self.home = Path(home)

    @property
    def root(self):
        return self.home / '.zuse'

    @property
    def journal(self):
        return self.home / '.zuse-user-image'


class Host:
    """Process control. Tests replace this with a fake."""

    def processes(self):
        """PIDs of this user's processes, minus this helper and its callers."""
        keep, pid = set(), os.getpid()
        while pid > 1:
            keep.add(pid)
            try:
                fields = Path('/proc', str(pid), 'stat').read_text().rsplit(')', 1)[1].split()
                pid = int(fields[1])
            except (OSError, ValueError, IndexError):
                break
        uid, found = os.getuid(), []
        for entry in os.listdir('/proc'):
            if not entry.isdigit() or int(entry) in keep:
                continue
            try:
                # A zombie is already dead; its parent (or init) reaps it.
                zombie = Path('/proc', entry, 'stat').read_text().rsplit(')', 1)[1].split()[0] == 'Z'
                if os.stat('/proc/' + entry).st_uid == uid and not zombie:
                    found.append(int(entry))
            except (OSError, IndexError):
                continue
        return found

    def kill(self, pid):
        try:
            os.kill(pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass

    def launch(self, argv):
        subprocess.Popen(argv, start_new_session=True, stdin=subprocess.DEVNULL,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, close_fds=True, cwd=HOME)

    def running(self, pid):
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False
        except PermissionError:
            return True

    def supervise(self, layout):
        """Start the idle supervisor the base pauses with, then ping it."""
        environment = {'HOME': str(layout.home), 'PATH': PATH, 'LANG': 'C.UTF-8'}
        subprocess.run([str(layout.root / 'lib/zuse-supervise')], env=environment, check=True,
                       capture_output=True, timeout=30, cwd=str(layout.home))
        return self.ping(layout)

    def ping(self, layout):
        result = subprocess.run(['python3', str(layout.root / 'lib/runtime-supervisor.py'), '--data-dir',
                                 str(layout.home / DATA), 'ping'], capture_output=True, text=True, timeout=15,
                                env={'HOME': str(layout.home), 'PATH': PATH})
        try:
            return json.loads(result.stdout) if result.returncode == 0 else None
        except ValueError:
            return None

    def sync(self):
        os.sync()


def file_sha256(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode), 'user_image_pinned_file_unsafe')
        digest = hashlib.sha256()
        while part := os.read(fd, 1048576):
            digest.update(part)
        return digest.hexdigest()
    finally:
        os.close(fd)


def tree_sha256(layout):
    """Every managed path under ~/.zuse: type, mode, and content or link target."""
    root = layout.root
    require(root.is_dir() and not root.is_symlink(), 'user_image_managed_tree_missing')
    skipped = {root / name for name in STATE}
    digest = hashlib.sha256()
    for directory, folders, files in os.walk(root):
        here = Path(directory)
        folders[:] = sorted(f for f in folders if f != '__pycache__' and here / f not in skipped)
        for name in sorted(folders) + sorted(files):
            path = here / name
            info = os.lstat(path)
            relative = str(path.relative_to(root))
            if stat.S_ISLNK(info.st_mode):
                body = 'link:' + os.readlink(path)
            elif stat.S_ISDIR(info.st_mode):
                body = 'dir'
            elif stat.S_ISREG(info.st_mode):
                body = 'file:' + file_sha256(path)
            else:
                raise Rejected('user_image_managed_tree_unsafe')
            digest.update(f'{relative}\0{stat.S_IMODE(info.st_mode):o}\0{body}\n'.encode())
    return digest.hexdigest()


def verify_pins(layout, runtime_sha, tree=None):
    require(isinstance(runtime_sha, str) and len(runtime_sha) == 64, 'user_image_pins_invalid')
    runtime = layout.root / RUNTIME
    require(not runtime.is_symlink() and runtime.is_file(), 'user_image_pinned_file_unsafe')
    require(file_sha256(runtime) == runtime_sha, 'user_image_pinned_file_changed')
    current = tree_sha256(layout)
    if tree is not None:
        require(isinstance(tree, str) and len(tree) == 64, 'user_image_pins_invalid')
        require(current == tree, 'user_image_pinned_file_changed')
    return current


def remove(path):
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)
    elif os.path.lexists(path):
        path.unlink()


def empty(path):
    if path.is_dir() and not path.is_symlink():
        for entry in path.iterdir():
            remove(entry)


def scrub_token_lines(path):
    """Keep registry settings in .npmrc-style files; drop any token line."""
    if path.is_symlink() or not path.is_file():
        return
    lines = path.read_text(errors='replace').splitlines(keepends=True)
    kept = [line for line in lines if not any(marker in line for marker in TOKEN_LINES)]
    if kept != lines:
        path.write_text(''.join(kept))


def leftovers(layout):
    """Credential, history, journal or runtime state that must not be in an image."""
    home, found = layout.home, []
    for name in CREDENTIALS + HISTORY:
        if os.path.lexists(home / name):
            found.append('~/' + name)
    ssh = home / '.ssh'
    if os.path.lexists(ssh) and (ssh.is_symlink() or not ssh.is_dir() or any(ssh.iterdir())):
        found.append('~/.ssh')
    if os.path.lexists(layout.journal):
        found.append(JOURNAL)
    for name in ('.npmrc', '.yarnrc', '.yarnrc.yml'):
        path = home / name
        if path.is_file() and not path.is_symlink():
            if any(marker in path.read_text(errors='replace') for marker in TOKEN_LINES):
                found.append('~/' + name)
    data = home / DATA
    if os.path.lexists(data) and (data.is_symlink() or not data.is_dir()
                                  or {e.name for e in data.iterdir()} - DATA_ALLOWED):
        found.append('~/' + DATA)
    for name in ('recovery', 'boot', 'state', 'preparation/secrets'):
        path = layout.root / name
        if os.path.lexists(path) and (path.is_symlink() or not path.is_dir() or any(path.iterdir())):
            found.append('~/.zuse/' + name)
    return found


def task_data(layout):
    found = []
    for name in TASK_DIRECTORIES:
        path = layout.home / name
        if os.path.lexists(path):
            require(not path.is_symlink() and path.is_dir(), 'user_image_task_directory_invalid')
            if any(path.iterdir()):
                found.append('~/' + name)
    return found


def idle(ping):
    return isinstance(ping, dict) and ping.get('version') == 1 and ping.get('bound') is False \
        and ping.get('childAlive') is False


def baseline(layout, runtime_sha):
    tree = verify_pins(layout, runtime_sha)
    require(not leftovers(layout) and not task_data(layout), 'user_image_managed_base_not_clean')
    require(not os.path.lexists(layout.journal), 'user_image_managed_base_not_clean')
    return {'version': VERSION, 'pinsVerified': True, 'treeSha256': tree}


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
    """Runs detached: the script's output goes to the journal's log."""
    journal = layout.journal
    (journal / 'runner.pid').write_text(str(os.getpid()))
    environment = ['HOME=' + str(layout.home), 'USER=' + USER, 'LOGNAME=' + USER, 'SHELL=/bin/bash',
                   'PATH=' + PATH, 'LANG=C.UTF-8', 'CI=1']
    command = ['/usr/bin/timeout', '--kill-after=30', str(seconds), '/usr/bin/env', '-i', *environment,
               '/bin/bash', '--noprofile', '--norc', '-e', str(journal / 'setup.sh')]
    with open(journal / 'setup.log', 'wb') as log:
        code = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                              cwd=str(layout.home)).returncode
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
    # Started, no runner and no exit record: the sandbox restarted or the runner died.
    return {'version': VERSION, 'state': 'lost', 'log': log_tail(layout)}


def sanitize(layout, host, runtime_sha, tree):
    verify_pins(layout, runtime_sha, tree)
    require((layout.journal / 'exit.json').is_file(), 'user_image_setup_not_finished')
    # Everything the setup script left running goes, the idle supervisor too;
    # a fresh one is started below so the image pauses exactly like the base.
    deadline = time.monotonic() + 10
    while processes := host.processes():
        require(time.monotonic() < deadline, 'user_image_processes_running')
        for pid in processes:
            host.kill(pid)
        time.sleep(0.05)
    home = layout.home
    for name in CREDENTIALS + HISTORY:
        if os.path.lexists(home / name):
            remove(home / name)
    ssh = home / '.ssh'
    if ssh.is_symlink() or ssh.is_file():
        ssh.unlink()
    else:
        empty(ssh)
    for name in ('.npmrc', '.yarnrc', '.yarnrc.yml'):
        scrub_token_lines(home / name)
    uid = os.getuid()
    for name in ('/tmp', '/var/tmp'):
        directory = Path(name)
        if directory.is_dir() and not directory.is_symlink() and layout.home == Path(HOME):
            for entry in directory.iterdir():
                try:
                    if os.lstat(entry).st_uid == uid:
                        remove(entry)
                except OSError:
                    continue
    for name in TASK_DIRECTORIES:
        empty(home / name)
    empty(home / DATA)
    for name in STATE:
        empty(layout.root / name)
    remove(layout.journal)
    verify_pins(layout, runtime_sha, tree)
    require(idle(host.supervise(layout)), 'user_image_runtime_started')
    require(not leftovers(layout), 'user_image_sanitize_incomplete')
    require(not task_data(layout), 'user_image_task_data_present')
    host.sync()
    return {'version': VERSION, 'sanitized': True, 'pinsVerified': True, 'runtimeStopped': True,
            'credentialsRemoved': True, 'historyRemoved': True, 'taskDataAbsent': True,
            'userProcessesStopped': True}


def check(layout, host, runtime_sha, tree):
    verify_pins(layout, runtime_sha, tree)
    require(not leftovers(layout), 'user_image_not_clean')
    require(not task_data(layout), 'user_image_task_data_present')
    # A fork keeps the image's idle supervisor; it must answer, unbound.
    require(idle(host.ping(layout)), 'user_image_runtime_started')
    return {'version': VERSION, 'clean': True, 'pinsVerified': True, 'supervisorIdle': True}


def main(argv, layout=None, host=None):
    host = host or Host()
    layout = layout or Layout()
    command = argv[0] if argv else ''
    try:
        if command == 'baseline':
            result = baseline(layout, argv[1])
        elif command == 'setup-start':
            result = setup_start(layout, host, int(argv[1]))
        elif command == 'setup-run':
            return setup_run(layout, int(argv[1]))
        elif command == 'setup-status':
            result = setup_status(layout, host)
        elif command == 'sanitize':
            result = sanitize(layout, host, argv[1], argv[2])
        elif command == 'check':
            result = check(layout, host, argv[1], argv[2])
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
