#!/usr/bin/env python3
"""Actual idle RPC/process exit/lock cleanup; no provider or model effects."""
import importlib.util,json,os,socket,fcntl,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def load(name,path):
 spec=importlib.util.spec_from_file_location(name,path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module
legacy=load('supervisor_fixture',Path(__file__).with_name('runtime-supervisor.py'))
hygiene=load('hygiene',ROOT/'base-supervisor-hygiene.py')
class StoppedService(unittest.TestCase):
 def test_successful_systemd_sigterm_from_actual_provider_probe_is_a_clean_stop(self):
  observed={'MainPID':'0','Result':'success','ExecMainCode':'2','ExecMainStatus':'15','ActiveState':'inactive','SubState':'dead','UnitFileState':'enabled'}
  hygiene.verify_stopped_service(observed,'enabled')
 def test_failed_killed_restarted_or_changed_service_is_not_a_clean_stop(self):
  observed={'MainPID':'0','Result':'success','ExecMainCode':'2','ExecMainStatus':'15','ActiveState':'inactive','UnitFileState':'enabled'}
  for change in ({'MainPID':'42'},{'Result':'signal'},{'ExecMainStatus':'9'},{'ExecMainCode':'3'},{'ActiveState':'active'},{'UnitFileState':'disabled'}):
   with self.subTest(change=change),self.assertRaises(hygiene.Rejected):
    hygiene.verify_stopped_service(dict(observed,**change),'enabled')
 def test_normal_exit_and_explicit_disabled_owner_remain_supported(self):
  observed={'MainPID':'0','Result':'success','ExecMainCode':'1','ExecMainStatus':'0','ActiveState':'inactive','UnitFileState':'enabled'}
  hygiene.verify_stopped_service(observed,'enabled')
  hygiene.verify_stopped_service(dict(observed,UnitFileState='disabled'),'disabled')

class Publication(unittest.TestCase):
 setUp=legacy.RuntimeSupervisorTest.setUp
 start=legacy.RuntimeSupervisorTest.start
 stop=legacy.RuntimeSupervisorTest.stop
 request=legacy.RuntimeSupervisorTest.request
 child=legacy.RuntimeSupervisorTest.child
 def clean_checkout(self):(self.checkout/'dirty').unlink(missing_ok=True)
 def stop_request(self):
  ping=legacy.helper.rpc(self.data,{'version':1,'action':'ping'})
  return dict(version=1,action='publication-stop',operationId='original-publication',expectedBootNonce=ping['bootNonce'])
 def cleanup(self,pid,ticks):return hygiene.clean_stopped(self.data,os.getuid(),pid,ticks,self.checkout,self.status,self.recovery)
 def test_idle_rpc_exits_releases_lock_and_only_ephemeral_paths_are_removed(self):
  self.clean_checkout();before=self.stop_request();pid=self.server.pid;ticks=hygiene.process_fact(pid)[1]
  boot=self.root/'boot-service';boot.write_bytes(b'enabled boot service, unchanged\n');binary=self.binary.read_bytes()
  receipt=legacy.helper.rpc(self.data,before);self.assertEqual(receipt['state'],'idle-stopping');self.server.wait(timeout=4);self.stop()
  cleaned=self.cleanup(pid,ticks);self.assertTrue(cleaned['supervisorLockReleased']);self.assertEqual(cleaned['removed'],['supervisor.json','supervisor.lock']);self.assertEqual(list(self.data.iterdir()),[])
  self.assertEqual(self.binary.read_bytes(),binary);self.assertEqual(boot.read_bytes(),b'enabled boot service, unchanged\n')
  # A fresh boot starts a new unbound resident instead of inheriting a PID/socket.
  self.start();new=legacy.helper.rpc(self.data,{'version':1,'action':'ping'});self.assertNotEqual(new['bootNonce'],before['expectedBootNonce']);self.assertFalse(new['bound']);self.assertFalse(new['childAlive'])
 def test_bound_live_child_refuses_publication_stop_and_preserves_work(self):
  legacy.helper.rpc(self.data,self.request());child=self.child();request=self.stop_request()
  with self.assertRaisesRegex(legacy.helper.Rejected,'publication_stop_runtime_bound'):legacy.helper.rpc(self.data,request)
  self.assertIsNone(self.server.poll());os.kill(child['pid'],0);self.assertEqual((self.checkout/'dirty').read_text(),'retained uncommitted work')
 def test_nonce_and_extra_fields_are_rejected_without_stopping(self):
  self.clean_checkout();request=self.stop_request();request['expectedBootNonce']='wrong'
  with self.assertRaisesRegex(legacy.helper.Rejected,'publication_stop_identity_mismatch'):legacy.helper.rpc(self.data,request)
  request=self.stop_request();request['workspaceId']='must-not-be-a-lifecycle-action'
  with self.assertRaisesRegex(legacy.helper.Rejected,'publication_stop_frame_invalid'):legacy.helper.rpc(self.data,request)
  self.assertIsNone(self.server.poll())
 def test_repair_and_recovery_history_refuse_even_without_a_running_child(self):
  self.clean_checkout()
  for name,value in [('cloud-runtime-supervisor-repairs.json',{'old':{'phase':'ready'}}),('cloud-runtime-supervisor-recovery.json',{'workspaceId':'old'})]:
   with self.subTest(name=name):
    self.stop();marker=self.data/name;marker.write_text(json.dumps(value));marker.chmod(0o600);self.start()
    with self.assertRaisesRegex(legacy.helper.Rejected,'publication_stop_runtime_bound'):legacy.helper.rpc(self.data,self.stop_request())
    self.assertIsNone(self.server.poll());self.stop();marker.unlink();self.start()
 def test_cleaner_refuses_live_pid_and_wrong_owner_without_deleting(self):
  self.clean_checkout();pid=self.server.pid;ticks=hygiene.process_fact(pid)[1]
  with self.assertRaisesRegex(hygiene.Rejected,'publication_resident_still_alive'):self.cleanup(pid,ticks)
  with self.assertRaisesRegex(hygiene.Rejected,'publication_directory_owner_invalid'):hygiene.clean_stopped(self.data,os.getuid()+1,pid,ticks,self.checkout,self.status,self.recovery)
  self.assertTrue((self.data/'.runtime-supervision/supervisor.json').exists());self.assertIsNone(self.server.poll())
 def test_symlink_hardlink_unknown_leaf_and_held_lock_are_never_removed(self):
  self.clean_checkout();pid=self.server.pid;ticks=hygiene.process_fact(pid)[1];self.stop();ipc=self.data/'.runtime-supervision';record=ipc/'supervisor.json';body=record.read_bytes();outside=self.root/'outside';outside.write_bytes(body);outside.chmod(0o600)
  record.unlink();record.symlink_to(outside)
  with self.assertRaisesRegex(hygiene.Rejected,'publication_ipc_owner_invalid'):self.cleanup(pid,ticks)
  self.assertTrue(record.is_symlink());record.unlink();record.write_bytes(body);record.chmod(0o600)
  alias=self.root/'lock-alias';os.link(ipc/'supervisor.lock',alias)
  with self.assertRaisesRegex(hygiene.Rejected,'publication_ipc_owner_invalid'):self.cleanup(pid,ticks)
  alias.unlink();unknown=ipc/'unrecognized';unknown.write_text('preserve')
  with self.assertRaisesRegex(hygiene.Rejected,'publication_unknown_ipc'):self.cleanup(pid,ticks)
  self.assertEqual(unknown.read_text(),'preserve');unknown.unlink()
  with (ipc/'supervisor.lock').open('rb') as lock:
   fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
   with self.assertRaisesRegex(hygiene.Rejected,'publication_lock_still_held'):self.cleanup(pid,ticks)
  self.assertTrue(record.exists());self.assertEqual(outside.read_bytes(),body)
if __name__=='__main__':unittest.main()
