#!/usr/bin/env python3
"""Local process/file controls; no provider, credentials, native model or network."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import types
import unittest

SOURCE=Path(__file__).resolve().parents[1]/'managed-base-preparation.py'
spec=importlib.util.spec_from_file_location('preparation',SOURCE)
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)

class Preparation(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();self.paths=m.Paths(Path(self.temp.name));self.uid=os.getuid()
  for p in (self.paths.bundle,self.paths.data,self.paths.manifest.parent,self.paths.runtime.parent,self.paths.supervisor.parent):p.mkdir(parents=True,exist_ok=True)
  self.paths.data.chmod(0o700)
  self.paths.runtime.write_bytes(b'actual-runtime-fixture');self.paths.runtime.chmod(0o555)
  self.paths.supervisor.write_bytes(b'actual-supervisor-fixture');self.paths.supervisor.chmod(0o555)
  self.expected=dict(bundleSha256='a'*64,runtimeSha256=m.file_hash(self.paths.runtime),supervisorSha256=m.file_hash(self.paths.supervisor),harnessRelease='b'*64,providerId='e2b')
  m.atomic(self.paths.layout,dict(version=1,status='layout-only',**self.expected))
  self.process=None;self.spawns=0;self.run_count=0
  self.sibling=subprocess.Popen(['/bin/sleep','30'],start_new_session=True)
 def tearDown(self):
  if self.process and self.process.poll() is None:self.process.kill();self.process.wait()
  self.sibling.terminate();self.sibling.wait();self.temp.cleanup()
 def spawn(self,args,**kw):
  self.spawns+=1;self.assertEqual(args[-1],'serve')
  self.assertEqual(set(kw['env']),{'PATH','HOME','ZUSE_RUNTIME_PROVIDER'})
  self.process=subprocess.Popen(['/bin/sleep','30'],start_new_session=True)
  return self.process
 def ping(self,_):return dict(version=1,residentPid=self.process.pid,bound=False,childAlive=False,recoveryAlive=False)
 def processes(self,_):return [self.process.pid] if self.process and self.process.poll() is None else []
 def run_installer(self,args,**kw):
  self.run_count+=1;self.assertEqual(args[-1],'prepare')
  self.assertEqual(kw['env']['ZUSE_RUNTIME_PROVIDER'],self.expected['providerId'])
  self.paths.bundle.rmdir()
  self.paths.manifest.write_text(json.dumps(dict(managedBase=True,credentialsRemoved=True,repositories=[],rustRuntimeSupervisorVersion=1,rustCommandAdmissionVersion=2,commandAdmission={'runtimeSha256':self.expected['runtimeSha256']},rustGithubCliVersion=1,githubCli={'loginShellResolvedManaged':True})))
  self.paths.manifest.chmod(0o444)
  return types.SimpleNamespace(returncode=0)
 def prepare(self,**extra):
  return m.prepare(self.paths,self.expected,owner=self.uid,uid=self.uid,ping_fn=self.ping,processes=self.processes,spawn=self.spawn,run=extra.pop('run',self.run_installer),**extra)
 def fresh_resident(self):
  self.process=subprocess.Popen(['/bin/sleep','30'],start_new_session=True)
 def ready(self):return m.ready(self.paths,self.expected,owner=self.uid,uid=self.uid,ping_fn=self.ping)
 def test_exact_owned_temporary_process_is_reaped_sibling_survives(self):
  self.prepare();self.assertIsNotNone(self.process.poll());self.assertIsNone(self.sibling.poll())
  receipt=m.load(self.paths.receipt,self.uid);self.assertEqual(receipt['status'],'prepared-uncertified')
  self.assertEqual(self.run_count,1)
  with self.assertRaisesRegex(ValueError,'already_attempted'):self.prepare()
  self.assertEqual(self.spawns,1)
  self.fresh_resident();self.assertEqual(self.ready()['status'],'resident-ready')
 def test_preparation_failure_never_ready_and_never_retries(self):
  self.prepare_failure=lambda *a,**k:types.SimpleNamespace(returncode=1)
  with self.assertRaisesRegex(ValueError,'image_preparation_failed'):self.prepare(run=self.prepare_failure)
  self.assertIsNotNone(self.process.poll());self.assertIsNone(self.sibling.poll())
  self.assertEqual(m.load(self.paths.receipt,self.uid)['status'],'preparing')
  with self.assertRaisesRegex(ValueError,'incomplete'):self.ready()
  with self.assertRaisesRegex(ValueError,'already_attempted'):self.prepare()
 def test_foreign_process_blocks_before_any_spawn(self):
  with self.assertRaisesRegex(ValueError,'foreign_process'):
   m.prepare(self.paths,self.expected,owner=self.uid,uid=self.uid,processes=lambda _:[self.sibling.pid],spawn=self.spawn)
  self.assertEqual(self.spawns,0);self.assertIsNone(self.sibling.poll())
 def test_changed_binding_and_symlinks_reject_before_spawn(self):
  self.expected['bundleSha256']='c'*64
  with self.assertRaisesRegex(ValueError,'binding_mismatch'):self.prepare()
  self.assertEqual(self.spawns,0)
  self.expected['bundleSha256']='a'*64
  original=self.paths.runtime.with_suffix('.saved');self.paths.runtime.rename(original);self.paths.runtime.symlink_to(original)
  with self.assertRaisesRegex(ValueError,'file_unsafe'):self.prepare()
  self.assertEqual(self.spawns,0)
 def test_restore_can_unpack_new_inodes_but_must_keep_exact_bytes(self):
  self.prepare();self.fresh_resident()
  old=self.paths.runtime.with_suffix('.old');self.paths.runtime.rename(old)
  self.paths.runtime.write_bytes(old.read_bytes());self.paths.runtime.chmod(0o555)
  self.assertEqual(self.ready()['status'],'resident-ready')
  self.paths.runtime.chmod(0o755);self.paths.runtime.write_bytes(b'changed')
  with self.assertRaisesRegex(ValueError,'artifact_changed'):self.ready()
 def test_busy_or_task_bound_supervisor_and_dirty_data_fail_readonly(self):
  self.prepare();self.fresh_resident()
  good=self.ping(self.paths)
  for field in ('bound','childAlive','recoveryAlive'):
   with self.assertRaisesRegex(ValueError,'not_idle'):
    m.ready(self.paths,self.expected,owner=self.uid,uid=self.uid,ping_fn=lambda _,field=field:{**good,field:True})
  task=self.paths.data/'commands.sqlite';task.write_bytes(b'preserve')
  with self.assertRaisesRegex(ValueError,'contains_task'):self.ready()
  self.assertEqual(task.read_bytes(),b'preserve')
 def test_daytona_uses_same_proofs_without_an_e2b_environment(self):
  self.expected['providerId']='daytona';m.atomic(self.paths.layout,dict(version=1,status='layout-only',**self.expected))
  self.prepare();self.fresh_resident();self.assertEqual(self.ready()['status'],'resident-ready')
 def test_invalid_installer_stage_fails_before_provider_or_root_effects(self):
  p=subprocess.run(['/bin/bash',str(SOURCE.with_name('managed-base-install.sh')),'/not-a-builder','invalid'],capture_output=True,text=True)
  self.assertEqual(p.returncode,2);self.assertIn('invalid_base_install_stage',p.stderr)

if __name__=='__main__':unittest.main()
