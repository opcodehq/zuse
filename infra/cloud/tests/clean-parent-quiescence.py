#!/usr/bin/env python3
"""Real owned processes/flocks/socket paths; no provider/system service effects."""
import importlib.util,json,os,signal,socket,subprocess,time,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def load(name,path):
 s=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
fixture=load('resident_fixture',Path(__file__).with_name('runtime-supervisor.py'))
q=load('clean_quiescence',ROOT/'clean-parent-quiescence.py')
h=load('clean_hygiene',ROOT/'base-supervisor-hygiene.py')

class ParentQuiescence(unittest.TestCase):
 setUp=fixture.RuntimeSupervisorTest.setUp
 start=fixture.RuntimeSupervisorTest.start
 stop=fixture.RuntimeSupervisorTest.stop
 request=fixture.RuntimeSupervisorTest.request
 child=fixture.RuntimeSupervisorTest.child
 def clean(self):(self.checkout/'dirty').unlink(missing_ok=True)
 def retire(self,lock,**changes):
  pid=self.server.pid;fact=q.process(pid);stops=[]
  def stop_unit():
   self.assertIn(q.process(pid)['state'],('T','t'));stops.append(pid)
  args=dict(expected_argv=list(self.server.args),frozen_clean=lambda:h.contents_clean(self.data,os.getuid(),self.checkout,self.status,self.recovery),service=lambda:dict(MainPID=str(pid),ActiveState='active',UnitFileState='enabled',ControlGroup='/test/zuse-runtime-supervisor.service'),cgroup_members=lambda _:[pid],stop_unit=stop_unit)
  args.update(changes)
  result=q.retire_idle(pid,fact['start'],os.getuid(),lock,**args)
  self.server.wait(timeout=2);self.assertEqual(stops,[pid]);return result
 def cleanup(self,lock,receipt):
  return h.clean_stopped(self.data,os.getuid(),receipt['residentPid'],receipt['residentStartTicks'],self.checkout,self.status,self.recovery,expected_paths=lock.expected,held_lock=lock.lock_fd)
 def winner(self):
  server=subprocess.Popen(list(self.server.args),stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
  self.addCleanup(lambda: self.finish(server))
  end=time.monotonic()+3
  while time.monotonic()<end:
   try:
    ping=fixture.helper.rpc(self.data,{'version':1,'action':'ping'})
    if ping['residentPid']==server.pid:return server
   except (OSError,fixture.helper.Rejected):pass
   time.sleep(.01)
  self.fail('replacement resident did not acquire current lock')
 def finish(self,server):
  if server.poll() is None:server.terminate();server.wait(timeout=8)
 def current_ping(self,pid):self.assertEqual(fixture.helper.rpc(self.data,{'version':1,'action':'ping'})['residentPid'],pid)
 def test_owned_idle_process_retires_without_path_cleanup_then_fresh_resident_owns_current_lock(self):
  self.clean();lock=q.VisibleLock(self.data,os.getuid())
  unrelated=subprocess.Popen(['/bin/sleep','30']);self.addCleanup(lambda:self.finish(unrelated))
  try:
   self.assertFalse(lock.protect(expected_pid=self.server.pid))
   receipt=self.retire(lock);self.assertTrue(receipt['retiredWithoutPathCleanup'])
   self.assertEqual(self.server.returncode,-signal.SIGKILL)
   self.assertTrue((self.data/'.runtime-supervision/control.sock').is_socket(),'legacy finally never unlinked a path')
   cleaned=self.cleanup(lock,receipt);self.assertTrue(cleaned['supervisorLockReleased']);self.assertEqual(list(self.data.iterdir()),[])
   self.assertIsNone(unrelated.poll())
  finally:lock.close()
  replacement=self.winner();self.current_ping(replacement.pid)
 def test_real_detached_live_listener_retires_then_current_lock_and_socket_recover(self):
  self.clean();old_ipc=self.data/'.runtime-supervision';record=(old_ipc/'supervisor.json').read_bytes();saved=self.root/'detached-live-ipc';old_ipc.rename(saved)
  old_ipc.mkdir(mode=0o700);(old_ipc/'supervisor.lock').touch(mode=0o600);(old_ipc/'supervisor.json').write_bytes(record);(old_ipc/'supervisor.json').chmod(0o600)
  with socket.socket(socket.AF_UNIX) as stale:stale.bind(str(old_ipc/'control.sock'))
  (old_ipc/'control.sock').chmod(0o600)
  with socket.socket(socket.AF_UNIX) as client:
   with self.assertRaises(ConnectionRefusedError):client.connect(str(old_ipc/'control.sock'))
  with socket.socket(socket.AF_UNIX) as client:
   client.connect(str(saved/'control.sock'));client.sendall(b'{"version":1,"action":"ping"}\n');self.assertEqual(json.loads(client.recv(4096))['residentPid'],self.server.pid)
  lock=q.VisibleLock(self.data,os.getuid())
  try:
   self.assertTrue(lock.protect(expected_pid=self.server.pid),'new visible lock is distinct from live detached old lock')
   receipt=self.retire(lock);self.cleanup(lock,receipt)
   with socket.socket(socket.AF_UNIX) as client:
    with self.assertRaises(ConnectionRefusedError):client.connect(str(saved/'control.sock'))
  finally:lock.close()
  replacement=self.winner();self.current_ping(replacement.pid)
 def test_replaced_ipc_directory_never_unlinks_winner_or_serves_old_listener(self):
  self.clean();lock=q.VisibleLock(self.data,os.getuid())
  try:
   receipt=self.retire(lock);saved=self.root/'retired-ipc';(self.data/'.runtime-supervision').rename(saved)
   winner=self.winner();socket_path=self.data/'.runtime-supervision/control.sock';identity=socket_path.stat().st_ino
   with self.assertRaisesRegex(h.Rejected,'publication_ipc_directory_changed'):self.cleanup(lock,receipt)
   self.assertEqual(socket_path.stat().st_ino,identity);self.current_ping(winner.pid)
   with socket.socket(socket.AF_UNIX) as client:
    with self.assertRaises(ConnectionRefusedError):client.connect(str(saved/'control.sock'))
  finally:lock.close()
 def test_replaced_data_directory_never_unlinks_current_winner(self):
  self.clean();lock=q.VisibleLock(self.data,os.getuid())
  try:
   receipt=self.retire(lock);self.data.rename(self.root/'retired-data');self.data.mkdir(mode=0o700)
   winner=self.winner();path=self.data/'.runtime-supervision/control.sock';inode=path.stat().st_ino
   with self.assertRaisesRegex(h.Rejected,'publication_data_directory_changed'):self.cleanup(lock,receipt)
   self.assertEqual(path.stat().st_ino,inode);self.current_ping(winner.pid)
  finally:lock.close()
 def test_current_visible_lock_with_another_resident_is_rejected_before_signals(self):
  self.clean();old=self.server;(self.data/'.runtime-supervision').rename(self.root/'old-ipc')
  winner=self.winner();lock=q.VisibleLock(self.data,os.getuid())
  try:
   with self.assertRaisesRegex(q.Rejected,'parent_visible_lock_has_other_owner'):self.retire(lock)
   self.assertIsNone(old.poll());self.current_ping(winner.pid)
  finally:
   lock.close()
   # Legacy teardown is intentionally unsafe; never SIGTERM this detached old
   # test resident against the winner pathname during fixture cleanup either.
   if old.poll() is None:old.kill();old.wait(timeout=2)
 def test_lock_replacement_is_detected_before_retirement(self):
  self.clean();lock=q.VisibleLock(self.data,os.getuid());path=self.data/'.runtime-supervision/supervisor.lock'
  try:
   path.rename(self.root/'old-lock');path.touch(mode=0o600)
   with self.assertRaisesRegex(q.Rejected,'parent_visible_path_changed'):self.retire(lock)
   self.assertIsNone(self.server.poll())
  finally:lock.close()
 def test_wrong_start_uid_program_or_cgroup_never_signals_or_stops(self):
  self.clean();lock=q.VisibleLock(self.data,os.getuid());pid=self.server.pid;start=q.process(pid)['start'];stops=[]
  try:
   for changes in [dict(start=start+1),dict(uid=os.getuid()+1),dict(expected_argv=['wrong']),dict(cgroup_members=lambda _:[pid,2147483647])]:
    args=dict(pid=pid,start=start,uid=os.getuid(),lock=lock,expected_argv=list(self.server.args),frozen_clean=lambda:h.contents_clean(self.data,os.getuid(),self.checkout,self.status,self.recovery),service=lambda:dict(MainPID=str(pid),ActiveState='active',UnitFileState='enabled',ControlGroup='/test/zuse-runtime-supervisor.service'),cgroup_members=lambda _:[pid],stop_unit=lambda:stops.append(pid));args.update(changes)
    with self.assertRaises(q.Rejected):q.retire_idle(**args)
    self.assertIsNone(self.server.poll());self.assertEqual(stops,[])
  finally:lock.close()
 def test_live_owned_descendant_prevents_clean_parent_retirement(self):
  self.clean();fixture.helper.rpc(self.data,self.request());self.child();lock=q.VisibleLock(self.data,os.getuid())
  try:
   with self.assertRaisesRegex(q.Rejected,'parent_live_descendants'):self.retire(lock)
   self.assertIsNone(self.server.poll())
  finally:lock.close()
 def test_binding_appearing_at_freeze_boundary_prevents_stop_and_kill(self):
  self.clean();lock=q.VisibleLock(self.data,os.getuid());stops=[]
  def frozen_clean():
   self.assertIn(q.process(self.server.pid)['state'],('T','t'))
   (self.data/'cloud-runtime-identity.json').write_text('new binding at boundary')
   h.contents_clean(self.data,os.getuid(),self.checkout,self.status,self.recovery)
  try:
   with self.assertRaisesRegex(h.Rejected,'publication_runtime_data_present'):self.retire(lock,frozen_clean=frozen_clean,stop_unit=lambda:stops.append(self.server.pid))
   self.assertEqual(stops,[]);self.assertIsNone(self.server.poll())
   until=time.monotonic()+1
   while q.process(self.server.pid)['state'] in ('T','t') and time.monotonic()<until:time.sleep(.005)
   self.assertNotIn(q.process(self.server.pid)['state'],('T','t'));self.current_ping(self.server.pid)
  finally:lock.close()

class StopObservation(unittest.TestCase):
 def test_failed_unit_after_real_pidfd_retirement_remains_rejected_with_terminal_facts(self):
  child=subprocess.Popen(['/bin/sleep','30']);start=q.process(child.pid)['start'];fd=os.pidfd_open(child.pid)
  signal.pidfd_send_signal(fd,signal.SIGKILL);child.wait(timeout=2);os.close(fd)
  value=dict(MainPID='0',ActiveState='failed',SubState='failed',Result='signal',ExecMainCode='2',ExecMainStatus='9',UnitFileState='enabled',Job='0',Environment='secret-must-not-escape')
  with self.assertRaisesRegex(q.Rejected,'parent_stop_job_unconfirmed') as caught:
   q.confirm_stopped(child.pid,start,'/test/zuse-runtime-supervisor.service',service=lambda:value,cgroup_members=lambda _:[],jobs=lambda:[dict(id=17,type='stop',state='running')],timeout=.001)
  facts=caught.exception.facts
  self.assertFalse(facts['originalIdentityPresent']);self.assertTrue(facts['cgroup']['empty']);self.assertEqual(facts['jobs'][0]['id'],17)
  self.assertEqual(facts['unit']['Result'],'signal');self.assertNotIn('Environment',facts['unit']);self.assertNotIn('secret',json.dumps(facts))
 def test_stopped_enabled_unit_succeeds_without_waiving_terminal_predicate(self):
  value=dict(MainPID='0',ActiveState='inactive',SubState='dead',Result='success',UnitFileState='enabled',ControlGroup='',Job='')
  facts=q.confirm_stopped(2147483647,1,'/test/zuse-runtime-supervisor.service',service=lambda:value,cgroup_members=lambda _:[],jobs=lambda:[],timeout=.01)
  self.assertEqual(facts['unit'],value);self.assertTrue(facts['cgroup']['empty'])
  for changes in (dict(jobs=lambda:[dict(id=3,type='stop',state='running')]),dict(cgroup_members=lambda _:[123])):
   with self.assertRaisesRegex(q.Rejected,'parent_stop_job_unconfirmed'):q.confirm_stopped(2147483647,1,'/test/zuse-runtime-supervisor.service',service=lambda:value,cgroup_members=changes.get('cgroup_members',lambda _:[]),jobs=changes.get('jobs',lambda:[]),timeout=.01)
  for key in ('ControlGroup','Job'):
   saved=value.pop(key)
   with self.assertRaisesRegex(q.Rejected,'parent_stop_job_unconfirmed'):q.confirm_stopped(2147483647,1,'/test/zuse-runtime-supervisor.service',service=lambda:value,cgroup_members=lambda _:[],jobs=lambda:[],timeout=.01)
   value[key]=saved
  value['UnitFileState']='disabled'
  with self.assertRaisesRegex(q.Rejected,'parent_stop_job_unconfirmed'):q.confirm_stopped(2147483647,1,'/test/zuse-runtime-supervisor.service',service=lambda:value,cgroup_members=lambda _:[],jobs=lambda:[],timeout=.01)
 def test_observation_failure_preserves_static_error_without_subprocess_output(self):
  def unavailable():raise subprocess.CalledProcessError(1,['secret-argv'],output='secret-output')
  with self.assertRaisesRegex(q.Rejected,'parent_stop_observation_failed') as caught:
   q.confirm_stopped(2147483647,1,'/test/zuse-runtime-supervisor.service',service=unavailable,cgroup_members=lambda _:[],jobs=lambda:[],timeout=.01)
  self.assertEqual(caught.exception.facts['observationErrorClass'],'CalledProcessError');self.assertNotIn('secret',json.dumps(caught.exception.facts))

class IntentionalStopNormalization(unittest.TestCase):
 def setUp(self):
  child=subprocess.Popen(['/bin/sleep','30']);self.pid=child.pid;self.start=q.process(child.pid)['start'];fd=os.pidfd_open(child.pid);signal.pidfd_send_signal(fd,signal.SIGKILL);child.wait(timeout=2);os.close(fd)
  self.group='/test/zuse-runtime-supervisor.service';self.state=dict(MainPID='0',ActiveState='failed',SubState='failed',Result='signal',ExecMainCode='2',ExecMainStatus='9',UnitFileState='enabled',Job='',ControlGroup='')
  # Synthetic already-validated factory receipt; PID absence comes from the real
  # child above. Other tests exercise actual retire_idle ownership/flock paths.
  self.proof=dict(residentPid=self.pid,residentStartTicks=self.start,residentUid=10001,controlGroup=self.group,unitHadOnlyResident=True,frozenCleanVerified=True,pidfdRetirement=True,retiredWithoutPathCleanup=True,visibleLockProtected=True,postRetirementFacts=dict(originalPid=self.pid,originalStartTicks=self.start,originalIdentityPresent=False,stopSubmitted=True,pidfdKillSubmitted=True))
  self.resets=0;self.authorizations=0
 def reset(self):
  self.resets+=1;self.state.update(ActiveState='inactive',SubState='dead',Result='success')
 def authorize(self):self.authorizations+=1
 def normalize(self,**changes):
  args=dict(authorize=self.authorize,service=lambda:dict(self.state),cgroup_members=lambda _:[],jobs=lambda:[],reset=self.reset);args.update(changes)
  return q.normalize_intentional_stop(self.pid,self.start,self.group,self.proof,**args)
 def test_exact_intentional_signal9_is_normalized_once_then_strictly_stopped(self):
  receipt=self.normalize();self.assertEqual(self.resets,1);self.assertEqual(self.authorizations,1);self.assertEqual(receipt['before']['unit']['ActiveState'],'failed');self.assertEqual(receipt['after']['unit']['ActiveState'],'inactive');self.assertTrue(receipt['strictStoppedVerified'])
  with self.assertRaisesRegex(q.Rejected,'normalization_denied'):self.normalize()
  self.assertEqual(self.resets,1)
 def test_every_unexpected_unit_terminal_field_is_rejected_before_reset(self):
  for key,value in dict(MainPID='123',ActiveState='active',SubState='dead',Result='timeout',ExecMainCode='1',ExecMainStatus='15',UnitFileState='disabled',Job='17',ControlGroup='/other').items():
   with self.subTest(key=key):
    original=self.state[key];self.state[key]=value
    with self.assertRaisesRegex(q.Rejected,'normalization_denied'):self.normalize()
    self.state[key]=original
  self.assertEqual(self.resets,0);self.assertEqual(self.authorizations,0)
 def test_every_missing_retirement_fence_is_rejected(self):
  for key in ('residentPid','residentStartTicks','residentUid','controlGroup','unitHadOnlyResident','frozenCleanVerified','pidfdRetirement','retiredWithoutPathCleanup','visibleLockProtected'):
   with self.subTest(key=key):
    value=self.proof.pop(key)
    with self.assertRaisesRegex(q.Rejected,'normalization_denied'):self.normalize()
    self.proof[key]=value
  for key in ('originalPid','originalStartTicks','originalIdentityPresent','stopSubmitted','pidfdKillSubmitted'):
   value=self.proof['postRetirementFacts'].pop(key)
   with self.assertRaisesRegex(q.Rejected,'normalization_denied'):self.normalize()
   self.proof['postRetirementFacts'][key]=value
  self.assertEqual(self.resets,0)
 def test_live_process_nonempty_group_or_pending_job_remains_rejected(self):
  for changes in (dict(cgroup_members=lambda _:[self.pid]),dict(jobs=lambda:[dict(id=1,type='stop',state='running')])):
   with self.assertRaisesRegex(q.Rejected,'normalization_denied'):self.normalize(**changes)
  child=subprocess.Popen(['/bin/sleep','30'])
  try:
   with self.assertRaisesRegex(q.Rejected,'normalization_denied'):q.normalize_intentional_stop(child.pid,q.process(child.pid)['start'],self.group,self.proof,authorize=self.authorize,service=lambda:self.state,cgroup_members=lambda _:[],jobs=lambda:[],reset=self.reset)
  finally:child.terminate();child.wait()
  self.assertEqual(self.resets,0)
 def test_authority_unit_hash_lock_or_image_failure_prevents_reset(self):
  for code in ('parent_boot_unit_changed','parent_visible_path_changed','parent_image_changed_before_normalization','publication_runtime_data_present'):
   def denied():raise q.Rejected(code)
   with self.assertRaisesRegex(q.Rejected,code):self.normalize(authorize=denied)
  with self.assertRaisesRegex(q.Rejected,'authority_changed'):self.normalize(authorize=lambda:False)
  self.assertEqual(self.resets,0)
 def test_fresh_state_race_and_uncertain_reset_never_resubmit(self):
  def race():self.state['Job']='19'
  with self.assertRaisesRegex(q.Rejected,'normalization_denied'):self.normalize(authorize=race)
  self.assertEqual(self.resets,0);self.state['Job']=''
  def uncertain():self.resets+=1;raise subprocess.TimeoutExpired(['secret'],3,output='secret')
  with self.assertRaisesRegex(q.Rejected,'normalization_failed') as caught:self.normalize(reset=uncertain)
  self.assertEqual(self.resets,1);self.assertTrue(caught.exception.facts['intentionalStopNormalization']['resetSubmitted']);self.assertNotIn('secret',json.dumps(caught.exception.facts))
 def test_reset_does_not_waive_enabled_or_empty_group_afterward(self):
  def disabled():self.reset();self.state['UnitFileState']='disabled'
  with self.assertRaisesRegex(q.Rejected,'parent_stop_job_unconfirmed'):self.normalize(reset=disabled)
  self.assertEqual(self.resets,1)
  self.state.update(ActiveState='failed',SubState='failed',Result='signal',UnitFileState='enabled');self.resets=0
  with self.assertRaisesRegex(q.Rejected,'parent_stop_job_unconfirmed'):self.normalize(cgroup_members=lambda _:[self.pid] if self.resets else [])
  self.assertEqual(self.resets,1)

if __name__=='__main__':unittest.main()
