#!/usr/bin/env python3
"""Private line-protocol bridge for the Rust PostgreSQL repair integration test.

Owns a real resident supervisor, disposable native SQLite and an actual detached
orphan. No provider, model, production URL or user credential is accessed.
"""
import importlib.util
import json
import os
from pathlib import Path
import signal
import sqlite3
import sys
import time

spec = importlib.util.spec_from_file_location('supervisor_tests', Path(__file__).with_name('runtime-supervisor.py'))
suite = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite)
repair_spec = importlib.util.spec_from_file_location('bridge_retirement', Path(__file__).resolve().parents[1] / 'runtime-repair.py')
retirement = importlib.util.module_from_spec(repair_spec)
repair_spec.loader.exec_module(retirement)


class Bridge:
    def __init__(self):
        if not retirement.enable_subreaper():
            raise RuntimeError('fixture_subreaper_unavailable')
        self.fixture = suite.RuntimeSupervisorTest()
        try:
            self.fixture.setUp()
        except BaseException:
            try:
                if getattr(self.fixture, 'server', None):
                    self.fixture.stop()
                retirement.retire_owned(os.getpid())
            finally:
                self.fixture.doCleanups()
            raise
        self.original = None

    def prepare(self):
        f = self.fixture
        profile = f.data / 'orgs' / 'account' / 'user'
        profile.mkdir(parents=True)
        journals = profile / 'journals'
        journals.mkdir()
        self.database = profile / 'zuse.sqlite'
        marker = dict(version=1, workspaceId='ws-bob', accountId='user-bob',
                      storage=dict(format='zuse-rust-engine-v1', storageIncarnationId='disk'),
                      database=str(self.database.relative_to(f.data)), ready=True)
        path = f.data / 'rust-runtime-storage.json'
        path.write_text(json.dumps(marker))
        path.chmod(0o600)
        (f.data / '.workspace-runtime.lock').touch(mode=0o600)
        with sqlite3.connect(self.database) as db:
            db.executescript("""
CREATE TABLE zuse_cloud_storage_identity(singleton INTEGER,workspace_id TEXT,account_id TEXT,storage_incarnation_id TEXT);
INSERT INTO zuse_cloud_storage_identity VALUES(1,'ws-bob','user-bob','disk');
CREATE TABLE processed_commands(command_id TEXT PRIMARY KEY,processed_at INTEGER);
CREATE TABLE zuse_cloud_command_receipts(command_id TEXT PRIMARY KEY,workspace_id TEXT,storage_incarnation_id TEXT,fingerprint TEXT,acknowledgment TEXT,lease_json TEXT,delivered INTEGER DEFAULT 0);
CREATE TABLE effects(count INTEGER);INSERT INTO effects VALUES(0);
""")
        (journals / 'old-chat.jsonl').write_text(json.dumps(dict(seq=1, event=dict(type='toolStart'))) + '\n')
        self.heartbeat = f.root / 'orphan-effects'
        orphan = "from pathlib import Path;import sys,time;p=Path(sys.argv[1]);p.write_text('start\\n');exec(\"while True:\\n with p.open('a') as f:f.write('effect\\\\n');f.flush()\\n time.sleep(.02)\")"
        lease = dict(leaseToken='retired-lease', storageIncarnationId='disk',
                     command=dict(commandId='attempted', workspaceId='ws-bob', fingerprint='fingerprint'))
        effect = """if os.environ['ZUSE_RUNTIME_OPERATION_ID']=='op-first':
 import sqlite3,subprocess,sys
 with sqlite3.connect(DATABASE) as db:
  db.execute("INSERT INTO processed_commands VALUES('attempted',1)")
  db.execute('UPDATE effects SET count=count+1')
  db.execute("INSERT INTO zuse_cloud_command_receipts(command_id,workspace_id,storage_incarnation_id,fingerprint,acknowledgment,lease_json) VALUES('attempted','ws-bob','disk','fingerprint',NULL,?)",(LEASE,))
 subprocess.Popen([sys.executable,'-c',ORPHAN,HEARTBEAT],start_new_session=True)
while True:time.sleep(.1)
""".replace('DATABASE', repr(str(self.database))).replace('LEASE', repr(json.dumps(lease))).replace('ORPHAN', repr(orphan)).replace('HEARTBEAT', repr(str(self.heartbeat)))
        f.launcher.write_text(f.launcher.read_text().replace('while True:time.sleep(.1)', effect))
        request = f.request()
        request.update(workspaceId='ws-bob', generation=1, apiUrl='https://api.example.test')
        suite.helper.rpc(f.data, request)
        self.original = f.child()['pid']
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline and not self.heartbeat.exists():
            time.sleep(.01)
        if not self.heartbeat.exists():
            raise RuntimeError('fixture_child_not_ready')
        os.kill(self.original, signal.SIGKILL)
        before = self.heartbeat.stat().st_size
        time.sleep(.1)
        if self.heartbeat.stat().st_size <= before:
            raise RuntimeError('fixture_orphan_not_live')
        return dict(prepared=True)

    def verify(self):
        ping = suite.helper.rpc(self.fixture.data, dict(version=1, action='ping'))
        with sqlite3.connect(self.database) as db:
            effects = db.execute('SELECT count FROM effects').fetchone()[0]
            acknowledgment = json.loads(db.execute("SELECT acknowledgment FROM zuse_cloud_command_receipts WHERE command_id='attempted'").fetchone()[0])
        before = self.heartbeat.stat().st_size
        time.sleep(.1)
        return dict(effects=effects, orphanRetired=self.heartbeat.stat().st_size == before,
                    freshPid=ping['childPid'], originalPid=self.original,
                    childAlive=ping['childAlive'], localState=acknowledgment['state'],
                    dirtyRetained=(self.fixture.checkout / 'dirty').read_text() == 'retained uncommitted work')

    def close(self):
        # A failed test may quit before repair-start retires the old orphan.
        # Adopt it from the stopped supervisor and retire it before deleting data.
        try:
            self.fixture.stop()
            retirement.retire_owned(os.getpid())
        finally:
            self.fixture.doCleanups()


def interrupted(_signal, _frame):
    raise SystemExit(0)


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, interrupted)
    bridge = Bridge()
    try:
        for line in sys.stdin:
            try:
                request = json.loads(line)
                if request.get('control') == 'prepare':
                    value = bridge.prepare()
                elif request.get('control') == 'verify':
                    value = bridge.verify()
                elif request.get('control') == 'quit':
                    print(json.dumps(dict(ok=True, value={})), flush=True)
                    break
                else:
                    value = suite.helper.rpc(bridge.fixture.data, request)
                print(json.dumps(dict(ok=True, value=value)), flush=True)
            except Exception:
                print(json.dumps(dict(ok=False, code='isolated_supervisor_fixture_failed')), flush=True)
    finally:
        bridge.close()
