#!/usr/bin/env python3
"""Private boot-owner integration with actual subprocesses, never model inference."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import signal
import socket
import stat
import threading
import subprocess
import sys
import tempfile
import time
import unittest

SCRIPT=Path(__file__).resolve().parents[1]/'runtime-supervisor.py'
spec=importlib.util.spec_from_file_location('runtime_supervisor',SCRIPT)
helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)
@unittest.skipUnless(sys.platform=='linux','Linux guest peer credentials')
class RuntimeSupervisorTest(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='zuse-supervisor-');self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name);self.data=self.root/'data';self.data.mkdir(mode=0o700)
        self.status=self.root/'status';self.status.mkdir(mode=0o700);self.checkout=self.root/'repo';self.checkout.mkdir()
        (self.checkout/'dirty').write_text('retained uncommitted work')
        self.launcher=self.root/'launch';self.launcher.write_text('''#!/usr/bin/env python3
import json,os,signal,time
from pathlib import Path
assert not any(k in os.environ for k in ('BOXD_API_KEY','BOAT_API_KEY','ANTHROPIC_API_KEY','OPENAI_API_KEY','CLOUD_DATA_ENCRYPTION_KEY'))
assert __import__('sys').argv[1:]==['--foreground']
p=Path(os.environ['ZUSE_USER_DATA']);(p/'child.json').write_text(json.dumps({'pid':os.getpid(),'retained':os.environ['ZUSE_RUNTIME_RETAINED']}))
(p/'cloud-runtime-identity.json').write_text('{}')
if os.environ.get('ZUSE_RUNTIME_BOOT_TOKEN_FILE'):Path(os.environ['ZUSE_RUNTIME_BOOT_TOKEN_FILE']).unlink()
while True:time.sleep(.1)
''');self.launcher.chmod(0o700)
        self.binary=self.root/'runtime';self.binary.write_text('#!/bin/sh\nexit 0\n');self.binary.chmod(0o700)
        self.recovery=self.root/'recovery';self.recovery.mkdir(mode=0o700)
        self.recovery_launcher=self.root/'recovery-launch';self.recovery_launcher.write_text("#!/usr/bin/env python3\nimport os,sys,time,json\nfrom pathlib import Path\np=Path(os.environ['ZUSE_RECOVERY_STATUS_ROOT'])/sys.argv[1]\n(p/'child.json').write_text(json.dumps({'pid':os.getpid()}))\nwhile True:time.sleep(.1)\n");self.recovery_launcher.chmod(0o700)
        self.server=None;self.start()
        self.addCleanup(self.stop)
    def start(self):
        env=dict(os.environ,BOXD_API_KEY='must-not-reach-child',ANTHROPIC_API_KEY='must-not-reach-child')
        self.server=subprocess.Popen(['python3',str(SCRIPT),'--data-dir',str(self.data),'--workspace-root',str(self.checkout),'--status-dir',str(self.status),'--launcher',str(self.launcher),'--binary',str(self.binary),'--recovery-root',str(self.recovery),'--recovery-launcher',str(self.recovery_launcher),'serve'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,start_new_session=True)
        end=time.monotonic()+4
        while time.monotonic()<end:
            try:helper.rpc(self.data,{'version':1,'action':'ping'});return
            except (OSError,helper.Rejected):time.sleep(.01)
        self.fail('supervisor failed to start')
    def stop(self):
        if self.server and self.server.poll() is None:self.server.terminate();self.server.wait(timeout=8)
        if self.server and self.server.stderr:self.server.stderr.close()
    def request(self,operation='op-first',retained=False):
        r=dict(version=1,action='launch',operationId=operation,workspaceId='task',generation=2,storageIncarnationId='disk',apiUrl='http://127.0.0.1',retained=retained)
        if not retained:
            token=self.status/('rust-boot-'+operation);token.write_text('private-fixture-token');token.chmod(0o600);r['bootTokenFile']=str(token)
        return r
    def child(self):
        end=time.monotonic()+4
        while time.monotonic()<end:
            try:return json.loads((self.data/'child.json').read_text())
            except (OSError,ValueError):time.sleep(.01)
        self.fail('child not started')
    def test_live_replacement_listener_refuses_start_without_unlink(self):
        self.stop();path=self.data/'.runtime-supervision'/'control.sock'
        with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as winner:
            winner.bind(str(path));path.chmod(0o600);winner.listen(2);inode=path.stat().st_ino
            result=subprocess.run(self.server.args,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,timeout=3)
            self.assertNotEqual(result.returncode,0);self.assertEqual(path.stat().st_ino,inode)
            with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:client.connect(str(path))
    def test_exit_does_not_unlink_replacement_listener_or_directory(self):
        for replace_directory in (False,True):
            if replace_directory:self.start()
            ipc=self.data/'.runtime-supervision';path=ipc/'control.sock'
            if replace_directory:ipc.rename(self.data/'old-ipc');ipc.mkdir(mode=0o700)
            else:path.unlink()
            with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as winner:
                winner.bind(str(path));path.chmod(0o600);winner.listen(2);inode=path.stat().st_ino
                self.stop();self.assertEqual(path.stat().st_ino,inode)
                with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:client.connect(str(path))
            path.unlink()
    def test_separate_exec_delivery_survives_and_duplicate_does_not_spawn_or_replay(self):
        request=self.request();initial=helper.rpc(self.data,request);first=self.child()
        self.assertEqual(first['retained'],'false')
        # The original token has been consumed; an exact lost-ACK redelivery
        # must still be acknowledged without starting another runtime.
        duplicate=helper.rpc(self.data,request);self.assertEqual(initial['childPid'],duplicate['childPid'])
        code='import json,sys;sys.path.insert(0,sys.argv[1]);import runpy;h=runpy.run_path(sys.argv[2]);print(json.dumps(h["rpc"](__import__("pathlib").Path(sys.argv[3]),json.loads(sys.argv[4]))))'
        result=subprocess.run(['python3','-c',code,str(SCRIPT.parent),str(SCRIPT),str(self.data),json.dumps({'version':1,'action':'ping'})],capture_output=True,text=True,check=True)
        ping=json.loads(result.stdout);self.assertTrue(ping['childAlive']);self.assertEqual(ping['childPid'],first['pid']);self.assertEqual(ping['binding'],dict(version=1,workspaceId='task',generation=2,storageIncarnationId='disk',apiUrl='http://127.0.0.1'));self.assertEqual(set(ping['binding']),{'version','workspaceId','generation','storageIncarnationId','apiUrl'})
        self.assertEqual((self.checkout/'dirty').read_text(),'retained uncommitted work')
        self.assertNotIn('private-fixture-token',(self.data/'cloud-runtime-supervision.json').read_text())
    def test_api_exec_launcher_performs_bounded_handoff_to_existing_boot_owner(self):
        request=self.request('op-via-launcher')
        launcher=Path(__file__).resolve().parents[1]/'launch-runtime.sh'
        env=dict(os.environ,ZUSE_USER_DATA=str(self.data),ZUSE_WORKSPACE_RUNTIME_STATUS_DIR=str(self.status),
            ZUSE_CLOUD_WORKSPACE_ROOT=str(self.checkout),ZUSE_RUNTIME_SUPERVISOR=str(SCRIPT),
            ZUSE_RUNTIME_BINARY=str(self.binary),ZUSE_CLOUD_WORKSPACE_ID='task',ZUSE_API_URL='http://127.0.0.1',
            ZUSE_RUNTIME_GENERATION='2',ZUSE_STORAGE_INCARNATION_ID='disk',ZUSE_RUNTIME_OPERATION_ID='op-via-launcher',
            ZUSE_RUNTIME_BOOT_TOKEN_FILE=request['bootTokenFile'])
        process=subprocess.Popen(['bash',str(launcher)],env=env,start_new_session=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        _,error=process.communicate(timeout=4);self.assertEqual(process.returncode,0,error.decode())
        first=self.child()
        try:os.killpg(process.pid,signal.SIGKILL)
        except ProcessLookupError:pass
        self.assertTrue(helper.rpc(self.data,{'version':1,'action':'ping'})['childAlive'])
        self.assertEqual(self.child()['pid'],first['pid'])

    def test_fences_workspace_disk_generation_and_operation_reuse(self):
        request=self.request();helper.rpc(self.data,request);self.child()
        for key,value in [('workspaceId','other'),('generation',3),('storageIncarnationId','other-disk'),('apiUrl','http://localhost')]:
            changed=dict(request);changed[key]=value
            with self.assertRaises(helper.Rejected):helper.rpc(self.data,changed)
        changed=dict(request);changed['retained']=True;changed.pop('bootTokenFile')
        with self.assertRaises(helper.Rejected):helper.rpc(self.data,changed)
    def test_retained_restart_preserves_identity_checkout_and_no_prompt_field(self):
        helper.rpc(self.data,self.request());first=self.child();self.stop()
        self.start();resumed=helper.rpc(self.data,self.request('op-resume',True))
        end=time.monotonic()+4
        while time.monotonic()<end:
            second=self.child()
            if second['pid']!=first['pid']:break
            time.sleep(.01)
        self.assertNotEqual(resumed['childPid'],first['pid']);self.assertEqual(second['retained'],'true')
        self.assertEqual((self.checkout/'dirty').read_text(),'retained uncommitted work')
        invalid=self.request('op-bad',True);invalid['prompt']='must never be replayed'
        with self.assertRaises(helper.Rejected):helper.rpc(self.data,invalid)
    def test_memory_resume_wakes_same_child_and_is_idempotent(self):
        helper.rpc(self.data,self.request());first=self.child()
        request=self.request('op-memory-wake',True)
        one=helper.rpc(self.data,request);two=helper.rpc(self.data,request)
        self.assertEqual(one['childPid'],first['pid']);self.assertEqual(two['childPid'],first['pid'])
    def test_same_accepted_resume_renotifies_only_a_still_parked_owned_socket(self):
        helper.rpc(self.data,self.request());first=self.child()
        counter=self.root/'notify-count'
        self.binary.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\np=Path('+repr(str(counter))+');p.write_text(p.read_text()+"1" if p.exists() else "1")\n')
        request=self.request('op-memory-retry',True)
        helper.rpc(self.data,request)
        self.assertEqual(counter.read_text(),'1')
        memory=self.data/'.runtime-memory';memory.mkdir(mode=0o700)
        parked=socket.socket(socket.AF_UNIX);parked.bind(str(memory/'wake.sock'));os.chmod(memory/'wake.sock',0o600)
        self.addCleanup(parked.close)
        second=helper.rpc(self.data,request)
        self.assertEqual(second['childPid'],first['pid'])
        self.assertEqual(counter.read_text(),'11','a consumed wake with transient authority failure must be deliverable under the same operation')
        memory.chmod(0o755)
        with self.assertRaises(helper.Rejected):helper.rpc(self.data,request)
        self.assertEqual(counter.read_text(),'11')
        memory.chmod(0o700)
        parked.close();(memory/'wake.sock').unlink()
        helper.rpc(self.data,request)
        self.assertEqual(counter.read_text(),'11','an active unparked child must remain idempotent')
        malformed=self.request('op-memory-retry',True);malformed['generation']=3
        with self.assertRaises(helper.Rejected):helper.rpc(self.data,malformed)
    def test_recovery_private_handoff_is_idempotent_cancellable_and_fenced(self):
        operation='restore-only';directory=self.recovery/operation;directory.mkdir(mode=0o700)
        config=dict(operationId=operation,workspaceId='task',mode='restore',home=str(self.data.parent),repositoriesPath=str(self.checkout),supervisorBinding=dict(workspaceId='task',generation=2,storageIncarnationId='disk'),token='never-in-ipc',key='never-in-receipts')
        path=directory/'config.json';path.write_text(json.dumps(config));path.chmod(0o600)
        request=dict(version=1,action='recover',operationId=operation)
        first=helper.rpc(self.data,request);second=helper.rpc(self.data,request)
        self.assertEqual(first['childPid'],second['childPid'])
        with self.assertRaises(helper.Rejected):helper.rpc(self.data,self.request())
        helper.rpc(self.data,dict(version=1,action='cancel-recovery',operationId=operation))
        deadline=time.monotonic()+4
        while time.monotonic()<deadline:
            if not helper.rpc(self.data,{'version':1,'action':'ping'})['recoveryAlive']:break
            time.sleep(.02)
        self.assertFalse(helper.rpc(self.data,{'version':1,'action':'ping'})['recoveryAlive'])
        invalid=self.request();invalid['generation']=3
        with self.assertRaises(helper.Rejected):helper.rpc(self.data,invalid)
        self.assertNotIn('never-in-',(self.data/'cloud-runtime-supervisor-recovery.json').read_text())

    def test_repeated_recovery_cancel_has_one_bounded_retirement_deadline(self):
        self.recovery_launcher.write_text(self.recovery_launcher.read_text().replace("import os,sys,time,json", "import os,sys,time,json,signal;signal.signal(signal.SIGTERM,signal.SIG_IGN)"))
        operation='wedged-restore';directory=self.recovery/operation;directory.mkdir(mode=0o700)
        config=dict(operationId=operation,workspaceId='task',mode='restore',home=str(self.data.parent),repositoriesPath=str(self.checkout),supervisorBinding=dict(workspaceId='task',generation=2,storageIncarnationId='disk'))
        path=directory/'config.json';path.write_text(json.dumps(config));path.chmod(0o600)
        helper.rpc(self.data,dict(version=1,action='recover',operationId=operation))
        deadline=time.monotonic()+2
        while time.monotonic()<deadline and not (directory/'child.json').exists():time.sleep(.01)
        self.assertTrue((directory/'child.json').exists())
        started=time.monotonic()
        while time.monotonic()-started<7:
            helper.rpc(self.data,dict(version=1,action='cancel-recovery',operationId=operation))
            if not helper.rpc(self.data,dict(version=1,action='ping'))['recoveryAlive']:break
            time.sleep(.1)
        self.assertFalse(helper.rpc(self.data,dict(version=1,action='ping'))['recoveryAlive'])
        self.assertLess(time.monotonic()-started,7)

    def test_recreation_private_rpc_preserves_recipe_and_is_idempotent(self):
        operation='restore-recreate';directory=self.recovery/operation;directory.mkdir(mode=0o700)
        config=dict(operationId=operation,workspaceId='task',mode='restore',home=str(self.data.parent),repositoriesPath=str(self.checkout),supervisorBinding=dict(workspaceId='task',generation=2,storageIncarnationId='disk'))
        path=directory/'config.json';path.write_text(json.dumps(config));path.chmod(0o600)
        helper.rpc(self.data,dict(version=1,action='recover',operationId=operation))
        helper.rpc(self.data,dict(version=1,action='cancel-recovery',operationId=operation))
        deadline=time.monotonic()+3
        while time.monotonic()<deadline and helper.rpc(self.data,dict(version=1,action='ping'))['recoveryAlive']:time.sleep(.02)
        restored=dict(binding=dict(operationId=operation,workspaceId='task',rustStorage=dict(accountId='account',storageIncarnationId='disk')),archiveSha256='a'*64)
        for name,value in [('rust-recovery-restored.json',restored),('environment-ready.json',dict(workspace='task',incarnation='disk',checkout=str(self.checkout)))]:
            path=self.data/name;path.write_text(json.dumps(value));path.chmod(0o600)
        request=dict(version=1,action='prepare-recreation',operationId='recreate',archiveOperationId=operation,workspaceId='task',ownerId='account',generation=2,storageIncarnationId='disk',archiveSha256='a'*64)
        first=helper.rpc(self.data,request);self.assertEqual(first['status'],'prepared')
        self.assertFalse((self.data/'environment-ready.json').exists())
        self.assertTrue((self.data/'.runtime-recreation/environment.json').exists())
        self.assertEqual(helper.rpc(self.data,request),first)
        self.assertEqual((self.checkout/'dirty').read_text(),'retained uncommitted work')
        self.assertEqual(helper.rpc(self.data,dict(version=1,action='ping'))['recreationVersion'],1)

    def test_dead_child_repair_retires_setsid_orphan_and_does_not_restart_twice(self):
        profile=self.data/'orgs'/'account'/'user';profile.mkdir(parents=True);(profile/'journals').mkdir()
        database=profile/'zuse.sqlite'
        marker=dict(version=1,workspaceId='task',accountId='account',storage=dict(format='zuse-rust-engine-v1',storageIncarnationId='disk'),database=str(database.relative_to(self.data)),ready=True)
        path=self.data/'rust-runtime-storage.json';path.write_text(json.dumps(marker));path.chmod(0o600)
        (self.data/'.workspace-runtime.lock').touch(mode=0o600)
        db=sqlite3.connect(database);db.executescript("CREATE TABLE zuse_cloud_storage_identity(singleton INTEGER,workspace_id TEXT,account_id TEXT,storage_incarnation_id TEXT);INSERT INTO zuse_cloud_storage_identity VALUES(1,'task','account','disk');CREATE TABLE processed_commands(command_id TEXT PRIMARY KEY,processed_at INTEGER);INSERT INTO processed_commands VALUES('committed',1);CREATE TABLE zuse_cloud_command_receipts(command_id TEXT PRIMARY KEY,workspace_id TEXT,storage_incarnation_id TEXT,fingerprint TEXT,acknowledgment TEXT,lease_json TEXT,delivered INTEGER DEFAULT 0);CREATE TRIGGER seed_lease AFTER INSERT ON zuse_cloud_command_receipts BEGIN UPDATE zuse_cloud_command_receipts SET lease_json=json_object('leaseToken','fixture-lease','storageIncarnationId',NEW.storage_incarnation_id,'command',json_object('commandId',NEW.command_id,'workspaceId',NEW.workspace_id,'fingerprint',NEW.fingerprint)) WHERE command_id=NEW.command_id; END;INSERT INTO zuse_cloud_command_receipts(command_id,workspace_id,storage_incarnation_id,fingerprint,acknowledgment) VALUES('committed','task','disk','bound',NULL);CREATE TABLE effects(count INTEGER);INSERT INTO effects VALUES(1);");db.close()
        (profile/'journals'/'chat.jsonl').write_text(json.dumps(dict(seq=1,event=dict(type='toolStart')))+'\n')
        heartbeat=self.root/'orphan-effects'
        orphan="from pathlib import Path;import sys,time;p=Path(sys.argv[1]);p.write_text('start\\n');exec(\"while True:\\n with p.open('a') as f:f.write('effect\\\\n');f.flush()\\n time.sleep(.02)\")"
        code=self.launcher.read_text().replace('while True:time.sleep(.1)',"if os.environ['ZUSE_RUNTIME_OPERATION_ID']=='op-first':\n import subprocess,sys\n subprocess.Popen([sys.executable,'-c',"+repr(orphan)+","+repr(str(heartbeat))+"],start_new_session=True)\nwhile True:time.sleep(.1)")
        self.launcher.write_text(code)
        helper.rpc(self.data,self.request());first=self.child();deadline=time.monotonic()+3
        while time.monotonic()<deadline and not heartbeat.exists():time.sleep(.02)
        self.assertTrue(heartbeat.exists());os.kill(first['pid'],signal.SIGKILL)
        size=heartbeat.stat().st_size;time.sleep(.1);self.assertGreater(heartbeat.stat().st_size,size)
        request=self.request('repair-operation',True);request.pop('retained');request['action']='repair-stop'
        stopped=helper.rpc(self.data,request);self.assertEqual(stopped['phase'],'stopped')
        start=dict(request,action='repair-start',resumeOperationId='repair-resume',commandIds=['committed'])
        restarted=helper.rpc(self.data,start);self.assertEqual(restarted['phase'],'ready');self.assertNotEqual(restarted['childPid'],first['pid'])
        size=heartbeat.stat().st_size;time.sleep(.1);self.assertEqual(heartbeat.stat().st_size,size)
        self.assertEqual(restarted['recovery']['receipts'][0]['state'],'outcome-unknown')
        duplicate=helper.rpc(self.data,start);self.assertEqual(duplicate['childPid'],restarted['childPid'])
        late_stop=helper.rpc(self.data,request);self.assertEqual(late_stop['phase'],'ready')
        ping=helper.rpc(self.data,{'version':1,'action':'ping'});self.assertTrue(ping['childAlive']);self.assertEqual(ping['childPid'],restarted['childPid'])
        with self.assertRaises(helper.Rejected):helper.rpc(self.data,dict(start,commandIds=['different']))
        db=sqlite3.connect(database);self.assertEqual(db.execute('SELECT count FROM effects').fetchone()[0],1);db.close()
        self.assertEqual((self.checkout/'dirty').read_text(),'retained uncommitted work')

    def test_cold_relaunch_retires_owned_setsid_orphan_but_preserves_unrelated_process(self):
        heartbeat=self.root/'cold-orphan-effects'
        orphan="from pathlib import Path;import sys,time;p=Path(sys.argv[1]);p.write_text('start\\n');exec(\"while True:\\n with p.open('a') as f:f.write('effect\\\\n');f.flush()\\n time.sleep(.02)\")"
        code=self.launcher.read_text().replace('while True:time.sleep(.1)',"if os.environ['ZUSE_RUNTIME_OPERATION_ID']=='op-first':\n import subprocess,sys\n subprocess.Popen([sys.executable,'-c',"+repr(orphan)+","+repr(str(heartbeat))+"],start_new_session=True)\nwhile True:time.sleep(.1)")
        self.launcher.write_text(code)
        unrelated=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],start_new_session=True)
        try:
            helper.rpc(self.data,self.request());first=self.child();deadline=time.monotonic()+3
            while time.monotonic()<deadline and not heartbeat.exists():time.sleep(.01)
            self.assertTrue(heartbeat.exists());os.kill(first['pid'],signal.SIGKILL)
            size=heartbeat.stat().st_size;time.sleep(.1);self.assertGreater(heartbeat.stat().st_size,size,'native orphan keeps writing after Rust parent loss')
            request=self.request('op-cold-relaunch',True);ready=helper.rpc(self.data,request)
            self.assertNotEqual(ready['childPid'],first['pid']);size=heartbeat.stat().st_size
            time.sleep(.1);self.assertEqual(heartbeat.stat().st_size,size,'no old tool effects after new runtime admission')
            self.assertIsNone(unrelated.poll(),'provider-exec sibling is outside resident ownership')
            duplicate=helper.rpc(self.data,request);self.assertEqual(duplicate['childPid'],ready['childPid'])
            self.assertEqual((self.checkout/'dirty').read_text(),'retained uncommitted work')
        finally:
            unrelated.terminate();unrelated.wait(timeout=3)

    def test_wrong_mode_file_and_symlink_are_rejected(self):
        request=self.request();Path(request['bootTokenFile']).chmod(0o644)
        with self.assertRaises(helper.Rejected):helper.rpc(self.data,request)
        Path(request['bootTokenFile']).unlink();Path(request['bootTokenFile']).symlink_to(self.status/'not-a-token')
        with self.assertRaises(helper.Rejected):helper.rpc(self.data,request)

UPDATE_TESTS=Path(__file__).resolve().with_name('runtime-update.py')
update_spec=importlib.util.spec_from_file_location('runtime_update_tests',UPDATE_TESTS)
update_tests=importlib.util.module_from_spec(update_spec);update_spec.loader.exec_module(update_tests)
update_helper=update_tests.helper

@unittest.skipUnless(sys.platform=='linux','Linux guest peer credentials')
class UpdateWatchdogTest(unittest.TestCase):
    """A new runtime that dies before it proves healthy is rolled back by the
    supervisor (without its help) and the previous runtime relaunched once."""
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='zuse-watchdog-');self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name);os.chmod(self.root,0o755)
        self.data=self.root/'home'/'.zuse-data';self.data.mkdir(parents=True,mode=0o700);os.chmod(self.data.parent,0o755)
        self.updates=self.data/'runtime-updates';self.updates.mkdir(mode=0o700)
        self.status=self.root/'status';self.status.mkdir(mode=0o700);self.checkout=self.root/'repo';self.checkout.mkdir()
        self.state=self.root/'state';self.state.mkdir(mode=0o755)
        self.bin=self.root/'bin';self.bin.mkdir(mode=0o755);self.binary=self.bin/'zuse-cloud-runtime'
        self.old=update_tests.runtime_bytes('old');self.new=update_tests.runtime_bytes('new')
        self.binary.write_bytes(self.old);self.binary.chmod(0o755)
        self.recovery=self.root/'recovery';self.recovery.mkdir(mode=0o700)
        # A runtime that crashes at once when the new release is installed.
        self.launcher=self.root/'launch';self.launcher.write_text('''#!/usr/bin/env python3
import json,os,sys,time
from pathlib import Path
p=Path(os.environ['ZUSE_USER_DATA']);binary=Path(os.environ['ZUSE_RUNTIME_BINARY']).read_bytes()
with (p/'launches.jsonl').open('a') as out:out.write(json.dumps({'pid':os.getpid(),'retained':os.environ['ZUSE_RUNTIME_RETAINED'],'new':b'release new' in binary,'token':os.environ.get('ZUSE_RUNTIME_BOOT_TOKEN_FILE')})+'\\n')
(p/'cloud-runtime-identity.json').write_text('{}')
token=os.environ.get('ZUSE_RUNTIME_BOOT_TOKEN_FILE')
if token and os.path.exists(token):os.unlink(token)
if b'release new' in binary:sys.exit(1)
while True:time.sleep(.1)
''');self.launcher.chmod(0o700)
        self.server=None
    def serve(self,layout):
        self.server=subprocess.Popen(['python3',str(SCRIPT),'--data-dir',str(self.data),'--workspace-root',str(self.checkout),'--status-dir',str(self.status),
            '--launcher',str(self.launcher),'--binary',str(self.binary),'--recovery-root',str(self.recovery),'--recovery-launcher',str(self.root/'none'),
            '--update-state',str(self.state),'--update-helper',str(self.root/'none'),'--update-layout',layout,'serve'],
            stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,start_new_session=True)
        def stop():
            if self.server.poll() is None:self.server.terminate();self.server.wait(timeout=8)
            self.server.stderr.close()
        self.addCleanup(stop)
        end=time.monotonic()+4
        while time.monotonic()<end:
            try:helper.rpc(self.data,{'version':1,'action':'ping'});return
            except (OSError,helper.Rejected):time.sleep(.01)
        self.fail('supervisor failed to start')
    def launch(self):
        token=self.status/'rust-boot-op-first';token.write_text('private-fixture-token');token.chmod(0o600)
        helper.rpc(self.data,dict(version=1,action='launch',operationId='op-first',workspaceId='task',generation=2,storageIncarnationId='disk',apiUrl='http://127.0.0.1',retained=False,bootTokenFile=str(token)))
    def launches(self):
        path=self.data/'launches.jsonl'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
    def wait_for(self,condition,timeout=8):
        end=time.monotonic()+timeout
        while time.monotonic()<end:
            if condition():return
            time.sleep(.05)
        self.fail('condition not reached')
    def alive_pid(self):
        ping=helper.rpc(self.data,{'version':1,'action':'ping'})
        return ping['childPid'] if ping['childAlive'] else None
    def assert_no_further_relaunch(self,count):
        time.sleep(2.5)
        self.assertEqual(len(self.launches()),count)
        self.assertIsNone(self.alive_pid())
    def helper_env(self):
        overrides=dict(DATA_DIR=self.updates,STATE_DIR=self.state,RUNTIME_ROOT=self.root/'opt-runtime',INSTALLED=self.binary,
            PUBLIC_KEY=self.root/'release.pub',OWNER_UID=os.getuid(),TASK_UID=os.getuid(),TASK_GID=os.getgid(),ARCHITECTURE='x86_64')
        saved={k:os.environ.get(k) for k in ('ZUSE_RUNTIME_UPDATE_'+n for n in overrides)}
        def restore():
            for k,v in saved.items():
                if v is None:os.environ.pop(k,None)
                else:os.environ[k]=v
        self.addCleanup(restore)
        for name,value in overrides.items():os.environ['ZUSE_RUNTIME_UPDATE_'+name]=str(value)
    def commit_with_helper(self):
        """The real root helper installs the new release (as after a `now` swap)."""
        old_umask=os.umask(0o022);self.addCleanup(os.umask,old_umask)
        keys=self.root/'keys';keys.mkdir();private=keys/'release.pem'
        update_tests.openssl('genpkey','-algorithm','ed25519','-out',str(private))
        public=update_tests.openssl('pkey','-in',str(private),'-pubout','-outform','DER')[-32:]
        (self.root/'release.pub').write_text(__import__('base64').b64encode(public).decode()+'\n');(self.root/'release.pub').chmod(0o644)
        (self.root/'opt-runtime'/'releases').mkdir(parents=True,mode=0o755)
        self.helper_env()
        digest=update_tests.sha(self.new)
        release={'sha256':digest,'version':'2.0.0','architecture':'x86_64','storageFormats':['zuse-rust-engine-v1']}
        message=keys/'message';message.write_bytes(update_helper.signing_bytes(release))
        signature=update_tests.openssl('pkeyutl','-sign','-inkey',str(private),'-rawin','-in',str(message))
        staged=self.updates/'staged'/digest;staged.mkdir(parents=True,mode=0o700)
        (staged/'zuse-cloud-runtime').write_bytes(self.new)
        (staged/'release.json').write_text(json.dumps({'release':release,'signature':__import__('base64').b64encode(signature).decode(),'updateId':'update-1'}))
        (self.updates/'request.json').write_text(json.dumps({'sha256':digest,'updateId':'update-1','requestedAt':1}))
        self.assertEqual(update_helper.main(['run']),0)
        self.assertEqual(json.loads((self.state/'result.json').read_text())['outcome'],'committed')
        self.assertEqual(self.binary.read_bytes(),self.new)
        return digest
    def run_helper_on_rollback(self):
        """Stands in for the helper's systemd path unit (PathExists=rollback.json)."""
        done=threading.Event();outcome={}
        def watch():
            end=time.monotonic()+10
            while time.monotonic()<end:
                if (self.updates/'rollback.json').exists():
                    outcome['request']=json.loads((self.updates/'rollback.json').read_text())
                    outcome['code']=update_helper.main(['run']);break
                time.sleep(.02)
            done.set()
        threading.Thread(target=watch,daemon=True).start()
        return done,outcome

    def test_root_crash_in_probation_rolls_back_through_the_helper_and_relaunches_once(self):
        digest=self.commit_with_helper();self.serve('root')
        done,outcome=self.run_helper_on_rollback()
        self.launch()
        self.assertTrue(done.wait(10));self.assertEqual(outcome['code'],0)
        self.assertEqual(outcome['request'],{'updateId':'update-1','sha256':digest})
        result=json.loads((self.state/'result.json').read_text())
        self.assertEqual((result['outcome'],result['updateId']),('rolled-back','update-1'))
        self.assertEqual(self.binary.read_bytes(),self.old)
        self.wait_for(lambda:len(self.launches())==2 and self.alive_pid())
        first,second=self.launches()
        self.assertEqual((first['new'],second['new']),(True,False))
        # The failed start consumed the boot credential: the relaunch resumes the stored identity.
        self.assertEqual((second['retained'],second['token']),('true',None))
        self.assertEqual(self.alive_pid(),second['pid'])
        record=json.loads((self.data/'cloud-runtime-supervisor-update-rollbacks.json').read_text())['update-1']
        self.assertEqual((record['phase'],record['layout'],record['exitCode']),('relaunched','root',1))
        # The restored runtime also dying is left to normal recovery.
        os.kill(second['pid'],signal.SIGKILL)
        self.assert_no_further_relaunch(2)
        self.assertFalse((self.updates/'rollback.json').exists())

    def test_root_healthy_release_is_never_rolled_back(self):
        digest=update_tests.sha(self.new);self.binary.write_bytes(self.new)
        (self.state/'result.json').write_text(json.dumps({'updateId':'update-1','outcome':'committed','sha256':digest,'previousSha256':update_tests.sha(self.old),'errorCode':None}))
        healthy=self.updates/'healthy.json';healthy.write_text(json.dumps({'sha256':digest,'updateId':'update-1','at':1}));healthy.chmod(0o600)
        self.serve('root');self.launch()
        self.wait_for(lambda:len(self.launches())==1)
        self.assert_no_further_relaunch(1)
        self.assertFalse((self.updates/'rollback.json').exists())
        self.assertEqual(self.binary.read_bytes(),self.new)

    def test_owned_crash_in_probation_restores_previous_and_relaunches_once(self):
        journal=self.root/'runtime-update';journal.mkdir(mode=0o700)
        (journal/'previous').write_bytes(self.old);self.binary.write_bytes(self.new)
        entry=dict(version=1,phase='swapped',updateId='update-1',sha256=update_tests.sha(self.new),previousSha256=update_tests.sha(self.old),updatedAt=1)
        (journal/'journal.json').write_text(json.dumps(entry));(journal/'journal.json').chmod(0o600)
        self.serve('owned');self.launch()
        self.wait_for(lambda:len(self.launches())==2 and self.alive_pid())
        self.assertEqual(self.binary.read_bytes(),self.old)
        self.assertEqual(stat.S_IMODE(self.binary.stat().st_mode),0o755)
        saved=json.loads((journal/'journal.json').read_text())
        self.assertEqual((saved['phase'],saved['errorCode'],saved['updateId'],saved['previousSha256']),('rolled-back','runtime_update_rolled_back','update-1',entry['previousSha256']))
        self.assertEqual([l['new'] for l in self.launches()],[True,False])
        os.kill(self.launches()[1]['pid'],signal.SIGKILL)
        self.assert_no_further_relaunch(2)

    def test_owned_journal_committed_or_clean_exit_is_left_alone(self):
        journal=self.root/'runtime-update';journal.mkdir(mode=0o700)
        (journal/'previous').write_bytes(self.old);self.binary.write_bytes(self.new)
        entry=dict(version=1,phase='committed',updateId='update-1',sha256=update_tests.sha(self.new),previousSha256=update_tests.sha(self.old),updatedAt=1)
        (journal/'journal.json').write_text(json.dumps(entry));(journal/'journal.json').chmod(0o600)
        self.serve('owned');self.launch()
        self.wait_for(lambda:len(self.launches())==1)
        self.assert_no_further_relaunch(1)
        self.assertEqual(self.binary.read_bytes(),self.new)
        self.assertEqual(json.loads((journal/'journal.json').read_text())['phase'],'committed')

class ConnectionReadinessTest(unittest.TestCase):
    def test_delayed_listener_waits_before_one_request_is_sent(self):
        with tempfile.TemporaryDirectory(prefix='zuse-rpc-ready-') as directory:
            data=Path(directory);ipc=data/'.runtime-supervision';ipc.mkdir(mode=0o700);received=[];errors=[]
            def listener():
                try:
                    time.sleep(.15)
                    with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as server:
                        server.bind(str(ipc/'control.sock'));server.listen(1);server.settimeout(2)
                        with server.accept()[0] as connection:
                            received.append(json.loads(connection.recv(32768)))
                            connection.sendall(b'{"version":1,"status":"accepted"}\n')
                except BaseException as error:errors.append(error)
            thread=threading.Thread(target=listener);thread.start()
            try:self.assertEqual(helper.rpc(data,{'version':1,'action':'launch'},connect_timeout=1)['status'],'accepted')
            finally:thread.join(timeout=3)
            self.assertFalse(thread.is_alive());self.assertEqual(errors,[])
            self.assertEqual(received,[{'version':1,'action':'launch'}])
    def test_lost_reply_never_reconnects_or_replays_a_sent_request(self):
        with tempfile.TemporaryDirectory(prefix='zuse-rpc-uncertain-') as directory:
            data=Path(directory);ipc=data/'.runtime-supervision';ipc.mkdir(mode=0o700);received=[];errors=[]
            with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as server:
                server.bind(str(ipc/'control.sock'));server.listen(2);server.settimeout(.2)
                def listener():
                    try:
                        with server.accept()[0] as connection:received.append(json.loads(connection.recv(32768)))
                        try:
                            with server.accept()[0] as connection:received.append(json.loads(connection.recv(32768)))
                        except socket.timeout:pass
                    except BaseException as error:errors.append(error)
                thread=threading.Thread(target=listener);thread.start()
                try:
                    with self.assertRaises(helper.Rejected):helper.rpc(data,{'version':1,'action':'launch'})
                finally:thread.join(timeout=2)
            self.assertFalse(thread.is_alive());self.assertEqual(errors,[])
            self.assertEqual(received,[{'version':1,'action':'launch'}])
    def test_missing_listener_has_a_bounded_connection_deadline(self):
        with tempfile.TemporaryDirectory(prefix='zuse-rpc-deadline-') as directory:
            started=time.monotonic()
            with self.assertRaises(OSError):helper.rpc(Path(directory),{'version':1,'action':'ping'},connect_timeout=.05)
            self.assertLess(time.monotonic()-started,.5)

class AgentWarmupTest(unittest.TestCase):
    def test_boot_reads_only_the_recorded_agent_inside_harness_releases(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);releases=root/'releases';agent=releases/'release-1'/'codex'
            agent.parent.mkdir(parents=True);agent.write_bytes(b'x'*(3<<20))
            data=root/'data';data.mkdir()
            self.assertIsNone(helper.warm_agent(data,releases),'no record yet: nothing to read')
            (data/helper.AGENT_RECORD).write_text(str(agent))
            reader=helper.warm_agent(data,releases);self.assertIsNotNone(reader)
            self.assertEqual(reader.wait(5),0,'a separate reader process reads it through')
            outside=root/'elsewhere';outside.write_bytes(b'secret')
            (data/helper.AGENT_RECORD).write_text(str(outside))
            self.assertIsNone(helper.warm_agent(data,releases),'paths outside harness releases are ignored')
            (data/helper.AGENT_RECORD).write_text(str(agent.parent))
            self.assertIsNone(helper.warm_agent(data,releases),'directories are ignored')
            link=releases/'release-1'/'link';link.symlink_to(outside)
            (data/helper.AGENT_RECORD).write_text(str(link))
            self.assertIsNone(helper.warm_agent(data,releases),'symlinks leaving releases are ignored')
            record=data/helper.AGENT_RECORD;record.unlink();os.mkfifo(record)
            started=time.monotonic()
            self.assertIsNone(helper.warm_agent(data,releases),'a FIFO record is ignored without blocking')
            self.assertLess(time.monotonic()-started,1)
            record.unlink();record.symlink_to(data/'elsewhere-record')
            self.assertIsNone(helper.warm_agent(data,releases),'a linked record is not followed')
            record.unlink();record.write_text(str(agent)+' '*5000)
            self.assertIsNone(helper.warm_agent(data,releases),'an oversized record is ignored')
            (releases/'loop-a').symlink_to(releases/'loop-b');(releases/'loop-b').symlink_to(releases/'loop-a')
            record.write_text(str(releases/'loop-a'))
            self.assertIsNone(helper.warm_agent(data,releases),'a symlink loop is ignored')

if __name__=='__main__':unittest.main()
