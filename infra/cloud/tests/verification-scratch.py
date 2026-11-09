#!/usr/bin/env python3
import fcntl,importlib.util,json,os,tempfile,unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
SOURCE=Path(__file__).resolve().parents[1]/'single-host/verification-scratch.py'
spec=importlib.util.spec_from_file_location('scratch',SOURCE);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class Scratch(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=Path(self.tmp.name);self.stage=self.root/'staging';self.stage.mkdir(mode=0o700);self.mountroot=self.stage/'verification-scratch-fs';self.mountroot.mkdir(mode=0o700);self.path=self.mountroot/'private';self.path.mkdir(mode=0o700);self.backing=self.stage/'verification-scratch.img'
  with self.backing.open('wb') as f:f.truncate(768*1024*1024)
  self.backing.chmod(0o600);self.settings={'VERIFICATION_SCRATCH_DIR':str(self.path),'VERIFICATION_SCRATCH_LIMIT_BYTES':str(768*1024*1024)};self.free=512*1024*1024;self.total=750*1024*1024;self.options='rw,nodev,nosuid,noexec';self.source='/dev/loop12';self.active=False;self.commands=[]
  original=m.private
  self.private=patch.object(m,'private',lambda path,uid,kind:original(path,os.getuid(),kind));self.private.start();self.addCleanup(self.private.stop)
 def host_run(self,args,**kwargs):
  self.commands.append(args)
  if args[0]=='findmnt':value={'filesystems':[{'target':str(self.mountroot),'source':self.source,'fstype':'ext4','options':self.options}]}
  elif args[0]=='losetup':value={'loopdevices':[{'name':self.source,'back-file':str(self.backing)}]}
  elif args[:2]==['docker','ps']:return SimpleNamespace(returncode=0,stdout='active-container\n' if self.active else '')
  else:self.fail('unexpected mutating or provider command')
  return SimpleNamespace(returncode=0,stdout=json.dumps(value))
 def inspect(self,**kwargs):return m.inspect(self.root,'staging',self.settings,run=self.host_run,mountpoint=lambda _:True,space=lambda _:SimpleNamespace(f_blocks=self.total,f_frsize=1,f_bavail=self.free),**kwargs)
 def test_disabled_has_no_mount_or_provider_queries(self):
  self.assertIsNone(m.inspect(self.root,'production',{},run=lambda *_:self.fail('must not query')))
 def test_private_fixed_size_ext4_loop_has_bounded_capacity(self):
  proof=self.inspect();self.assertTrue(proof['hardQuota']);self.assertEqual(proof['limitBytes'],768*1024*1024);self.assertEqual(proof['freeBytes'],self.free)
 def test_production_custom_path_unmounted_or_insecure_options_fail(self):
  with self.assertRaisesRegex(ValueError,'staging_only'):m.inspect(self.root,'production',self.settings)
  wrong=dict(self.settings,VERIFICATION_SCRATCH_DIR='/tmp')
  with self.assertRaisesRegex(ValueError,'configuration_invalid'):m.inspect(self.root,'staging',wrong)
  with self.assertRaisesRegex(ValueError,'not_mounted'):m.inspect(self.root,'staging',self.settings,mountpoint=lambda _:False)
  self.options='rw'
  with self.assertRaisesRegex(ValueError,'mount_invalid'):self.inspect()
 def test_low_free_or_unbounded_filesystem_fails_before_deploy(self):
  self.free=16*1024*1024
  with self.assertRaisesRegex(ValueError,'capacity_insufficient'):self.inspect()
  self.assertTrue(self.inspect(require_free=False)['hardQuota'])
  self.free=512*1024*1024;self.total=1024*1024*1024
  with self.assertRaisesRegex(ValueError,'capacity_insufficient'):self.inspect()
 def test_wrong_backing_loop_or_size_and_symlink_fail(self):
  self.source='/dev/sda'
  with self.assertRaisesRegex(ValueError,'mount_invalid'):self.inspect()
  self.source='/dev/loop12';self.backing.write_bytes(b'x')
  with self.assertRaisesRegex(ValueError,'backing_size_invalid'):self.inspect()
 def fixture_tree(self):
  base=self.path/'zuse-cloud-effects';base.mkdir(mode=0o700);verification=base/'verification';verification.mkdir(mode=0o700);locks=base/'locks';locks.mkdir(mode=0o700);workspace='a'*64
  entry=verification/workspace;entry.mkdir(mode=0o700);lockdir=locks/workspace;lockdir.mkdir(mode=0o700);lock=lockdir/'.workspace-runtime.lock';lock.touch(mode=0o600);temporary=entry/'.tmpABCdef12';temporary.mkdir(mode=0o700);(temporary/'private-plaintext').write_bytes(b'fixture');return temporary,lock
 def cleanup(self):
  proof=self.inspect(require_free=False)
  with patch.object(m,'inspect',return_value=proof):return m.cleanup(self.root,self.settings,run=self.host_run)
 def test_cleanup_requires_stopped_services_and_preserves_original_lock_namespace(self):
  temporary,lock=self.fixture_tree();self.active=True
  with self.assertRaisesRegex(ValueError,'services_running'):self.cleanup()
  self.assertTrue(temporary.exists());self.active=False
  # Allowed archive symlinks are unlinked, never followed into another tree.
  outside=self.root/'outside';outside.write_text('untouched');(temporary/'link').symlink_to(outside)
  result=self.cleanup();self.assertEqual(result['temporaryDirectoriesRemoved'],1);self.assertFalse(temporary.exists());self.assertTrue(lock.exists());self.assertEqual(outside.read_text(),'untouched')
 def test_cleanup_cannot_touch_current_lock_winner(self):
  temporary,lock=self.fixture_tree()
  with lock.open('r+') as held:
   fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB)
   with self.assertRaisesRegex(ValueError,'workspace_busy'):self.cleanup()
  self.assertTrue(temporary.exists())
 def test_unknown_temp_directory_is_preserved_for_inspection(self):
  temporary,lock=self.fixture_tree();temporary.rename(temporary.with_name('unknown'))
  with self.assertRaisesRegex(ValueError,'unknown_temporary'):self.cleanup()
  self.assertTrue(temporary.with_name('unknown').exists())
 def test_staging_boot_service_mounts_existing_filesystem_before_only_app_pair(self):
  service_spec=importlib.util.spec_from_file_location('scratch_boot',SOURCE.with_name('verification-scratch-service.py'));service=importlib.util.module_from_spec(service_spec);service_spec.loader.exec_module(service)
  saved=self.path.with_name('private-saved');self.path.rename(saved);calls=[]
  settings=dict(self.settings,ZUSE_ENV='staging')
  deploy=SimpleNamespace(read_env=lambda _:settings,compose=lambda *_:(['docker','compose','--project-name','zuse-staging'],{}))
  scratch=SimpleNamespace(private=m.private,require=m.require,MIN_LIMIT=m.MIN_LIMIT,MAX_LIMIT=m.MAX_LIMIT,inspect=lambda *_:calls.append('inspect'))
  def run(args,**kwargs):
   calls.append(args)
   if args[0]=='blkid':return SimpleNamespace(returncode=0,stdout='ext4\n')
   if args[0]=='mount':saved.rename(self.path)
   if args[0]=='docker':self.assertEqual(kwargs['env']['DOCKER_HOST'],'unix:///var/run/docker.sock')
   return SimpleNamespace(returncode=0,stdout='')
  with patch.object(service.os,'geteuid',return_value=0),patch.object(service,'load',side_effect=lambda name,_:deploy if name=='scratch_deploy' else scratch):result=service.start(self.root,run=run,mountpoint=lambda _:False)
  self.assertEqual(calls[0][0],'blkid');self.assertEqual(calls[1][0],'mount');self.assertEqual(calls[2],'inspect');self.assertEqual(calls[3][-2:],['api','worker']);self.assertNotIn('auth-worker',calls[3]);self.assertTrue(result['backingFormatted']==False)
 def test_unmounted_lower_child_refuses_boot_instead_of_binding_rootfs(self):
  service_spec=importlib.util.spec_from_file_location('scratch_boot_negative',SOURCE.with_name('verification-scratch-service.py'));service=importlib.util.module_from_spec(service_spec);service_spec.loader.exec_module(service)
  deploy=SimpleNamespace(read_env=lambda _:dict(self.settings,ZUSE_ENV='staging'))
  with patch.object(service.os,'geteuid',return_value=0),patch.object(service,'load',side_effect=lambda name,_:deploy if name=='scratch_deploy' else m):
   with self.assertRaisesRegex(ValueError,'scratch_unmounted_child_present'):service.start(self.root,run=lambda *_:self.fail('must not mount or start services'),mountpoint=lambda _:False)
 def test_overlay_only_changes_two_staging_services_and_same_mount(self):
  text=SOURCE.with_suffix('.yaml').read_text();self.assertEqual(text.count('TMPDIR: /var/lib/zuse/verification-scratch'),2);self.assertEqual(text.count('source: ${VERIFICATION_SCRATCH_DIR:?}'),2);self.assertNotIn('auth-worker:',text);self.assertNotIn('production:',text);self.assertEqual(text.count('create_host_path: false'),2)
if __name__=='__main__':unittest.main()
