#!/usr/bin/env python3
"""User image guest helper: pins, one detached setup run, sanitize and check."""
import hashlib, importlib.util, json, os, sys, tempfile, time, unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / 'user-image-guest.py'
spec = importlib.util.spec_from_file_location('user_image_guest', SCRIPT)
guest = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guest)


class FakeHost:
    def __init__(self):
        self.units = {guest.UNIT: {'ActiveState': 'inactive', 'SubState': 'dead', 'MainPID': '0', 'Result': 'success'}}
        self.processes = [4242]
        self.killed = []
        self.launched = []
        self.stopped = []
        self.alive = set()

    def unit_state(self, unit):
        return self.units[unit]

    def stop_unit(self, unit):
        self.stopped.append(unit)

    def user_processes(self, uid):
        return list(self.processes)

    def kill(self, pid):
        self.killed.append(pid)
        self.processes.remove(pid)

    def launch(self, argv):
        self.launched.append(argv)

    def running(self, pid):
        return pid in self.alive

    def sync(self):
        pass


class UserImageGuestTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='zuse-user-image-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.layout = guest.Layout(self.root, uid=os.getuid())
        self.host = FakeHost()
        self.pins = {}
        for name in guest.REQUIRED_PINS:
            path = self.layout.at(guest.PINNED[name][0])
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(name.encode())
            path.chmod(0o644)
            self.pins[name] = hashlib.sha256(name.encode()).hexdigest()
        for name in guest.TASK_DIRECTORIES:
            self.layout.at(name).mkdir(parents=True, exist_ok=True)
        (self.layout.home / '.zuse-data').mkdir(mode=0o700)
        self.layout.journal.mkdir(parents=True)
        (self.layout.journal / 'setup.sh').write_text('echo hi\n')
        (self.layout.journal / 'guest.py').write_text('# runner\n')

    def run_command(self, *argv):
        from io import StringIO
        from contextlib import redirect_stdout
        out = StringIO()
        with redirect_stdout(out):
            code = guest.main(list(argv), self.layout, self.host)
        return code, json.loads(out.getvalue())

    def finish_setup(self, code=0, log='installed pnpm\n'):
        (self.layout.journal / 'setup.log').write_text(log)
        (self.layout.journal / 'exit.json').write_text(json.dumps({'version': 1, 'exitCode': code}))

    def test_pins_reject_any_changed_or_writable_pinned_file(self):
        self.assertEqual(self.run_command('pins', json.dumps(self.pins)), (0, {'version': 1, 'pinsVerified': True}))
        runtime = self.layout.at(guest.PINNED['runtimeSha256'][0])
        runtime.write_bytes(b'replaced by the setup script')
        self.assertEqual(self.run_command('pins', json.dumps(self.pins))[1]['error'], 'user_image_pinned_file_changed')
        runtime.write_bytes(b'runtimeSha256')
        runtime.chmod(0o666)
        self.assertEqual(self.run_command('pins', json.dumps(self.pins))[1]['error'], 'user_image_pinned_file_unsafe')
        runtime.chmod(0o644)
        missing = dict(self.pins)
        del missing['helperSha256']
        self.assertEqual(self.run_command('pins', json.dumps(missing))[1]['error'], 'user_image_pins_invalid')

    def test_setup_launches_once_and_reports_exit_with_bounded_log(self):
        self.assertEqual(self.run_command('setup-status')[1]['state'], 'not-started')
        self.assertEqual(self.run_command('setup-start', '600')[1]['state'], 'running')
        self.assertEqual(len(self.host.launched), 1)
        self.assertEqual(self.host.launched[0][-2:], ['setup-run', '600'])
        # A lost reply never launches the script a second time.
        self.run_command('setup-start', '600')
        self.assertEqual(len(self.host.launched), 1)
        (self.layout.journal / 'runner.pid').write_text('77')
        self.host.alive.add(77)
        self.assertEqual(self.run_command('setup-status')[1]['state'], 'running')
        self.host.alive.clear()
        self.assertEqual(self.run_command('setup-status')[1]['state'], 'lost')
        self.finish_setup(2, 'x' * 40000 + 'last line\n')
        status = self.run_command('setup-status')[1]
        self.assertEqual((status['state'], status['exitCode']), ('exited', 2))
        self.assertLessEqual(len(status['log']), guest.LOG_TAIL)
        self.assertTrue(status['log'].endswith('last line\n'))
        self.assertEqual(self.run_command('setup-start', '5')[1]['error'], 'user_image_setup_timeout_invalid')

    def test_setup_run_executes_script_and_records_exit(self):
        if os.geteuid() == 0 or not Path('/usr/bin/sudo').exists():
            self.skipTest('runs only the journal path without sudo')
        # Exercise the journal contract with a stand-in command runner.
        original = guest.subprocess.run
        calls = []

        def fake_run(command, **kwargs):
            calls.append(command)
            kwargs['stdout'].write(b'hello from setup\n')

            class Result:
                returncode = 0
            return Result()
        guest.subprocess.run = fake_run
        try:
            self.assertEqual(guest.setup_run(self.layout, 120), 0)
        finally:
            guest.subprocess.run = original
        self.assertIn('/usr/bin/timeout', calls[0][0])
        self.assertIn('-u', calls[0])
        self.assertEqual(calls[0][calls[0].index('-u') + 1], 'zuse')
        self.assertEqual(json.loads((self.layout.journal / 'exit.json').read_text())['exitCode'], 0)
        self.assertEqual(guest.log_tail(self.layout), 'hello from setup\n')

    def test_sanitize_removes_credentials_history_and_journal_but_keeps_tools(self):
        self.finish_setup()
        home = self.layout.home
        for name in ('.bash_history', '.codex/auth.json', '.config/gh/hosts.yml', '.git-credentials',
                     '.ssh/id_ed25519', '.npm/_logs/debug.log', '.docker/config.json', '.aws/credentials'):
            path = home / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('secret')
        tool = home / '.bun/bin/bun'
        tool.parent.mkdir(parents=True)
        tool.write_text('tool')
        store = home / '.local/share/pnpm/store/v3/index'
        store.parent.mkdir(parents=True)
        store.write_text('warm')
        (home / '.npmrc').write_text('registry=https://registry.npmjs.org/\n//registry.npmjs.org/:_authToken=abc\n')
        (self.layout.at('/home/zuse/workspace') / 'clone').mkdir()
        (self.layout.at('/tmp')).mkdir(exist_ok=True)
        (self.layout.at('/tmp') / 'download.tar').write_text('x')
        self.assertEqual(self.run_command('check', json.dumps(self.pins))[1]['error'], 'user_image_not_clean')
        code, result = self.run_command('sanitize', json.dumps(self.pins))
        self.assertEqual(code, 0, result)
        self.assertTrue(result['sanitized'] and result['credentialsRemoved'] and result['runtimeStopped'])
        self.assertEqual(self.host.killed, [4242])
        self.assertIn(guest.SETUP_UNIT, self.host.stopped)
        for name in ('.bash_history', '.codex/auth.json', '.config/gh', '.git-credentials', '.npm/_logs',
                     '.docker/config.json', '.aws'):
            self.assertFalse(os.path.lexists(home / name), name)
        self.assertEqual(list((home / '.ssh').iterdir()), [])
        self.assertTrue(tool.exists() and store.exists())
        self.assertEqual((home / '.npmrc').read_text(), 'registry=https://registry.npmjs.org/\n')
        self.assertFalse(self.layout.journal.exists())
        self.assertEqual(list(self.layout.at('/home/zuse/workspace').iterdir()), [])
        self.assertEqual(list(self.layout.at('/tmp').iterdir()), [])
        self.assertEqual(self.run_command('check', json.dumps(self.pins)), (0, {'version': 1, 'clean': True, 'pinsVerified': True}))

    def test_sanitize_fails_closed(self):
        # Setup still running: nothing is captured.
        self.assertEqual(self.run_command('sanitize', json.dumps(self.pins))[1]['error'], 'user_image_setup_not_finished')
        self.finish_setup()
        # The script started the resident supervisor: the image would carry runtime state.
        self.host.units[guest.UNIT] = {'ActiveState': 'active', 'SubState': 'running', 'MainPID': '99'}
        self.assertEqual(self.run_command('sanitize', json.dumps(self.pins))[1]['error'], 'user_image_runtime_started')
        self.host.units[guest.UNIT] = {'ActiveState': 'inactive', 'SubState': 'dead', 'MainPID': '0'}
        (self.layout.home / '.zuse-data' / 'cloud-runtime-identity.json').write_text('{}')
        self.assertEqual(self.run_command('sanitize', json.dumps(self.pins))[1]['error'], 'user_image_runtime_state_present')
        (self.layout.home / '.zuse-data' / 'cloud-runtime-identity.json').unlink()
        # The script replaced the supervisor: pins catch it before and after.
        supervisor = self.layout.at(guest.PINNED['supervisorSha256'][0])
        supervisor.write_text('patched')
        self.assertEqual(self.run_command('sanitize', json.dumps(self.pins))[1]['error'], 'user_image_pinned_file_changed')

    def snapshot(self, branch='main'):
        """A depth-1 bare repository tar, made the way the server makes it."""
        import subprocess, tarfile
        source = self.root / 'source'
        env = {'PATH': '/usr/bin:/bin', 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1',
               'HOME': str(self.root), 'GIT_AUTHOR_NAME': 'a', 'GIT_AUTHOR_EMAIL': 'a@example.com',
               'GIT_COMMITTER_NAME': 'a', 'GIT_COMMITTER_EMAIL': 'a@example.com'}
        run = lambda *a, cwd=None: subprocess.run(a, cwd=cwd, env=env, check=True, capture_output=True, text=True).stdout.strip()
        run('git', 'init', '-q', '-b', branch, str(source))
        (source / 'README.md').write_text('hello\n')
        run('git', 'add', '.', cwd=source)
        run('git', 'commit', '-q', '-m', 'one', cwd=source)
        (source / 'README.md').write_text('hello again\n')
        run('git', 'commit', '-q', '-am', 'two', cwd=source)
        bare = self.root / 'snapshot.git'
        run('git', 'init', '-q', '--bare', str(bare))
        run('git', '-C', str(bare), '-c', 'fetch.unpackLimit=1', 'fetch', '-q', '--depth=1', '--no-tags',
            'file://' + str(source), f'+refs/heads/{branch}:refs/heads/{branch}')
        commit = run('git', '-C', str(bare), 'rev-parse', f'refs/heads/{branch}')
        out = self.root / 'snapshot.tar'
        with tarfile.open(out, 'w') as archive:
            for name in ('HEAD', 'shallow', 'objects', 'refs'):
                archive.add(bare / name, arcname=name)
        return commit, out.read_bytes()

    def stage(self, key, data):
        sha = hashlib.sha256(data).hexdigest()
        chunks = -(-len(data) // guest.REPO_CHUNK)
        result = self.run_command('repos-stage', key, sha, str(len(data)), str(chunks))[1]
        self.assertEqual(result['chunks'], {})
        for n in range(chunks):
            (self.layout.at(guest.REPO_STAGE) / key / f'chunk.{n}').write_bytes(
                data[n * guest.REPO_CHUNK:(n + 1) * guest.REPO_CHUNK])

    def test_repositories_unpack_without_remote_and_survive_sanitize(self):
        commit, data = self.snapshot()
        key = 'a' * 64
        self.assertEqual(self.run_command('repos-unpack', key, 'main', commit)[1]['error'], 'user_image_repository_not_staged')
        self.stage(key, data)
        status = self.run_command('repos-status')[1]
        self.assertEqual(status['staged'][key]['chunks'], {'0': len(data)})
        code, result = self.run_command('repos-unpack', key, 'main', commit)
        self.assertEqual(code, 0, result)
        self.assertEqual((result['baked'], result['commit'], result['branch']), (True, commit, 'main'))
        repo = self.layout.at(guest.REPOS) / f'{key}.git'
        self.assertEqual((repo / 'config').read_text(), guest.REPO_CONFIG)
        self.assertFalse((self.layout.at(guest.REPO_STAGE) / key).exists())
        status = self.run_command('repos-status')[1]
        self.assertEqual(status['baked'][key]['commit'], commit)
        # A wrong expected commit is refused and leaves nothing half-installed.
        other = 'b' * 64
        self.stage(other, data)
        self.assertEqual(self.run_command('repos-unpack', other, 'main', 'c' * 40)[1]['error'], 'user_image_repository_commit_mismatch')
        self.assertFalse(os.path.lexists(self.layout.at(guest.REPOS) / f'{other}.git'))
        self.assertFalse(os.path.lexists(self.layout.at(guest.REPOS) / f'.{other}.unpack'))
        # Attempts are counted across restaging.
        self.assertEqual(self.run_command('repos-skip', other, 'repository_unpack_failed', '0')[1]['attempts'], 1)
        self.stage(other, data)
        self.assertEqual(self.run_command('repos-skip', other, 'repository_unpack_failed', '1')[1]['attempts'], 2)
        self.assertTrue(self.run_command('repos-status')[1]['skipped'][other]['final'])
        # Sanitize drops the staging area and keeps the baked repository.
        self.finish_setup()
        code, result = self.run_command('sanitize', json.dumps(self.pins))
        self.assertEqual(code, 0, result)
        self.assertTrue(repo.is_dir())
        self.assertEqual(self.run_command('check', json.dumps(self.pins))[1]['clean'], True)

    def test_repositories_refuse_remotes_hooks_and_foreign_paths(self):
        import io, tarfile
        commit, data = self.snapshot()
        key = 'd' * 64
        # A snapshot member outside the bare-repository layout is refused.
        buffer = io.BytesIO()
        with tarfile.open(fileobj=io.BytesIO(data)) as source, tarfile.open(fileobj=buffer, mode='w') as archive:
            for member in source.getmembers():
                archive.addfile(member, source.extractfile(member) if member.isfile() else None)
            hook = tarfile.TarInfo('hooks/post-checkout')
            hook.size = 2
            archive.addfile(hook, io.BytesIO(b'#!'))
        self.stage(key, buffer.getvalue())
        self.assertEqual(self.run_command('repos-unpack', key, 'main', commit)[1]['error'], 'user_image_repository_member_invalid')
        self.stage(key, data)
        self.assertEqual(self.run_command('repos-unpack', key, 'main', commit)[0], 0)
        repo = self.layout.at(guest.REPOS) / f'{key}.git'
        guest.remove(self.layout.journal)
        self.assertEqual(self.run_command('check', json.dumps(self.pins))[1]['clean'], True)
        # Something wrote a remote with a token into a baked repository later.
        (repo / 'config').write_text(guest.REPO_CONFIG + '[remote "origin"]\n\turl = https://x:ghp_abc@github.com/a/b.git\n')
        self.assertEqual(self.run_command('check', json.dumps(self.pins))[1]['error'], 'user_image_not_clean')
        (repo / 'config').write_text(guest.REPO_CONFIG)
        (self.layout.at(guest.REPOS) / 'stray').write_text('x')
        self.assertEqual(self.run_command('check', json.dumps(self.pins))[1]['error'], 'user_image_not_clean')

    def test_symlinked_pinned_path_is_rejected(self):
        runtime = self.layout.at(guest.PINNED['runtimeSha256'][0])
        target = self.root / 'elsewhere'
        target.write_bytes(b'runtimeSha256')
        runtime.unlink()
        runtime.symlink_to(target)
        self.assertEqual(self.run_command('pins', json.dumps(self.pins))[1]['error'], 'user_image_path_symlink')


if __name__ == '__main__':
    unittest.main()
