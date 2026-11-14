#!/usr/bin/env python3
"""Prized user image guest helper: tree pins, one detached setup run, sanitize and check."""
import hashlib, importlib.util, json, os, tempfile, unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / 'prized-base' / 'user-image-guest.py'
spec = importlib.util.spec_from_file_location('prized_user_image_guest', SCRIPT)
guest = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guest)

IDLE = {'version': 1, 'bound': False, 'childAlive': False}


class FakeHost:
    def __init__(self):
        self.pids = [4242]
        self.killed = []
        self.launched = []
        self.supervised = 0
        self.ping_reply = dict(IDLE)

    def processes(self):
        return list(self.pids)

    def kill(self, pid):
        self.killed.append(pid)
        self.pids.remove(pid)

    def launch(self, argv):
        self.launched.append(argv)

    def running(self, pid):
        return False

    def supervise(self, layout):
        self.supervised += 1
        (layout.home / '.zuse-data' / '.runtime-supervision').mkdir(exist_ok=True)
        return self.ping_reply

    def ping(self, layout):
        return self.ping_reply

    def sync(self):
        pass


class PrizedUserImageGuestTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='zuse-prized-image-')
        self.addCleanup(temp.cleanup)
        self.layout = guest.Layout(temp.name)
        self.host = FakeHost()
        root = self.layout.root
        for name in ('bin', 'lib/__pycache__', 'libexec/node/bin', 'status', 'recovery', 'boot', 'state',
                     'preparation/secrets', 'preparation/status'):
            (root / name).mkdir(parents=True)
        (root / guest.RUNTIME).write_bytes(b'runtime')
        (root / 'lib/zuse-launch').write_text('#!/bin/sh\n')
        (root / 'libexec/node/bin/node').write_bytes(b'node')
        (root / 'preparation/status/manifest.json').write_text('{}')
        (self.layout.home / 'workspace').mkdir()
        (self.layout.home / '.zuse-data' / '.runtime-supervision').mkdir(parents=True)
        self.runtime = hashlib.sha256(b'runtime').hexdigest()

    def run_main(self, *args):
        from io import StringIO
        import contextlib
        out = StringIO()
        with contextlib.redirect_stdout(out):
            code = guest.main(list(args), self.layout, self.host)
        return code, json.loads(out.getvalue())

    def build(self):
        code, base = self.run_main('baseline', self.runtime)
        self.assertEqual(code, 0, base)
        journal = self.layout.journal
        journal.mkdir(mode=0o700)
        (journal / 'setup.sh').write_text('npm i -g bun\n')
        (journal / 'guest.py').write_text('')
        code, started = self.run_main('setup-start', '600')
        self.assertEqual((code, started['state']), (0, 'running'))
        self.assertEqual(len(self.host.launched), 1)
        (journal / 'exit.json').write_text(json.dumps({'version': 1, 'exitCode': 0}))
        return base['treeSha256']

    def test_clean_build_sanitizes_restarts_the_idle_supervisor_and_checks(self):
        tree = self.build()
        home = self.layout.home
        # What a setup script and its tools leave behind.
        (home / '.bash_history').write_text('history')
        (home / '.npmrc').write_text('prefix=/home/prized/.local\n//registry.npmjs.org/:_authToken=abc\n')
        (home / 'workspace' / 'leftover').write_text('x')
        (home / '.zuse-data' / 'cloud-runtime-identity.json').write_text('{}')
        (self.layout.root / 'status' / 'supervisor.log').write_text('log')
        (self.layout.root / 'lib/__pycache__/x.pyc').write_bytes(b'cache')
        (home / '.local/bin').mkdir(parents=True)
        (home / '.local/bin/bun').write_bytes(b'bun')
        code, result = self.run_main('sanitize', self.runtime, tree)
        self.assertEqual(code, 0, result)
        self.assertTrue(result['sanitized'] and result['runtimeStopped'])
        self.assertEqual(self.host.killed, [4242])
        self.assertEqual(self.host.supervised, 1)
        self.assertFalse((home / '.bash_history').exists())
        self.assertEqual((home / '.npmrc').read_text(), 'prefix=/home/prized/.local\n')
        self.assertFalse(any((home / 'workspace').iterdir()))
        self.assertFalse(self.layout.journal.exists())
        self.assertEqual({p.name for p in (home / '.zuse-data').iterdir()}, {'.runtime-supervision'})
        # What setup installed stays.
        self.assertEqual((home / '.local/bin/bun').read_bytes(), b'bun')
        code, checked = self.run_main('check', self.runtime, tree)
        self.assertEqual((code, checked['clean'], checked['supervisorIdle']), (0, True, True))

    def test_a_changed_runtime_or_managed_file_is_refused(self):
        tree = self.build()
        (self.layout.root / 'lib/zuse-launch').write_text('#!/bin/sh\nexit 1\n')
        code, result = self.run_main('sanitize', self.runtime, tree)
        self.assertEqual((code, result['error']), (3, 'user_image_pinned_file_changed'))
        code, result = self.run_main('baseline', 'f' * 64)
        self.assertEqual((code, result['error']), (3, 'user_image_pinned_file_changed'))

    def test_a_bound_supervisor_or_leaked_credential_fails_the_check(self):
        tree = self.build()
        self.run_main('sanitize', self.runtime, tree)
        self.host.ping_reply = {'version': 1, 'bound': True, 'childAlive': True}
        code, result = self.run_main('check', self.runtime, tree)
        self.assertEqual((code, result['error']), (3, 'user_image_runtime_started'))
        self.host.ping_reply = dict(IDLE)
        (self.layout.home / '.git-credentials').write_text('https://x:y@github.com')
        code, result = self.run_main('check', self.runtime, tree)
        self.assertEqual((code, result['error']), (3, 'user_image_not_clean'))

    def test_setup_runs_once_and_status_reports_the_exit(self):
        self.build()
        code, again = self.run_main('setup-start', '600')
        self.assertEqual((code, again['state'], len(self.host.launched)), (0, 'exited', 1))
        (self.layout.journal / 'setup.log').write_text('added 2 packages\n')
        code, status = self.run_main('setup-status')
        self.assertEqual((status['state'], status['exitCode'], status['log']), ('exited', 0, 'added 2 packages\n'))

    def test_state_folders_and_python_caches_are_outside_the_tree(self):
        before = guest.tree_sha256(self.layout)
        (self.layout.root / 'status' / 'rust-runtime.log').write_text('x')
        (self.layout.root / 'lib/__pycache__/y.pyc').write_bytes(b'y')
        self.assertEqual(guest.tree_sha256(self.layout), before)
        os.chmod(self.layout.root / 'lib/zuse-launch', 0o755)
        self.assertNotEqual(guest.tree_sha256(self.layout), before)


if __name__ == '__main__':
    unittest.main()
