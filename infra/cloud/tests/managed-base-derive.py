import hashlib,importlib.util,json,os,shutil,sys,tempfile,unittest,subprocess,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[3]
def module(name,path):
 s=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
D=module('derive',ROOT/'infra/cloud/managed-base-derive.py');M=module('manager',ROOT/'infra/cloud/harness-release.py')
def fixture(root):
 image=root/'image';inp=root/'input';inp.mkdir();owner=os.getuid()
 files=[]
 def put(name,body):
  body=body if isinstance(body,bytes) else body.encode();p=inp/name;p.write_bytes(body);files.append(dict(path=name,bytes=len(body),sha256=hashlib.sha256(body).hexdigest()));return p
 for name in ('runtime','claude','codex','grok','launch-runtime.sh','launch-recovery.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py','prepare-plan.json'):put(name,'synthetic '+name)
 put('harness-release.py',(ROOT/'infra/cloud/harness-release.py').read_bytes());put('managed-base-install.sh',(ROOT/'infra/cloud/managed-base-install.sh').read_bytes())
 assets={};assetroot=root/'assets';assetroot.mkdir()
 for name in sorted(M.CODEX_REQUIRED):
  p=assetroot/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(('synthetic '+name).encode());executable=name.endswith(('host','bwrap','/rg'));p.chmod(0o555 if executable else 0o444);assets[name]=dict(bytes=p.stat().st_size,sha256=D.digest(p),executable=executable)
 manifest={'schema':2,'platform':'linux-x86_64','harnesses':{name:{'version':'1.2.3','sha256':D.digest(inp/name)} for name in ('claude','codex','grok')}};manifest['harnesses']['codex'].update(assets=assets,executable='codex-package/bin/codex')
 release=hashlib.sha256(M.canonical(manifest)).hexdigest();put('harness-manifest.json',json.dumps(manifest))
 archive=root/'assets.tar';M.archive_assets(assetroot,assets,archive);put('codex-assets.tar',archive.read_bytes())
 parent={'schema':1,'status':'candidate-only','releaseId':'zuse-base-'+'a'*32,'harnessRelease':release,'runtimeSha256':files[0]['sha256'],'files':list(files)};parenttext=json.dumps(parent)
 oldmanifest={'managedBase':True,'credentialsRemoved':True,'repositories':[]}
 artifact='zuse-base-build-'+'b'*24
 builder={'provider':'boat','status':'prepared-uncertified','runId':artifact,'artifactId':artifact,'snapshotName':artifact,'machineId':'original-builder','cleanupVerified':True,'artifactRetained':True,'runtimeSha256':parent['runtimeSha256'],'harnessRelease':release,'manifest':oldmanifest,'bundleSha256':hashlib.sha256(parenttext.encode()).hexdigest(),'providerArtifactVersion':'immutable-snapshot-id'}
 proof={'status':'mechanism-verified-uncertified','cleanupVerified':True,'artifactRetained':True,'sourceMachineAbsent':True,'agentAcceptanceVerified':False,'artifactId':artifact,'snapshotName':artifact,'runtimeSha256':parent['runtimeSha256'],'restoreMachineId':'original-restore'}
 cleanup={'readOnly':True,'allMachinesAbsent':True,'previousOperations':[{'kind':'sandboxes','id':identity,'httpStatus':404,'absent':True} for identity in ('original-builder','original-restore')]+[{'kind':'saved-snapshots','id':artifact,'httpStatus':200,'absent':False}]}
 docs={k:json.dumps(v) for k,v in [('proof',proof),('builder',builder),('cleanup',cleanup)]};docs['parentBundle']=parenttext
 lineage={'schema':1,'provider':'boat','documents':docs,'digests':{k:hashlib.sha256(v.encode()).hexdigest() for k,v in docs.items()},'parentBundleFileSha256':builder['bundleSha256']}
 target=dict(parent);target['files']=list(files);target['managedBaseDerivationVersion']=1
 (inp/'runtime').write_bytes(b'new guest');target['runtimeSha256']=D.digest(inp/'runtime');target['files'][0]=dict(path='runtime',bytes=9,sha256=target['runtimeSha256'])
 p=put('managed-base-derive.py',(ROOT/'infra/cloud/managed-base-derive.py').read_bytes());target['files'].append(files[-1])
 (inp/'bundle.json').write_text(json.dumps(target));(inp/'derivation.json').write_text(json.dumps(lineage));expected=D.digest(inp/'derivation.json')
 # Restore the ORIGINAL native/tool bytes and complete package under the actual
 # production manager's immutable release layout, never a fake validation shim.
 installed=image/'opt/zuse/harnesses/releases'/release;installed.mkdir(parents=True)
 for name in ('claude','codex','grok'):shutil.copyfile(inp/name,installed/name);(installed/name).chmod(0o555)
 (installed/'manifest.json').write_text(json.dumps(manifest));M.install_assets(archive,installed,assets);M.complete_codex(installed,manifest)
 for name,path in D.INSTALLED.items():
  p=image/path.lstrip('/');p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(('synthetic '+name).encode() if name!='harness-release.py' else (ROOT/'infra/cloud/harness-release.py').read_bytes())
 p=image/'opt/zuse/preparation/status/manifest.json';p.parent.mkdir(parents=True);p.write_text(json.dumps(oldmanifest))
 (image/'home/zuse/workspace').mkdir(parents=True);(image/'home/zuse/.zuse-data/.runtime-supervision').mkdir(parents=True)
 for name,value in docs.items():(root/(name+'.json')).write_text(value)
 return dict(image=image,input=inp,expected=expected,owner=owner,release=installed,lineage=lineage,target=target)

class Derivation(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory(dir=ROOT/'.context',prefix='derive-test-');self.f=fixture(Path(self.tmp.name))
 def tearDown(self):self.tmp.cleanup()
 def verify(self,**kw):return D.verify(self.f['input'],self.f['expected'],filesystem=self.f['image'],owner=self.f['owner'],ping=kw.get('ping',dict(version=1,bound=False,childAlive=False,recoveryAlive=False)))
 def test_real_complete_distribution_and_clean_parent_pass(self):
  r=self.verify();self.assertTrue(r['completeCodexAssetsVerified']);self.assertNotEqual(r['parentRuntimeSha256'],r['runtimeSha256'])
 def test_factory_image_proof_is_distinct_and_does_not_waive_task_or_native_checks(self):
  args=dict(filesystem=self.f['image'],owner=self.f['owner'],quiescence_image=True)
  r=D.verify_parent(self.f['input'],self.f['expected'],**args)
  self.assertEqual(r['status'],'quiescence-image-verified');self.assertNotEqual(r['status'],'passed');self.assertTrue(r['completeCodexAssetsVerified'])
  identity=self.f['image']/'home/zuse/.zuse-data/cloud-runtime-identity.json';identity.write_text('bound task')
  with self.assertRaises(ValueError):D.verify_parent(self.f['input'],self.f['expected'],**args)
  identity.unlink();asset=self.f['release']/'codex-package/codex-path/rg';asset.chmod(0o755);asset.write_bytes(b'changed native asset')
  with self.assertRaises(ValueError):D.verify_parent(self.f['input'],self.f['expected'],**args)
 def test_tampered_companion_and_symlink_rejected(self):
  p=self.f['release']/'codex-package/codex-path/rg';p.chmod(0o755);p.write_bytes(b'tampered')
  with self.assertRaises(ValueError):self.verify()
  p.unlink();p.symlink_to('/bin/true')
  with self.assertRaises(ValueError):self.verify()
 def test_small_parent_preflight_needs_no_new_runtime_payload(self):
  (self.f['input']/'runtime').unlink()
  r=D.verify_parent(self.f['input'],self.f['expected'],filesystem=self.f['image'],owner=self.f['owner'],ping=dict(version=1,bound=False,childAlive=False,recoveryAlive=False),bundle_sha256=D.digest(self.f['input']/'bundle.json'))
  self.assertEqual(r['status'],'passed');self.assertTrue(r['completeCodexAssetsVerified'])
  with self.assertRaises((OSError,ValueError)):self.verify()
 def test_small_preflight_collects_multiple_violations_and_pins_metadata(self):
  for name in ('opt','usr/local'):(self.f['image']/name).chmod(0o777)
  r=D.verify_parent(self.f['input'],self.f['expected'],filesystem=self.f['image'],owner=self.f['owner'],ping=dict(version=1,bound=False,childAlive=False,recoveryAlive=False))
  self.assertEqual(r['status'],'rejected');self.assertTrue({'/opt','/usr/local'}.issubset({f['path'] for f in r['failures']}))
  with self.assertRaises(ValueError):D.verify_parent(self.f['input'],self.f['expected'],bundle_sha256='0'*64,filesystem=self.f['image'],owner=self.f['owner'])
 def test_writable_ancestor_reports_exact_numeric_facts(self):
  parent=Path(self.tmp.name)/'provider-writable';parent.mkdir(mode=0o777);parent.chmod(0o777);child=parent/'root-input';child.mkdir(mode=0o700)
  with self.assertRaises(D.PathRejected) as failure:D.secure(child,os.getuid())
  self.assertEqual(failure.exception.facts['path'],str(parent));self.assertEqual(failure.exception.facts['mode'],'0o777');self.assertFalse(failure.exception.facts['symlink'])
 def test_missing_companion_and_home_symlink_rejected(self):
  p=self.f['release']/'codex-package/bin/codex-code-mode-host';body=p.read_bytes();p.unlink()
  with self.assertRaises(ValueError):self.verify()
  p.write_bytes(body);p.chmod(0o555)
  home=self.f['image']/'home/zuse';saved=home.with_name('saved-zuse');home.rename(saved);home.symlink_to(saved)
  with self.assertRaises(ValueError):self.verify()
 def test_task_data_credentials_checkout_and_live_child_rejected(self):
  for path in ('home/zuse/.zuse-data/cloud-runtime-identity.json','home/zuse/.codex/auth.json','home/zuse/workspace/user-file'):
   p=self.f['image']/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_text('synthetic')
   with self.assertRaises(ValueError):self.verify()
   p.unlink()
  with self.assertRaises(ValueError):self.verify(ping=dict(version=1,bound=True,childAlive=True,recoveryAlive=False))
 def test_lineage_or_harness_change_rejected(self):
  for change in ('digest','harness','cleanup','taskmanifest'):
   lineage=json.loads(json.dumps(self.f['lineage']));bundle=json.loads(json.dumps(self.f['target']))
   if change=='digest':lineage['digests']['proof']='0'*64
   elif change=='harness':bundle['harnessRelease']='0'*64
   else:
    key='cleanup' if change=='cleanup' else 'builder';doc=json.loads(lineage['documents'][key])
    if change=='cleanup':doc['previousOperations'][0]['httpStatus']=200
    else:doc['manifest']['repositories']=['synthetic-repo']
    text=json.dumps(doc);lineage['documents'][key]=text;lineage['digests'][key]=hashlib.sha256(text.encode()).hexdigest()
   with self.assertRaises(ValueError):D.parent_binding(lineage,bundle)
 def test_parent_runtime_and_overlay_tampering_rejected(self):
  p=self.f['image']/'usr/local/bin/zuse-cloud-runtime';p.write_text('different runtime')
  with self.assertRaises(ValueError):self.verify()
  p.write_text('synthetic runtime');(self.f['input']/'runtime').write_text('modified target')
  with self.assertRaises(ValueError):self.verify()

 def stopped_fixture(self):
  f=self.f;image=f['image'];data=image/'home/zuse/.zuse-data';data.chmod(0o700)
  ipc=data/'.runtime-supervision';ipc.chmod(0o700);lock=ipc/'supervisor.lock';lock.touch(mode=0o600)
  child=subprocess.Popen(['/bin/sleep','30'])
  helper=module('hygiene',ROOT/'infra/cloud/base-supervisor-hygiene.py');ticks=helper.process_fact(child.pid)[1]
  child.terminate();child.wait(timeout=2)
  checkout=image/'home/zuse/workspace';status=image/'var/lib/zuse/workspace';recovery=image/'var/lib/zuse/recovery'
  status.mkdir(parents=True,exist_ok=True);recovery.mkdir(parents=True,exist_ok=True)
  h=helper.clean_stopped(data,f['owner'],child.pid,ticks,checkout,status,recovery)
  unit=image/'etc/systemd/system/zuse-runtime-supervisor.service';unit.parent.mkdir(parents=True);unit.write_text('synthetic enabled boot owner\n')
  h.update(operationId='original-builder-op',residentPid=child.pid,residentStartTicks=ticks,bootNonceSha256='b'*64,atomicStopAcknowledged=False,legacyBuilder=True,enabledBootServiceRetained=True,bootServiceSha256=D.digest(unit),runtimeSha256=json.loads(f['lineage']['documents']['builder'])['runtimeSha256'])
  receipt=dict(version=1,operationId=h['operationId'],machineId='original-clean-builder',parentLineageSha256=f['expected'],targetBundleSha256=D.digest(f['input']/'bundle.json'),hygiene=h)
  path=f['input']/'parent-stopped.json';path.write_text(json.dumps(receipt));path.chmod(0o600)
  return path,receipt,dict(MainPID='0',ActiveState='inactive',UnitFileState='enabled')

 def test_real_stopped_parent_is_admitted_only_with_bound_receipt(self):
  path,receipt,service=self.stopped_fixture()
  # The old live-only path rejects this exact hygienically stopped filesystem.
  path.unlink()
  with self.assertRaises((ValueError,OSError)):self.verify()
  path.write_text(json.dumps(receipt));path.chmod(0o600)
  r=D.verify(self.f['input'],self.f['expected'],filesystem=self.f['image'],owner=self.f['owner'],operation_id='original-builder-op',service_fact=service)
  self.assertEqual(r['status'],'passed');self.assertTrue(r['completeCodexAssetsVerified'])

 def test_stopped_parent_rejects_wrong_op_runtime_service_or_new_task(self):
  path,receipt,service=self.stopped_fixture()
  def check(op='original-builder-op',facts=service):
   return D.verify(self.f['input'],self.f['expected'],filesystem=self.f['image'],owner=self.f['owner'],operation_id=op,service_fact=facts)
  with self.assertRaises(ValueError):check(op='different-operation')
  for field,value in [('runtimeSha256','0'*64),('supervisorLockReleased',False),('legacyBuilder',False),('bootServiceSha256','0'*64)]:
   altered=json.loads(json.dumps(receipt));altered['hygiene'][field]=value;path.write_text(json.dumps(altered))
   with self.assertRaises(ValueError):check()
  path.write_text(json.dumps(receipt))
  with self.assertRaises(ValueError):check(facts=dict(MainPID='1',ActiveState='active',UnitFileState='enabled'))
  identity=self.f['image']/'home/zuse/.zuse-data/cloud-runtime-identity.json';identity.write_text('new task')
  with self.assertRaises(ValueError):check()

 def test_delayed_original_resident_observation_timeout_is_classified_and_retry_is_read_only(self):
  path=str(Path(self.tmp.name)/'ping.sock')
  server_code="""import socket,sys,time,json
s=socket.socket(socket.AF_UNIX);s.bind(sys.argv[1]);s.listen(2)
for i in range(2):
 c,_=s.accept();assert c.recv(16)==b'ping'
 if i==0:time.sleep(.12)
 try:c.sendall(json.dumps(dict(version=1,bound=False,childAlive=False,recoveryAlive=False)).encode())
 except BrokenPipeError:pass
 c.close()
s.close()
"""
  server=subprocess.Popen([sys.executable,'-I','-B','-c',server_code,path],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  client_code="import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.sendall(b'ping'); print(s.recv(4096).decode())"
  try:
   deadline=time.monotonic()+2
   while not Path(path).exists():
    if time.monotonic()>deadline:self.fail('controlled resident did not bind')
    time.sleep(.005)
   pid=server.pid
   def observe(command,**kwargs):
    self.assertEqual(command,['sudo','-n','-u','zuse','python3','/usr/local/lib/zuse/runtime-supervisor.py','ping'])
    return subprocess.run([sys.executable,'-I','-B','-c',client_code,path],**kwargs)
   with self.assertRaises(D.InspectionFailure) as failure:D.read_parent_ping(run=observe,timeout=.05)
   self.assertEqual(failure.exception.facts['stage'],'resident-idle-ping')
   self.assertEqual(failure.exception.facts['errorClass'],'TimeoutExpired')
   self.assertNotIn('argv',failure.exception.facts);self.assertNotIn('stdout',failure.exception.facts)
   self.assertIsNone(server.poll());self.assertEqual(server.pid,pid)
   # This is a second read of the SAME original controlled resident, not a
   # launch/restart or production timeout change. The caller chooses the retry.
   self.assertGreaterEqual(D.PARENT_PING_TIMEOUT_SECONDS,3+10)
   self.assertFalse(D.read_parent_ping(run=observe)['childAlive'])
   server.wait(timeout=2);self.assertEqual(server.returncode,0)
  finally:
   if server.poll() is None:server.terminate();server.wait(timeout=2)

 def test_inspection_diagnostics_never_include_subprocess_body_or_argv(self):
  def failed(*args,**kwargs):raise subprocess.CalledProcessError(7,['secret-argv'],output=b'secret-output',stderr=b'secret-stderr')
  with self.assertRaises(D.InspectionFailure) as failure:D.read_parent_ping(run=failed)
  facts=failure.exception.facts
  self.assertEqual(facts['exitStatus'],7);self.assertEqual(facts['errorClass'],'CalledProcessError')
  self.assertNotIn('secret',json.dumps(facts))


def disabled_fixture(f):
 root=f['image'];inp=f['input'];lineage=f['lineage'];parent=json.loads(lineage['documents']['parentBundle']);builder=json.loads(lineage['documents']['builder']);proof=json.loads(lineage['documents']['proof']);target=json.loads((inp/'bundle.json').read_text())
 helper=(ROOT/'infra/cloud/managed-base-fresh-boot.py').read_bytes();entry=dict(path='managed-base-fresh-boot.py',bytes=len(helper),sha256=hashlib.sha256(helper).hexdigest())
 parent['files'].append(entry);target['files'].append(dict(entry));(inp/entry['path']).write_bytes(helper)
 for v in(parent,target):v.update(managedBaseFreshBootVersion=1,managedBaseFreshBootMode='post-hydration-first-start-v1')
 unit=b'[Service]\nUser=zuse\nExecStart=/usr/bin/python3 /usr/local/lib/zuse/runtime-supervisor.py serve\n'
 p=root/'etc/systemd/system/zuse-runtime-supervisor.service';p.parent.mkdir(parents=True);p.write_bytes(unit)
 p=root/'usr/local/lib/zuse/managed-base-fresh-boot.py';p.write_bytes(helper)
 pins=dict(version=1,mode='post-hydration-first-start-v1',runtimeSha256=parent['runtimeSha256'],supervisorSha256=next(x['sha256']for x in parent['files']if x['path']=='runtime-supervisor.py'),serviceSha256=hashlib.sha256(unit).hexdigest(),manifestSha256='a'*64,helperSha256=entry['sha256'])
 builder['manifest']['rustRuntimeSupervisorVersion']=1;builder['manifest']['commandAdmission']={'runtimeSha256':parent['runtimeSha256']};pins['manifestSha256']=hashlib.sha256(json.dumps(builder['manifest']).encode()).hexdigest();builder['initialRuntimeBootstrap']=pins;proof['initialRuntimeBootstrap']=dict(pins)
 parenttext=json.dumps(parent);builder['bundleSha256']=hashlib.sha256(parenttext.encode()).hexdigest();lineage['parentBundleFileSha256']=builder['bundleSha256']
 for name,value in [('parentBundle',parent),('builder',builder),('proof',proof)]:
  lineage['documents'][name]=json.dumps(value);lineage['digests'][name]=hashlib.sha256(lineage['documents'][name].encode()).hexdigest()
 (inp/'derivation.json').write_text(json.dumps(lineage));(inp/'bundle.json').write_text(json.dumps(target));f['expected']=D.digest(inp/'derivation.json')
 (root/'opt/zuse/preparation/status/manifest.json').write_text(json.dumps(builder['manifest']))
 (root/'home/zuse/.zuse-data/.runtime-supervision').rmdir()
 for name,mode in [('usr/local/bin/zuse-cloud-runtime',0o755),('usr/local/lib/zuse/runtime-supervisor.py',0o555),('usr/local/lib/zuse/managed-base-fresh-boot.py',0o555),('etc/systemd/system/zuse-runtime-supervisor.service',0o644),('opt/zuse/preparation/status/manifest.json',0o600)]:(root/name).chmod(mode)
 return dict(MainPID='0',ActiveState='inactive',UnitFileState='disabled',ControlGroup='')

class DisabledCapturedParent(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory(dir=ROOT/'.context',prefix='disabled-derive-test-');self.f=fixture(Path(self.tmp.name));self.service=disabled_fixture(self.f)
 def tearDown(self):self.tmp.cleanup()
 def verify(self,**kw):
  return D.verify_parent(self.f['input'],self.f['expected'],filesystem=self.f['image'],owner=self.f['owner'],quiescence_image=True,service_fact=kw.get('service',self.service),process_facts=kw.get('processes',[]),**kw.get('extra',{}))
 def test_exact_disabled_parent_passes_without_starting_old_resident(self):
  result=self.verify()
  self.assertEqual(result['status'],'quiescence-image-verified')
  self.assertEqual(result['parentMode'],'post-hydration-first-start-v1')
  self.assertTrue(result['noResidentPresent'])
 def test_disabled_but_running_enabled_or_pending_unit_is_rejected(self):
  for changes in({'MainPID':'41'},{'ActiveState':'active'},{'UnitFileState':'enabled'},{'Job':'41'}):
   with self.subTest(changes=changes),self.assertRaisesRegex(ValueError,'disabled_parent_service_not_inactive'):self.verify(service=dict(self.service,**changes))
 def test_any_agent_user_process_or_runtime_root_process_is_rejected(self):
  for processes in([{'pid':41,'state':'S','uids':[10001]*4,'argv':['/bin/sh']}],[{'pid':42,'state':'S','uids':[0]*4,'argv':['/usr/bin/python3','/usr/local/lib/zuse/runtime-supervisor.py','serve']}]):
   with self.subTest(processes=processes),self.assertRaisesRegex(ValueError,'disabled_parent_process_present'):self.verify(processes=processes)
 def test_dirty_runtime_and_credentials_and_repository_are_rejected(self):
  for name in('home/zuse/.zuse-data/cloud-runtime-identity.json','home/zuse/workspace/file','home/zuse/.config/grok/auth.json','home/zuse/.ssh/id_ed25519','var/lib/zuse/recovery/bound.json'):
   p=self.f['image']/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text('sensitive')
   with self.subTest(name=name),self.assertRaises(ValueError):self.verify()
   p.unlink()
 def test_helper_unit_or_manifest_tampering_is_rejected(self):
  for name in('usr/local/lib/zuse/managed-base-fresh-boot.py','etc/systemd/system/zuse-runtime-supervisor.service','opt/zuse/preparation/status/manifest.json'):
   p=self.f['image']/name;original=p.read_bytes();p.write_bytes(original+b' ')
   with self.subTest(name=name),self.assertRaises(ValueError):self.verify()
   p.write_bytes(original)
 def test_unknown_boot_mode_is_rejected_before_admission(self):
  lineage=self.f['lineage'];parent=json.loads(lineage['documents']['parentBundle']);parent['managedBaseFreshBootMode']='future-unverified'
  builder=json.loads(lineage['documents']['builder']);text=json.dumps(parent);builder['bundleSha256']=hashlib.sha256(text.encode()).hexdigest();lineage['parentBundleFileSha256']=builder['bundleSha256']
  for name,value in [('parentBundle',parent),('builder',builder)]:
   lineage['documents'][name]=json.dumps(value);lineage['digests'][name]=hashlib.sha256(lineage['documents'][name].encode()).hexdigest()
  (self.f['input']/'derivation.json').write_text(json.dumps(lineage));self.f['expected']=D.digest(self.f['input']/'derivation.json')
  with self.assertRaisesRegex(ValueError,'unsupported_clean_parent_mode'):self.verify()
 def test_disabled_receipt_is_distinct_and_revalidated_for_overlay(self):
  Q=module('disabled_parent_quiescence',ROOT/'infra/cloud/clean-parent-quiescence.py');proof=self.verify();operation='zuse-base-build-'+'e'*24
  receipt=Q.already_disabled_parent(proof,operation,self.f['expected'],self.verify)
  self.assertFalse(receipt['startedOrStoppedProcess']);self.assertFalse(receipt['legacyBuilder']);self.assertFalse(receipt['atomicStopAcknowledged']);self.assertNotIn('residentPid',receipt)
  stopped={'version':1,'operationId':operation,'machineId':'one-owned-builder','parentLineageSha256':self.f['expected'],'targetBundleSha256':D.digest(self.f['input']/'bundle.json'),'hygiene':receipt}
  path=self.f['input']/'parent-stopped.json';path.write_text(json.dumps(stopped));path.chmod(0o600)
  result=D.verify_parent(self.f['input'],self.f['expected'],filesystem=self.f['image'],owner=self.f['owner'],operation_id=operation,service_fact=self.service,process_facts=[])
  self.assertEqual(result['status'],'passed')
  stopped['hygiene']['noResidentPresent']=False;path.write_text(json.dumps(stopped))
  with self.assertRaisesRegex(ValueError,'disabled_parent_stop_receipt_invalid'):D.verify_parent(self.f['input'],self.f['expected'],filesystem=self.f['image'],owner=self.f['owner'],operation_id=operation,service_fact=self.service,process_facts=[])
 def test_production_task_directories_use_agent_uid_not_operator_uid(self):
  from unittest.mock import patch
  _,lineage,_,_,_=D.context(self.f['input'],self.f['expected'],self.f['owner']);original=D.secure;checked=[]
  private={self.f['image']/x for x in ('home/zuse/.zuse-data','home/zuse/workspace','var/lib/zuse/workspace','var/lib/zuse/recovery')}
  for path in private:path.mkdir(parents=True,exist_ok=True)
  def permissions(path,uid):
   from types import SimpleNamespace
   if path in private:self.assertEqual(uid,10001);checked.append(path)
   observed=original(path,self.f['owner'])
   # Translate the fixture's one OS uid into the production root/agent domains.
   if '/'+str(path.relative_to(self.f['image']))in D.DISABLED_IMAGE_PINS:return SimpleNamespace(st_mode=observed.st_mode,st_nlink=observed.st_nlink,st_uid=uid,st_gid=uid)
   return observed
  with patch.object(D,'secure',side_effect=permissions):D.disabled_parent_state(lineage,self.f['image'],0,self.service,[])
  self.assertEqual(set(checked),private)
 def test_actual_five_pin_metadata_includes_agent_owned_manifest(self):
  from unittest.mock import patch
  from types import SimpleNamespace
  import stat
  saved=json.loads((ROOT/'infra/cloud/tests/fixtures/boat-disabled-parent-five-pins.json').read_text());pins=dict(saved['pins'],version=1,mode=saved['mode']);metadata={self.f['image']/x['path'].lstrip('/'):x for x in saved['files']}
  lineage={'parentBundle':{'managedBaseFreshBootVersion':1,'managedBaseFreshBootMode':saved['mode'],'files':[{'path':'runtime-supervisor.py','sha256':pins['supervisorSha256']},{'path':'managed-base-fresh-boot.py','sha256':pins['helperSha256']}]},'builder':{'runtimeSha256':pins['runtimeSha256'],'initialRuntimeBootstrap':pins,'manifest':{'rustRuntimeSupervisorVersion':1,'commandAdmission':{'runtimeSha256':pins['runtimeSha256']}}},'proof':{'initialRuntimeBootstrap':dict(pins)}}
  original=D.secure;checks=[]
  def observed(path,uid):
   if path in metadata:
    row=metadata[path];self.assertEqual(uid,row['uid'],str(path));checks.append(row['path']);return SimpleNamespace(st_mode=stat.S_IFREG|row['mode'],st_nlink=row['nlink'],st_uid=row['uid'],st_gid=row['gid'])
   return original(path,self.f['owner'])
  with patch.object(D,'secure',side_effect=observed),patch.object(D,'digest',side_effect=lambda path:metadata[path]['sha256']):
   value=D.disabled_parent_state(lineage,self.f['image'],0,self.service,[])
  self.assertTrue(value['noResidentPresent']);self.assertEqual(set(checks),{x['path']for x in saved['files']})
 def test_saved_five_pin_metadata_rejects_owner_mode_symlink_hash_and_unknown_path(self):
  from types import SimpleNamespace
  import stat
  saved=json.loads((ROOT/'infra/cloud/tests/fixtures/boat-disabled-parent-five-pins.json').read_text())
  for row in saved['files']:
   fields=dict(st_mode=stat.S_IFREG|row['mode'],st_uid=row['uid'],st_gid=row['gid'],st_nlink=1)
   D.disabled_pin_metadata(row['path'],SimpleNamespace(**fields),row['sha256'],saved['pins'])
   for change in({'st_uid':999},{'st_gid':999},{'st_mode':stat.S_IFREG|0o777},{'st_mode':stat.S_IFLNK|0o777},{'st_nlink':2}):
    with self.subTest(path=row['path'],change=change),self.assertRaises(ValueError):D.disabled_pin_metadata(row['path'],SimpleNamespace(**dict(fields,**change)),row['sha256'],saved['pins'])
   with self.assertRaises(ValueError):D.disabled_pin_metadata(row['path'],SimpleNamespace(**fields),'0'*64,saved['pins'])
  with self.assertRaisesRegex(ValueError,'unsupported_disabled_parent_pin_path'):D.disabled_pin_metadata('/unknown',SimpleNamespace(),'',saved['pins'])
 def test_full_preflight_replays_retained_parent_manifest_and_five_pins(self):
  from unittest.mock import patch
  saved=json.loads((ROOT/'infra/cloud/tests/fixtures/boat-disabled-parent-five-pins.json').read_text());inp=self.f['input'];image=self.f['image'];lineage=self.f['lineage'];parent=json.loads(lineage['documents']['parentBundle']);builder=json.loads(lineage['documents']['builder']);proof=json.loads(lineage['documents']['proof']);pins=dict(saved['pins'],version=1,mode=saved['mode'])
  # Four small files replay the exact captured bytes. The184MB runtime digest
  # comes from retained signed image metadata; this test does not copy a VM binary.
  bodies={'runtime-supervisor.py':(ROOT/'infra/cloud/runtime-supervisor.py').read_bytes(),'managed-base-fresh-boot.py':(ROOT/'infra/cloud/managed-base-fresh-boot.py').read_bytes()}
  for name,body in bodies.items():
   path=image/'usr/local/lib/zuse'/name;path.chmod(0o755);path.write_bytes(body);path.chmod(0o555)
   entry=next(x for x in parent['files']if x['path']==name);entry.update(sha256=hashlib.sha256(body).hexdigest(),bytes=len(body))
  (image/'etc/systemd/system/zuse-runtime-supervisor.service').write_text(saved['serviceRaw']);(image/'opt/zuse/preparation/status/manifest.json').write_text(saved['manifestRaw'])
  parent['runtimeSha256']=pins['runtimeSha256'];next(x for x in parent['files']if x['path']=='runtime').update(sha256=pins['runtimeSha256'],bytes=184426912);builder.update(runtimeSha256=pins['runtimeSha256'],manifest=json.loads(saved['manifestRaw']),initialRuntimeBootstrap=pins);proof.update(runtimeSha256=pins['runtimeSha256'],initialRuntimeBootstrap=dict(pins));parenttext=json.dumps(parent);builder['bundleSha256']=hashlib.sha256(parenttext.encode()).hexdigest();lineage['parentBundleFileSha256']=builder['bundleSha256']
  for name,value in [('parentBundle',parent),('builder',builder),('proof',proof)]:
   lineage['documents'][name]=json.dumps(value);lineage['digests'][name]=hashlib.sha256(lineage['documents'][name].encode()).hexdigest()
  (inp/'derivation.json').write_text(json.dumps(lineage));expected=D.digest(inp/'derivation.json');original=D.digest;runtime=image/'usr/local/bin/zuse-cloud-runtime'
  with patch.object(D,'digest',side_effect=lambda path:pins['runtimeSha256']if path==runtime else original(path)):
   value=D.verify_parent(inp,expected,filesystem=image,owner=self.f['owner'],quiescence_image=True,service_fact=self.service,process_facts=[])
  self.assertEqual(value['status'],'quiescence-image-verified');self.assertEqual(value['parentRuntimeSha256'],pins['runtimeSha256']);self.assertTrue(value['sourceClean']);self.assertTrue(value['completeCodexAssetsVerified'])
 def test_disabled_receipt_cannot_outlive_fresh_verification(self):
  Q=module('disabled_parent_quiescence_race',ROOT/'infra/cloud/clean-parent-quiescence.py');proof=self.verify()
  with self.assertRaisesRegex(Q.Rejected,'disabled_parent_changed_before_receipt'):Q.already_disabled_parent(proof,'zuse-base-build-'+'e'*24,self.f['expected'],lambda:dict(proof,noResidentPresent=False))

if __name__=='__main__':
 if len(sys.argv)==3 and sys.argv[1]=='--emit-fixture':
  f=fixture(Path(sys.argv[2]));print(json.dumps({k:str(v) for k,v in f.items() if k in ('image','input','expected')}))
 else:unittest.main()
