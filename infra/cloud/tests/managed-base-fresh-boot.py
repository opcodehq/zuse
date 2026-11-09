"""Protected real filesystem/journals and real peer-credential IPC; no provider calls."""
import base64,copy,hashlib,importlib.util,json,os,socket,subprocess,sys,tempfile,time,unittest
from unittest.mock import patch
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('fresh_boot',ROOT/'managed-base-fresh-boot.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class Units:
 def __init__(self):self.starts=0;self.fail=False;self.bind=False;self.root=None;self.current=dict(MainPID='0',ActiveState='inactive',SubState='dead',UnitFileState='enabled',Result='success',ExecMainStatus='0',Job='',ControlGroup='')
 def state(self):return dict(self.current)
 def start(self):
  self.starts+=1;self.current.update(MainPID='123',ActiveState='active',SubState='running')
  if self.bind:(self.root/'.runtime-identity').write_text('not-an-authorized-clean-image')
  if self.fail:raise subprocess.TimeoutExpired(['systemctl'],15)
 def resident(self,*_):return dict(residentPid=123,residentStartTicks=456,bootNonceSha256='d'*64,ipcVerified=True,unbound=True,childAlive=False,recoveryAlive=False)
class Tests(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.root=Path(self.temp.name);self.layout=m.Layout(self.root,owner=os.getuid(),user=os.getuid())
  for n in ('/home/zuse/.zuse-data','/home/zuse/workspace','/var/lib/zuse/workspace','/var/lib/zuse/recovery','/opt/zuse/runtime-bootstrap'):
   p=self.layout.at(n);p.mkdir(parents=True,mode=0o700);p.chmod(0o700)
  self.pins={}
  for key,path,body in [('runtimeSha256','/usr/local/bin/zuse-cloud-runtime',b'synthetic pinned runtime'),('supervisorSha256','/usr/local/lib/zuse/runtime-supervisor.py',b'synthetic pinned supervisor'),('serviceSha256','/etc/systemd/system/'+m.UNIT,b'synthetic pinned unit'),('manifestSha256','/opt/zuse/preparation/status/manifest.json',m.canonical(dict(managedBase=True,credentialsRemoved=True,repositories=[],rustRuntimeSupervisorVersion=1))),('helperSha256',m.HELPER,(ROOT/'managed-base-fresh-boot.py').read_bytes())]:
   p=self.layout.at(path);p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(body);p.chmod(0o600);self.pins[key]=m.digest(body)
  self.request=dict(version=1,provider='boxd',purpose='initial-allocation',operationId='operation-one',workspaceId='workspace-one',generation=1,storageIncarnationId='disk-one',sourceMachineId='machine-one',imagePins=self.pins)
  self.units=Units();self.units.root=self.layout.data
 def boot(self,request=None):return m.bootstrap(request or self.request,self.layout,self.units)
 def test_initial_start_once_and_same_binding_replay_rechecks_current_state(self):
  first=self.boot();again=self.boot();self.assertEqual(self.units.starts,1);self.assertEqual(first['status'],'ready');self.assertEqual(first['requestSha256'],again['requestSha256']);self.assertEqual(first['imagePins'],self.pins)
  (self.layout.data/'cloud-runtime-identity.json').write_text('bound')
  with self.assertRaisesRegex(m.Rejected,'authority_present'):self.boot()
  self.assertEqual(self.units.starts,1)
 def test_incomplete_original_ipc_waits_without_authorizing_unknown_state(self):
  ipc=self.layout.data/'.runtime-supervision';ipc.mkdir(mode=0o700)
  with self.assertRaisesRegex(m.Pending,'ipc_not_ready'):m.resident_ipc(self.layout)
  lock=ipc/'supervisor.lock';lock.write_text('');lock.chmod(0o600)
  with self.assertRaisesRegex(m.Pending,'ipc_not_ready'):m.resident_ipc(self.layout)
  temporary=ipc/('.runtime-write-supervisor-'+'a'*32);temporary.write_text('{}');temporary.chmod(0o600)
  with self.assertRaisesRegex(m.Pending,'ipc_not_ready'):m.resident_ipc(self.layout)
  unexpected=ipc/'unexpected';unexpected.write_text('');unexpected.chmod(0o600)
  with self.assertRaisesRegex(m.Rejected,'ipc_state_unknown'):m.resident_ipc(self.layout)
  unexpected.unlink();temporary.unlink()
  lock.unlink();lock.symlink_to(self.layout.at('/usr/local/bin/zuse-cloud-runtime'))
  with self.assertRaisesRegex(m.Rejected,'ipc_metadata_invalid'):m.resident_ipc(self.layout)
 def test_complete_ipc_entries_still_reject_extra_files(self):
  import socket
  ipc=self.layout.data/'.runtime-supervision';ipc.mkdir(mode=0o700)
  for name in ('supervisor.json','supervisor.lock'):
   p=ipc/name;p.write_text('{}');p.chmod(0o600)
  with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as peer:
   peer.bind(str(ipc/'control.sock'));(ipc/'control.sock').chmod(0o600)
   self.assertEqual(m.resident_ipc(self.layout),ipc)
   unexpected=ipc/'other';unexpected.write_text('');unexpected.chmod(0o600)
   with self.assertRaisesRegex(m.Rejected,'ipc_state_unknown'):m.resident_ipc(self.layout)
 def test_start_observes_original_resident_until_ipc_appears(self):
  resident=self.units.resident;calls=[]
  def delayed(*args):
   calls.append(True)
   if len(calls)==1:raise FileNotFoundError()
   if len(calls)==2:raise m.Pending('fresh_boot_ipc_not_ready')
   return resident(*args)
  self.units.resident=delayed
  self.assertEqual(self.boot()['status'],'ready');self.assertEqual(self.units.starts,1);self.assertEqual(len(calls),3)
 def test_resident_observation_is_bounded_and_never_restarts(self):
  def unavailable(*_):raise FileNotFoundError()
  self.units.resident=unavailable
  with patch.object(m.time,'monotonic',side_effect=[0,1]),self.assertRaisesRegex(m.Pending,'fresh_boot_starting'):self.boot()
  self.assertEqual(self.units.starts,1)
  self.units.resident=lambda *_:(_ for _ in ()).throw(m.Rejected('fresh_boot_ipc_peer_mismatch'))
  with self.assertRaisesRegex(m.Rejected,'ipc_peer_mismatch'):self.boot()
  self.assertEqual(self.units.starts,1)
 def test_waiting_for_ipc_rejects_changed_original_unit(self):
  def changed(*_):
   self.units.current['MainPID']='456';raise FileNotFoundError()
  self.units.resident=changed
  with self.assertRaisesRegex(m.Rejected,'fresh_boot_unit_changed'):self.boot()
  self.assertEqual(self.units.starts,1)
 def test_copied_cache_parent_receipt_is_not_authority_for_another_source(self):
  self.boot();self.units.current.update(MainPID='0',ActiveState='inactive',SubState='dead')
  next_request=copy.deepcopy(self.request);next_request['sourceMachineId']='machine-two';next_request['purpose']='cache-preparation'
  next_result=self.boot(next_request);self.assertEqual(self.units.starts,2);self.assertNotEqual(next_result['requestSha256'],m.digest(m.canonical(self.request)))
  self.assertEqual(len(list(self.layout.journal.glob('*.json'))),2)
 def test_unknown_start_inspects_original_without_second_start(self):
  self.units.fail=True
  with self.assertRaisesRegex(m.Pending,'start_uncertain'):self.boot()
  result=self.boot();self.assertEqual(result['status'],'ready');self.assertEqual(self.units.starts,1)
 def test_stopped_original_ready_receipt_never_restarts_same_machine(self):
  self.boot();self.units.current.update(MainPID='0',ActiveState='inactive',SubState='dead')
  with self.assertRaisesRegex(m.Pending,'original_start_pending'):self.boot()
  self.assertEqual(self.units.starts,1)
 def test_current_authority_race_after_start_rejects_ready_receipt(self):
  self.units.bind=True
  with self.assertRaisesRegex(m.Rejected,'authority_present'):self.boot()
  self.assertEqual(self.units.starts,1);record=json.loads(next(self.layout.journal.glob('*.json')).read_text());self.assertEqual(record['phase'],'start-dispatched')
 def test_failed_disabled_or_stale_ipc_never_start(self):
  baseline=self.units.state()
  for field,value in [('UnitFileState','disabled'),('ActiveState','failed')]:
   self.units.current={**baseline,field:value}
   with self.subTest(field=field),self.assertRaises(m.Rejected):self.boot()
   self.assertEqual(self.units.starts,0)
  self.units.current=baseline;(self.layout.data/'.runtime-supervision').mkdir(mode=0o700)
  with self.assertRaisesRegex(m.Rejected,'stale_ipc'):self.boot()
  self.assertEqual(self.units.starts,0)
 def test_pending_original_unit_job_or_activation_never_issues_start(self):
  baseline=self.units.state()
  for change in (dict(Job='13'),dict(ActiveState='activating'),dict(ActiveState='deactivating')):
   self.units.current={**baseline,**change}
   with self.subTest(change=change),self.assertRaisesRegex(m.Pending,'unit_job_pending'):self.boot()
   self.assertEqual(self.units.starts,0)
 def test_all_fixed_pins_reject_tamper_before_start(self):
  for key in m.PIN_NAMES:
   request=copy.deepcopy(self.request);request['imagePins'][key]='0'*64
   with self.subTest(key=key),self.assertRaisesRegex(m.Rejected,'hash_mismatch'):self.boot(request)
  self.assertEqual(self.units.starts,0)
 def test_fixed_file_symlink_and_manifest_task_state_reject(self):
  p=self.layout.at('/usr/local/bin/zuse-cloud-runtime');p.rename(p.with_name('original'));p.symlink_to(p.with_name('original'))
  with self.assertRaisesRegex(m.Rejected,'symlink'):self.boot()
  self.assertEqual(self.units.starts,0)
 def test_work_checkout_status_recovery_and_agent_credentials_reject(self):
  for name in ('/home/zuse/workspace/file','/var/lib/zuse/workspace/runtime.json','/var/lib/zuse/recovery/archive'):
   p=self.layout.at(name);p.write_text('unexpected')
   with self.subTest(name=name),self.assertRaisesRegex(m.Rejected,'task_data_present'):self.boot()
   p.unlink()
  p=self.layout.at('/home/zuse/.codex/auth.json');p.parent.mkdir();p.write_text('synthetic-only')
  with self.assertRaisesRegex(m.Rejected,'agent_authority'):self.boot()
  self.assertEqual(self.units.starts,0)
 def test_all_existing_image_authority_denylist_paths_reject_before_start(self):
  for name in ('.codex/auth.json','.claude/.credentials.json','.grok/auth.json','.config/grok/auth.json','.config/gh','.zuse-github-cli','.git-credentials','.netrc','.zuse/cloud-auth','.zuse-image/provider-secrets.json'):
   p=self.layout.at('/home/zuse/'+name);p.parent.mkdir(parents=True,exist_ok=True);p.write_text('synthetic forbidden authority')
   with self.subTest(name=name),self.assertRaisesRegex(m.Rejected,'agent_authority'):self.boot()
   p.unlink()
  self.assertEqual(self.units.starts,0)
 def test_real_flock_owner_rejects_replaced_visible_lock(self):
  path=self.root/'owned-lock'
  script="import fcntl,os,sys,time;f=open(sys.argv[1],'w');fcntl.flock(f,fcntl.LOCK_EX);print('ready',flush=True);time.sleep(30)"
  child=subprocess.Popen([sys.executable,'-c',script,str(path)],stdout=subprocess.PIPE,text=True)
  try:
   self.assertEqual(child.stdout.readline().strip(),'ready');m.owned_visible_lock(path,child.pid)
   path.rename(self.root/'detached-lock');path.touch()
   with self.assertRaisesRegex(m.Rejected,'visible_lock_not_owned'):m.owned_visible_lock(path,child.pid)
  finally:child.terminate();child.wait(timeout=5);child.stdout.close()
 def test_ssh_must_be_absent_or_real_empty_directory(self):
  p=self.layout.at('/home/zuse/.ssh');p.mkdir();m.authority_free(self.layout)
  (p/'id_private').write_text('synthetic forbidden key')
  with self.assertRaisesRegex(m.Rejected,'ssh_authority'):self.boot()
  (p/'id_private').unlink();p.rmdir();p.symlink_to(self.root)
  with self.assertRaisesRegex(m.Rejected,'symlink'):self.boot()
  self.assertEqual(self.units.starts,0)
 def test_prepared_cache_payload_is_preserved(self):
  p=self.layout.at('/opt/zuse/project-caches/prepared/dependency');p.parent.mkdir(parents=True);p.write_text('exact-dependency')
  self.boot();self.assertEqual(p.read_text(),'exact-dependency')
 def test_request_whitelist_rejects_task_resume_secrets_and_wrong_types(self):
  for mutation in [dict(purpose='retained'),dict(provider='other'),dict(generation=True),dict(generation=0),dict(bootToken='must-not-be-accepted'),dict(sourceMachineId='../other')]:
   with self.subTest(mutation=list(mutation)),self.assertRaises(m.Rejected):self.boot({**self.request,**mutation})
  self.assertEqual(self.units.starts,0)
 def test_matching_journal_symlink_never_authorizes(self):
  key=m.digest(m.canonical(self.request));p=self.layout.journal/(key+'.json');p.symlink_to(self.layout.at('/opt/zuse/preparation/status/manifest.json'))
  with self.assertRaisesRegex(m.Rejected,'journal_invalid'):self.boot()
  self.assertEqual(self.units.starts,0)
class BoatTests(unittest.TestCase):
 def setUp(self):
  Tests.setUp(self);self.request['provider']='boat';self.units.current['UnitFileState']='disabled';self.units.disables=0
  def disable():self.units.disables+=1;self.units.current['UnitFileState']='disabled'
  self.units.disable=disable
 def boot(self,request=None):return m.bootstrap(request or self.request,self.layout,self.units)
 def test_boat_initial_stays_disabled_and_observes_original_once(self):
  self.assertEqual(self.boot()['status'],'ready');self.boot();self.assertEqual(self.units.starts,1);self.assertEqual(self.units.current['UnitFileState'],'disabled')
  self.units.current['UnitFileState']='enabled'
  with self.assertRaisesRegex(m.Rejected,'enabled_unit_required'):self.boot()
 def test_boat_new_cold_operation_requires_empty_authority_not_fake_identity(self):
  self.boot();self.units.current.update(MainPID='0',ActiveState='inactive',SubState='dead');self.request['operationId']='cold-operation'
  (self.layout.at('/home/zuse/workspace')/'dirty').write_text('untouched')
  with self.assertRaisesRegex(m.Rejected,'task_data_present'):self.boot()
  self.assertEqual(self.units.starts,1)
 def identities(self):
  self.request['purpose']='retained-resume';api='https://example.invalid';identity=dict(version=1,workspaceId=self.request['workspaceId'],generation=1,storageIncarnationId='disk-one',apiUrl=api)
  stored={**identity,'accountId':'account-one','bootstrapAcknowledged':True,'bootstrap':dict(workspaceId='workspace-one',runtimeGeneration=1,runtimeStorage=dict(storageIncarnationId='disk-one'),zuseAccountId='account-one',providerSandboxId='machine-one')}
  record=dict(version=1,identity=identity,receipts={'original-op':'a'*64})
  for name,v in [('cloud-runtime-identity.json',stored),('cloud-runtime-supervision.json',record)]:
   p=self.layout.data/name;p.write_text(json.dumps(v));p.chmod(0o600)
  return stored,record,identity
 def test_retained_identity_exact_bindings_and_no_secret_receipt(self):
  stored,record,identity=self.identities();self.assertEqual(m.retained_identity(self.layout,self.request),identity)
  for field,value in [('workspaceId','other'),('generation',2),('storageIncarnationId','other'),('sourceMachineId','other')]:
   with self.subTest(field=field),self.assertRaises(m.Rejected):m.retained_identity(self.layout,{**self.request,field:value})
  record['identity']['apiUrl']='https://other.invalid';p=self.layout.data/'cloud-runtime-supervision.json';p.write_text(json.dumps(record));p.chmod(0o600)
  with self.assertRaisesRegex(m.Rejected,'identity_mismatch'):m.retained_identity(self.layout,self.request)
 def test_actual_provider_sandbox_binding_is_mandatory_even_with_matching_alias(self):
  stored,_,_=self.identities();stored['bootstrap']['sourceSandboxId']='machine-one';path=self.layout.data/'cloud-runtime-identity.json'
  for value in [None,'foreign-machine']:
   if value is None:stored['bootstrap'].pop('providerSandboxId',None)
   else:stored['bootstrap']['providerSandboxId']=value
   path.write_text(json.dumps(stored));path.chmod(0o600)
   with self.subTest(value=value),self.assertRaisesRegex(m.Rejected,'source_mismatch'):m.retained_identity(self.layout,self.request)
  self.assertEqual(self.units.starts,0)
 def test_retained_live_peer_noop_and_replay_rechecks_identity(self):
  _,_,identity=self.identities();self.units.current.update(MainPID='123',ActiveState='active',SubState='running')
  self.units.resident=lambda *_:dict(residentPid=123,residentStartTicks=456,bootNonceSha256='d'*64,ipcVerified=True,unbound=False,identityVerified=True,childAlive=True,recoveryAlive=False)
  receipt=self.boot();self.assertTrue(receipt['resident']['identityVerified']);self.assertEqual(self.units.starts,0)
  p=self.layout.data/'cloud-runtime-identity.json';p.unlink()
  with self.assertRaises(FileNotFoundError):self.boot()
  self.assertEqual(self.units.starts,0)
 def test_retained_inactive_requires_original_absence_and_preserves_lock_path(self):
  self.identities();ipc=self.layout.data/'.runtime-supervision';ipc.mkdir(mode=0o700);record=ipc/'supervisor.json';record.write_text(json.dumps(dict(version=1,bootNonce='a'*32,residentPid=os.getpid())));record.chmod(0o600);lock=ipc/'supervisor.lock';lock.touch(mode=0o600);before=lock.stat().st_ino
  with self.assertRaisesRegex(m.Rejected,'original_resident_present'):self.boot()
  record.write_text(json.dumps(dict(version=1,bootNonce='a'*32,residentPid=2147483647)));record.chmod(0o600)
  self.units.resident=lambda *_:dict(residentPid=123,residentStartTicks=456,bootNonceSha256='d'*64,ipcVerified=True,unbound=False,identityVerified=True,childAlive=False,recoveryAlive=False)
  self.assertEqual(self.boot()['status'],'ready');self.assertEqual(lock.stat().st_ino,before);self.assertEqual(self.units.starts,1);self.assertEqual(self.units.current['UnitFileState'],'disabled')
 def test_retained_live_replacement_socket_refuses_start_and_preserves_winner(self):
  self.identities();ipc=self.layout.data/'.runtime-supervision';ipc.mkdir(mode=0o700);record=ipc/'supervisor.json';record.write_text(json.dumps(dict(version=1,bootNonce='a'*32,residentPid=2147483647)));record.chmod(0o600);(ipc/'supervisor.lock').touch(mode=0o600);path=ipc/'control.sock'
  with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as winner:
   winner.bind(str(path));path.chmod(0o600);winner.listen(4);inode=path.stat().st_ino
   with self.assertRaisesRegex(m.Rejected,'socket_live'):self.boot()
   self.assertEqual(self.units.starts,0);self.assertEqual(path.stat().st_ino,inode)
   with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:client.connect(str(path))
 def test_factory_disable_requires_exact_stop_and_is_once(self):
  self.request['purpose']='publication-proof';self.units.current['UnitFileState']='enabled'
  stop=dict(operationId=self.request['operationId'],runtimeSha256=self.pins['runtimeSha256'],bootServiceSha256=self.pins['serviceSha256'],atomicStopAcknowledged=True,legacyBuilder=False,originalResidentExited=True,supervisorLockReleased=True,runtimeDataAbsent=True,enabledBootServiceRetained=True,residentPid=2147483647,residentStartTicks=3)
  for patch in [dict(atomicStopAcknowledged=False),dict(operationId='other'),dict(legacyBuilder=True)]:
   with self.assertRaises(m.Rejected):m.disable_capture(self.request,{**stop,**patch},self.layout,self.units)
  receipt=m.disable_capture(self.request,stop,self.layout,self.units);self.assertTrue(receipt['unitDisabled']);m.disable_capture(self.request,stop,self.layout,self.units);self.assertEqual(self.units.disables,1)
  with self.assertRaisesRegex(m.Rejected,'original_mismatch'):m.disable_capture(self.request,{**stop,'residentStartTicks':4},self.layout,self.units)
class PeerTests(unittest.TestCase):
 def peer(self):
  temp=tempfile.TemporaryDirectory();self.addCleanup(temp.cleanup);path=Path(temp.name)/'peer.sock'
  code="import socket,sys,os,json;s=socket.socket(socket.AF_UNIX);s.bind(sys.argv[1]);s.listen(1);c,_=s.accept();c.settimeout(2);v=c.recv(1024);c.sendall(json.dumps({'version':1,'residentPid':os.getpid(),'bound':False}).encode()+b'\\n') if v else None;c.close();s.close()"
  child=subprocess.Popen([sys.executable,'-c',code,str(path)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);self.addCleanup(lambda:child.wait(timeout=3) if child.poll() is not None else (child.terminate(),child.wait(timeout=3)))
  until=time.monotonic()+2
  while not path.exists() and child.poll() is None and time.monotonic()<until:time.sleep(.005)
  self.assertTrue(path.exists());return path,child
 def test_real_unix_peer_matches_exact_pid_and_uid(self):
  path,child=self.peer();value=m.peer_ping(path,child.pid,os.getuid());self.assertEqual(value['residentPid'],child.pid);child.wait(timeout=3)
 def test_wrong_process_peer_rejected_before_any_ping_request(self):
  path,child=self.peer()
  with self.assertRaisesRegex(m.Rejected,'peer_mismatch'):m.peer_ping(path,child.pid+1,os.getuid())
  child.wait(timeout=3)
if __name__=='__main__':unittest.main()
