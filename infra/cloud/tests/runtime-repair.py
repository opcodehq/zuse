#!/usr/bin/env python3
"""Actual child loss, detached orphan retirement and SQLite commit-before-ACK."""
import importlib.util,json,os,re
from pathlib import Path
import signal,sqlite3,subprocess,sys,tempfile,time,unittest
SCRIPT=Path(__file__).resolve().parents[1]/'runtime-repair.py'
spec=importlib.util.spec_from_file_location('repair',SCRIPT);repair=importlib.util.module_from_spec(spec);spec.loader.exec_module(repair)
@unittest.skipUnless(sys.platform=='linux','Linux process fencing')
class RuntimeRepairTest(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory(prefix='zuse-repair-');self.addCleanup(self.temp.cleanup);self.data=Path(self.temp.name)
  self.profile=self.data/'orgs'/'account'/'user';self.profile.mkdir(parents=True);self.journals=self.profile/'journals';self.journals.mkdir()
  self.database=self.profile/'zuse.sqlite';self.identity=dict(workspaceId='task',generation=3,storageIncarnationId='disk',apiUrl='http://127.0.0.1')
  marker=dict(version=1,workspaceId='task',accountId='account',storage=dict(format='zuse-rust-engine-v1',storageIncarnationId='disk'),database=str(self.database.relative_to(self.data)),ready=True)
  path=self.data/'rust-runtime-storage.json';path.write_text(json.dumps(marker));path.chmod(0o600)
  db=sqlite3.connect(self.database);db.executescript("CREATE TABLE zuse_cloud_storage_identity(singleton INTEGER,workspace_id TEXT,account_id TEXT,storage_incarnation_id TEXT);INSERT INTO zuse_cloud_storage_identity VALUES(1,'task','account','disk');CREATE TABLE processed_commands(command_id TEXT PRIMARY KEY,processed_at INTEGER);CREATE TABLE zuse_cloud_command_receipts(command_id TEXT PRIMARY KEY,workspace_id TEXT,storage_incarnation_id TEXT,fingerprint TEXT,acknowledgment TEXT,lease_json TEXT,delivered INTEGER DEFAULT 0);CREATE TRIGGER seed_lease AFTER INSERT ON zuse_cloud_command_receipts BEGIN UPDATE zuse_cloud_command_receipts SET lease_json=json_object('leaseToken','fixture-lease','storageIncarnationId',NEW.storage_incarnation_id,'command',json_object('commandId',NEW.command_id,'workspaceId',NEW.workspace_id,'fingerprint',NEW.fingerprint)) WHERE command_id=NEW.command_id; END;CREATE TABLE effects(count INTEGER);INSERT INTO effects VALUES(0);");db.close()
 def test_committed_action_missing_ack_does_not_reexecute_after_detached_orphan_is_retired(self):
  self.assertTrue(repair.enable_subreaper());heartbeat=self.data/'orphan-effects';ready=self.data/'ready'
  orphan=self.data/'orphan.py';orphan.write_text("import sys,time,os\nfrom pathlib import Path\np=Path(sys.argv[1]);p.write_text('start\\n')\nwhile True:\n with p.open('a') as out:out.write('effect\\n');out.flush()\n time.sleep(.02)\n")
  parent=self.data/'parent.py';parent.write_text("import sqlite3,sys,subprocess,time\nfrom pathlib import Path\ndb=sqlite3.connect(sys.argv[1]);db.execute(\"INSERT INTO processed_commands VALUES('original',1)\");db.execute('UPDATE effects SET count=count+1');db.execute(\"INSERT INTO zuse_cloud_command_receipts(command_id,workspace_id,storage_incarnation_id,fingerprint,acknowledgment) VALUES('original','task','disk','bound-fingerprint',NULL)\");db.commit();db.close()\nsubprocess.Popen([sys.executable,sys.argv[2],sys.argv[3]],start_new_session=True)\nPath(sys.argv[4]).write_text('ready')\nwhile True:time.sleep(.1)\n")
  child=subprocess.Popen([sys.executable,str(parent),str(self.database),str(orphan),str(heartbeat),str(ready)],start_new_session=True)
  try:
   deadline=time.monotonic()+3
   while time.monotonic()<deadline and (not ready.exists() or not heartbeat.exists()):time.sleep(.02)
   self.assertTrue(ready.exists());self.assertTrue(heartbeat.exists())
   child.kill();child.wait(timeout=3);before=heartbeat.stat().st_size;time.sleep(.1);self.assertGreater(heartbeat.stat().st_size,before,'setsid orphan survives original parent loss')
   self.assertGreaterEqual(repair.retire_owned(os.getpid()),1);after=heartbeat.stat().st_size;time.sleep(.1);self.assertEqual(heartbeat.stat().st_size,after,'old harness cannot race new execution')
   (self.journals/'chat.jsonl').write_text(json.dumps(dict(seq=1,event=dict(type='toolStart')))+'\n'+ '{torn')
   report=repair.reconcile(self.data,self.identity,['original','unproven'])
   self.assertEqual(report['interruptedChats'],['chat']);self.assertEqual(int((self.journals/'chat.resume').read_text()),repair.MAX_RESUME_ATTEMPTS)
   self.assertEqual(report['receipts'][0]['state'],'outcome-unknown');self.assertIsNone(report['receipts'][1]['fingerprint'])
   # Fresh child uses the existing native processed ledger, never the old prompt.
   code="import sqlite3,sys;db=sqlite3.connect(sys.argv[1]);known=db.execute(\"SELECT 1 FROM processed_commands WHERE command_id='original'\").fetchone();db.execute('UPDATE effects SET count=count+1') if not known else None;db.commit()"
   subprocess.run([sys.executable,'-c',code,str(self.database)],check=True)
   db=sqlite3.connect(self.database);self.assertEqual(db.execute('SELECT count FROM effects').fetchone()[0],1);db.close()
   self.assertEqual(repair.reconcile(self.data,self.identity,['original'])['quarantinedCommandCount'],1)
  finally:
   if child.poll() is None:child.kill();child.wait(timeout=3)
   repair.retire_owned(os.getpid())
 def test_root_owned_setsid_orphan_is_fenced_and_retired(self):
  if os.getuid()!=0 and subprocess.run(['sudo','-n','true'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode:self.skipTest('root-tool fixture requires configured guest sudo')
  self.assertTrue(repair.enable_subreaper());marker=self.data/'root-orphan'
  code="import os,sys,time;from pathlib import Path;os.setsid();p=Path(sys.argv[1]);p.write_text(str(os.getuid()));p.chmod(0o644);exec('while True: time.sleep(.1)')"
  command=[sys.executable,'-c',code,str(marker)]
  if os.getuid()!=0:command=['sudo','-n',*command]
  child=subprocess.Popen(command,start_new_session=False)
  try:
   deadline=time.monotonic()+3
   while time.monotonic()<deadline and not marker.exists():time.sleep(.02)
   self.assertEqual(marker.read_text(),'0');self.assertGreaterEqual(repair.retire_owned(os.getpid()),1)
   deadline=time.monotonic()+2
   while time.monotonic()<deadline and child.poll() is None:time.sleep(.02)
   self.assertIsNotNone(child.poll())
  finally:
   if child.poll() is None:repair.retire_owned(os.getpid())
   child.wait(timeout=3)
 def test_unbound_ack_is_never_proof_and_corrupt_original_lease_fails_closed(self):
  db=sqlite3.connect(self.database)
  db.execute("INSERT INTO zuse_cloud_command_receipts(command_id,workspace_id,storage_incarnation_id,fingerprint,acknowledgment) VALUES('wrong-ack','task','disk','original',?)",(json.dumps(dict(commandId='other',fingerprint='original',state='applied')),))
  db.commit();db.close()
  result=repair.reconcile(self.data,self.identity,['wrong-ack'])
  self.assertIsNone(result['receipts'][0]['state'])
  db=sqlite3.connect(self.database)
  db.execute("INSERT INTO zuse_cloud_command_receipts(command_id,workspace_id,storage_incarnation_id,fingerprint,acknowledgment) VALUES('corrupt','task','disk','bound',NULL)")
  db.execute("UPDATE zuse_cloud_command_receipts SET lease_json=? WHERE command_id='corrupt'",(json.dumps(dict(leaseToken='original',storageIncarnationId='wrong-disk',command=dict(commandId='corrupt',workspaceId='task',fingerprint='bound'))),))
  db.commit();db.close()
  with self.assertRaisesRegex(RuntimeError,'lease_binding_invalid'):repair.reconcile(self.data,self.identity,['corrupt'])
  db=sqlite3.connect(self.database);self.assertIsNone(db.execute("SELECT acknowledgment FROM zuse_cloud_command_receipts WHERE command_id='corrupt'").fetchone()[0]);db.close()
 def test_storage_fence_and_known_receipt_are_checked_without_rewriting_wal_receipts(self):
  db=sqlite3.connect(self.database);ack=json.dumps(dict(commandId='known',fingerprint='original-fingerprint',leaseToken='fixture-lease',state='applied'))
  db.execute("INSERT INTO zuse_cloud_command_receipts(command_id,workspace_id,storage_incarnation_id,fingerprint,acknowledgment) VALUES('known','task','disk','original-fingerprint',?)",(ack,));db.commit();db.close()
  (self.journals/'complete.jsonl').write_text(json.dumps(dict(seq=1,event=dict(type='done')))+'\n')
  result=repair.reconcile(self.data,self.identity,['known']);self.assertEqual(result['interruptedChats'],[]);self.assertEqual(result['receipts'][0]['state'],'applied')
  db=sqlite3.connect(self.database);self.assertEqual(db.execute('SELECT acknowledgment FROM zuse_cloud_command_receipts').fetchone()[0],ack);db.close()
  wrong=dict(self.identity,storageIncarnationId='other')
  with self.assertRaisesRegex(RuntimeError,'fenced'):repair.reconcile(self.data,wrong,['known'])
 def test_legacy_real_engine_schema_without_optional_mailbox_keeps_history_and_outcomes_unknown(self):
  self.database.unlink()
  # Load the real append-only engine migration strings, not a hand-written
  # mailbox fixture that assumes the optional subsystem has been instantiated.
  source=(SCRIPT.parents[2]/'crates/sync/src/store.rs').read_text()
  block=source.split('const MIGRATIONS:',1)[1].split('];',1)[0]
  block=re.sub(r'//[^\n]*','',block)
  migrations=[json.loads('"'+raw.replace('\n','\\n')+'"') for raw in re.findall(r'"((?:[^"\\]|\\.)*)"',block,re.S)]
  self.assertGreaterEqual(len(migrations),8)
  with sqlite3.connect(self.database) as db:
   db.execute('PRAGMA journal_mode=WAL')
   db.execute('CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at INTEGER NOT NULL) STRICT')
   for version,sql in enumerate(migrations,1):
    db.executescript(sql);db.execute('INSERT INTO schema_migrations VALUES(?,?)',(version,1))
   db.executescript("CREATE TABLE zuse_cloud_storage_identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1),workspace_id TEXT NOT NULL,account_id TEXT NOT NULL,storage_incarnation_id TEXT NOT NULL);INSERT INTO zuse_cloud_storage_identity VALUES(1,'task','account','disk')")
   db.execute("INSERT INTO processed_commands VALUES('original-legacy',1)")
   db.execute("INSERT INTO snapshots(doc_id,bytes,saved_at) VALUES('legacy-chat',?,1)",(b'preserved-real-engine-snapshot',))
   db.execute("INSERT INTO tool_blobs VALUES('legacy-chat/tool','legacy-chat',?)",(b'preserved-tool-output',))
  journal=self.journals/'legacy-chat.jsonl';journal.write_text(json.dumps(dict(seq=1,event=dict(type='done')))+'\n');before=journal.read_bytes()
  report=repair.reconcile(self.data,self.identity,['original-legacy','never-admitted'])
  self.assertEqual(report['quarantinedCommandCount'],0);self.assertEqual(report['interruptedChats'],[])
  self.assertEqual(report['receipts'],[dict(commandId=cid,fingerprint=None,state=None) for cid in ['original-legacy','never-admitted']])
  self.assertEqual(journal.read_bytes(),before);self.assertFalse(journal.with_suffix('.resume').exists())
  with sqlite3.connect(self.database) as db:
   self.assertIsNone(db.execute("SELECT type FROM sqlite_master WHERE name='zuse_cloud_command_receipts'").fetchone(),'repair must not manufacture a mailbox receipt table')
   self.assertEqual(db.execute('SELECT * FROM processed_commands').fetchall(),[('original-legacy',1)])
   self.assertEqual(db.execute('SELECT bytes FROM snapshots').fetchone()[0],b'preserved-real-engine-snapshot')
   self.assertEqual(db.execute('SELECT bytes FROM tool_blobs').fetchone()[0],b'preserved-tool-output')
   self.assertEqual(db.execute('SELECT count(*) FROM schema_migrations').fetchone()[0],len(migrations))
 def test_present_malformed_mailbox_schema_remains_fail_closed(self):
  with sqlite3.connect(self.database) as db:
   db.execute('DROP TABLE zuse_cloud_command_receipts')
   db.execute('CREATE TABLE zuse_cloud_command_receipts(unrelated TEXT)')
  with self.assertRaises(sqlite3.OperationalError):repair.reconcile(self.data,self.identity,['unproven'])
  with sqlite3.connect(self.database) as db:self.assertEqual(db.execute('SELECT count(*) FROM processed_commands').fetchone()[0],0)
if __name__=='__main__':unittest.main()
