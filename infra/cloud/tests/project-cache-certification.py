#!/usr/bin/env python3
"""Synthetic isolated evidence/PG fixtures; never live certification evidence."""
import copy,hashlib,importlib.util,json,os,subprocess,sys,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
HERE=Path(__file__).resolve().parents[1]
def load(name,path):
 s=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
m=load('cache_cert',HERE/'project-cache-certification.py')
def write(p,v):
 p.write_bytes(m.canonical(v)+b'\n');p.chmod(0o600)
def fixture(root):
 release=dict(provider='boxd',releaseId='synthetic-base',artifactId='synthetic-base-snapshot',architecture='x86_64',capabilities=dict(id='boxd'),manifest=dict(managedBase=True,credentialsRemoved=True,repositories=[],buildId='synthetic-base'),harnesses=[dict(agent=a,sha256='b'*64) for a in ('claude','codex','grok')],initialRuntimeBootstrap=dict(version=1,mode='stopped-memory-snapshot-v1',**{k:'a'*64 for k in m.PINS}))
 report=dict(schema=1,status='passed',runId='synthetic-cert',provider='boxd',releaseSha256=m.digest(m.canonical(release)),driverSha256='c'*64,cases=[dict(agent=a,case=c,passed=True,elapsedMs=1) for a in ('claude','codex','grok') for c in m.module('managed-base-release').CASES],cleanupVerified=True)
 write(root/'base-report.json',report);release['certificationReportSha256']=m.digest((root/'base-report.json').read_bytes());write(root/'release.json',release)
 i=dict(version=1,ownerId='synthetic-owner',projectId='synthetic-project',repositoryIdentity='github.com/example/public',provider='boxd',architecture='x86_64',baseReleaseId='synthetic-base',baseArtifactId='synthetic-base-snapshot',baseArtifactVersion='1',baseRuntimeSha256='a'*64,recipeSource='project',recipeDigest='d'*64,sourceCommit='e'*40,dependencyInputs={'package-lock.json':'f'*40})
 key=m.identity_key(i);op='cacheop_synthetic';prepared=dict(version=1,cacheKey=key,operationId=op,runtimeSha256='a'*64,recipeDigest='d'*64,dependencyDigest=m.digest(json.dumps([i['sourceCommit'],i['dependencyInputs']],separators=(',',':')).encode()),sourceCommit='e'*40,sourceBundleSha256='9'*64,credentialsRemoved=True,taskStorageAbsent=True,agentProfilesAbsent=True,secretFreeSource=True)
 stamp=dict(version=1,identity=i,receipt=prepared);write(root/'prepared-stamp.json',stamp)
 h=dict(version=1,operationId=op,runtimeSha256='a'*64,atomicStopAcknowledged=True,legacyBuilder=False,originalResidentExited=True,supervisorLockReleased=True,runtimeDataAbsent=True,enabledBootServiceRetained=True,residentPid=42,residentStartTicks=123,bootServiceSha256='a'*64,bootNonceSha256='c'*64,removed=['supervisor.lock','supervisor.json','control.sock'])
 cap=dict(version=1,runtimeSha256='a'*64,environmentCacheVersion=1,secretFreeSourceBundleVersion=1,retainedTaskStorageAbsentRequired=True,credentialFreePreparationRequired=True,preparationCaptureVersion=1)
 capture=dict(state='ready',receipt=dict(version=1,preparationCaptureVersion=1,cacheKey=key,operationId=op,runtimeSha256='a'*64,preparedReceiptSha256=m.digest((root/'prepared-stamp.json').read_bytes()),hygiene=h))
 def boot(machine):
  q=dict(version=1,provider='boxd',purpose='cache-preparation',operationId=op,workspaceId='cache:'+key,generation=1,storageIncarnationId='cache:'+op,sourceMachineId=machine,imagePins={k:'a'*64 for k in m.PINS});return dict(request=q,receipt=dict(q,status='ready',requestSha256=m.digest(m.canonical(q)),unitStartSubmitted=True,resident=dict(residentPid=42,residentStartTicks=123,bootNonceSha256='c'*64,ipcVerified=True,unbound=True,childAlive=False,recoveryAlive=False)))
 restored=dict(version=1,status='passed',operationId=op,cacheKey=key,builderId='synthetic-builder',builderName='pc-b-synthetic',verifierId='synthetic-verifier',verifierName='pc-v-synthetic',artifactId='synthetic-cache-snapshot',artifactName='pc-s-synthetic',artifactVersion='1',sourceVerified=True,builderBootstrap=boot('synthetic-builder'),verifierBootstrap=boot('synthetic-verifier'),restoredStampSha256=m.digest((root/'prepared-stamp.json').read_bytes()),captureStatus=capture,runtimeSha256='a'*64)
 cleanup=dict(version=1,independentlyVerified=True,operationId=op,allAbsent=True,checks=[dict(kind=kind,reference=ref,providerNotFound=True) for kind,ref in [('machine',restored[k]) for k in ('builderId','builderName','verifierId','verifierName')]+[('snapshot',restored[k]) for k in ('artifactId','artifactName')]])
 expected_marker=dict(path=m.MARKER,sha256='7'*64,bytes=42)
 intent=dict(version=1,phase='capture-intent',operationId=op,cacheKey=key,builderId=restored['builderId'],expectedHead=i['sourceCommit'],expectedMarker=expected_marker,preparedStampSha256=m.digest((root/'prepared-stamp.json').read_bytes()),createdAtMs=2);write(root/'pre-capture.json',intent)
 fs=dict(version=1,operationId=op,cacheKey=key,captureIntentSha256=m.digest((root/'pre-capture.json').read_bytes()),captureObservedAtMs=3,builder=dict(machineId=restored['builderId'],head=i['sourceCommit'],marker=copy.deepcopy(expected_marker),symlinksRejected=True,readOnly=True,observedAtMs=1),restored=dict(machineId=restored['verifierId'],head=i['sourceCommit'],marker=copy.deepcopy(expected_marker),symlinksRejected=True,readOnly=True,observedAtMs=4));write(root/'filesystem.json',fs)
 for name,v in [('capability.json',cap),('capture.json',capture),('restore.json',restored),('cleanup.json',cleanup)]:write(root/name,v)
 evidence=dict(version=1,kind='project-cache-capability-evidence-v1',provider='boxd',releaseId='synthetic-base',baseArtifactId='synthetic-base-snapshot',baseArtifactVersion='1',runtimeSha256='a'*64,operationId=op,cacheKey=key,documents={n:m.digest((root/n).read_bytes()) for n in m.DOCS});write(root/'evidence.json',evidence)
 return release
class EvidenceTests(unittest.TestCase):
 def setUp(self):
  self.t=tempfile.TemporaryDirectory();self.addCleanup(self.t.cleanup);self.root=Path(self.t.name);self.release=fixture(self.root)
 def plan(self):return m.make_plan(self.root/'release.json',self.root/'base-report.json',self.root)
 def change(self,name,mutate):
  p=self.root/name;v=m.load(p);mutate(v);write(p,v)
  if name in m.DOCS:
   e=m.load(self.root/'evidence.json');e['documents'][name]=m.digest(p.read_bytes());write(self.root/'evidence.json',e)
 def test_complete_evidence_builds_metadata_only_plan_without_db(self):
  with patch.object(m.subprocess,'run',side_effect=AssertionError('No DB in default plan')):p=self.plan()
  self.assertEqual(p['receipt']['preparationCaptureVersion'],1);self.assertFalse(p['databaseVerified']);self.assertNotIn('ownerId',m.canonical(p).decode());self.assertEqual(set(p['evidenceSha256']),m.DOCS|{'evidence.json'})
 def test_every_cross_binding_and_safety_failure_rejects(self):
  mutations=[('filesystem.json',lambda v:v['restored'].update(head='0'*40)),('filesystem.json',lambda v:v['restored']['marker'].update(sha256='0'*64)),('filesystem.json',lambda v:v['restored']['marker'].update(bytes=1)),('filesystem.json',lambda v:v['restored'].update(symlinksRejected=False)),('filesystem.json',lambda v:v['restored'].update(observedAtMs=1)),('pre-capture.json',lambda v:v['expectedMarker'].update(path='../outside')),('pre-capture.json',lambda v:v['expectedMarker'].update(bytes=4097)),('capability.json',lambda v:v.update(runtimeSha256='0'*64)),('capability.json',lambda v:v.update(version=True)),('capability.json',lambda v:v.pop('preparationCaptureVersion')),('prepared-stamp.json',lambda v:v['identity'].update(ownerId='other')),('prepared-stamp.json',lambda v:v['receipt'].update(credentialsRemoved=1)),('capture.json',lambda v:v['receipt'].update(preparedReceiptSha256='0'*64)),('capture.json',lambda v:v['receipt']['hygiene'].update(atomicStopAcknowledged=False)),('capture.json',lambda v:v['receipt']['hygiene'].update(legacyBuilder=True)),('capture.json',lambda v:v['receipt']['hygiene'].update(bootServiceSha256='0'*64)),('restore.json',lambda v:v.update(restoredStampSha256='0'*64)),('restore.json',lambda v:v['verifierBootstrap']['request'].update(sourceMachineId='other')),('restore.json',lambda v:v['verifierBootstrap']['receipt']['resident'].update(childAlive=True)),('restore.json',lambda v:v.update(builderId=v['verifierId'])),('restore.json',lambda v:v.update(artifactId='synthetic-base-snapshot')),('cleanup.json',lambda v:v['checks'].pop()),('cleanup.json',lambda v:v['checks'][0].update(providerNotFound=False)),('cleanup.json',lambda v:v['checks'][0].update(reference='other')),('cleanup.json',lambda v:v.update(allAbsent=False))]
  for name,mutate in mutations:
   with self.subTest(name=name):
    fixture(self.root);self.change(name,mutate)
    with self.assertRaises(m.Rejected):self.plan()
 def test_missing_or_forged_full_base_certification_rejects(self):
  for mutate in [lambda v:v.pop('certificationReportSha256'),lambda v:v.update(certificationReportSha256='0'*64),lambda v:v.update(releaseId='synthetic-canary')]:
   fixture(self.root);self.change('release.json',mutate)
   with self.assertRaises(m.Rejected):self.plan()
  fixture(self.root);r=m.load(self.root/'base-report.json');r['cases'].pop();write(self.root/'base-report.json',r);self.release['certificationReportSha256']=m.digest((self.root/'base-report.json').read_bytes());write(self.root/'release.json',self.release)
  with self.assertRaisesRegex(m.Rejected,'cases_incomplete'):self.plan()
 def test_unknown_fields_client_content_symlink_and_duplicate_json_reject(self):
  self.change('restore.json',lambda v:v.update(token='never-retain-this'))
  with self.assertRaises(m.Rejected):self.plan()
  fixture(self.root);p=self.root/'capability.json';p.rename(self.root/'original');p.symlink_to(self.root/'original')
  with self.assertRaisesRegex(m.Rejected,'symlink'):self.plan()
  p.unlink();fixture(self.root);(self.root/'capability.json').write_text('{"version":1,"version":1}');(self.root/'capability.json').chmod(0o600)
  self.change('evidence.json',lambda v:v['documents'].update({'capability.json':m.digest((self.root/'capability.json').read_bytes())}))
  with self.assertRaisesRegex(m.Rejected,'duplicate_json'):self.plan()
 def test_default_cli_does_not_read_inherited_database_or_provider_keys(self):
  env=dict(os.environ,ZUSE_CLOUD_DATABASE_URL='secret-sentinel',BOXD_API_KEY='secret-sentinel');p=subprocess.run([sys.executable,str(HERE/'project-cache-certification.py'),'--release',str(self.root/'release.json'),'--base-report',str(self.root/'base-report.json'),'--evidence-directory',str(self.root),'--plan',str(self.root/'plan.json')],capture_output=True,text=True,env=env)
  self.assertEqual(p.returncode,0,p.stderr);self.assertNotIn('secret-sentinel',p.stdout+p.stderr+(self.root/'plan.json').read_text());self.assertEqual(json.loads(p.stdout)['status'],'validated-plan-only')
 def test_future_primitive_commands_use_same_protected_helper_before_preparation(self):
  stamp=m.load(self.root/'prepared-stamp.json');i=stamp['identity'];op=stamp['receipt']['operationId'];guest=dict(version=1,cacheKey=m.identity_key(i),operationId=op,identity=i,recipe={'secretBindings':[]},sourceBundleSha256='9'*64,home='/home/zuse',cacheRoot='/opt/zuse/project-caches',bundle='/opt/zuse/preparation/source.bundle',supervisor=True)
  raw=m.canonical(guest);v=m.primitive_commands(self.release,guest,raw,op,builder='original-builder')
  self.assertEqual(v['status'],'commands-only-not-evidence');self.assertTrue(v['requiresSeparateProviderGo']);self.assertTrue(v['requiresBuilderAbsenceBeforeVerifierCreate']);self.assertFalse(v['providerEffects'])
  restored=m.primitive_commands(self.release,guest,raw,op,verifier='original-verifier',cache_artifact='original-cache-snapshot',cache_version='1')
  for packet,role,machine in [(v,'builder','original-builder'),(restored,'verifier','original-verifier')]:
   c=packet['commands'][role+'Bootstrap'];self.assertEqual(c['request']['sourceMachineId'],machine);self.assertEqual(c['request']['purpose'],'cache-preparation');self.assertIn('sudo -n /usr/bin/python3 -I -B -c',c['command']);self.assertEqual(c['requestSha256'],m.digest(m.canonical(c['request'])))
  self.assertNotIn('secretBindings',m.canonical(v).decode());self.assertIn(m.digest(raw),v['commands']['capture']);self.assertTrue(restored['commands']['restoredCaptureStatus'].endswith(' --status'));self.assertNotIn('prepare',restored['commands']);self.assertNotIn('verifierBootstrap',v['commands'])
  write(self.root/'guest-plan.json',guest)
  cli=subprocess.run([sys.executable,str(HERE/'project-cache-certification.py'),'--commands','--release',str(self.root/'release.json'),'--guest-plan',str(self.root/'guest-plan.json'),'--operation-id',op,'--builder-id','original-builder','--plan',str(self.root/'commands.json')],capture_output=True,text=True)
  self.assertEqual(cli.returncode,0,cli.stderr);self.assertEqual(json.loads(cli.stdout)['status'],'commands-only-not-evidence')
  with self.assertRaises(m.Rejected):m.primitive_commands(self.release,guest,raw,op,builder='builder',verifier='verifier')
  guest['recipe']['secretBindings']=['not-allowed']
  with self.assertRaises(m.Rejected):m.primitive_commands(self.release,guest,raw,op,builder='original-builder')
 def test_actual_readonly_filesystem_probe_rejects_symlinks_and_bounds(self):
  key='a'*64;base=self.root/'probe';checkout=base/key/'checkout';checkout.mkdir(parents=True)
  for args in [['init','-q'],['config','user.name','Synthetic'],['config','user.email','synthetic@example.test']]:subprocess.run(['/usr/bin/git','-C',str(checkout),*args],check=True,capture_output=True)
  (checkout/'README').write_text('synthetic public fixture');subprocess.run(['/usr/bin/git','-C',str(checkout),'add','README'],check=True,capture_output=True);subprocess.run(['/usr/bin/git','-C',str(checkout),'commit','-qm','synthetic'],check=True,capture_output=True)
  marker=checkout/m.MARKER;marker.parent.mkdir();marker.write_bytes(b'synthetic dependency');marker.chmod(0o600)
  program=m.FILESYSTEM_PROBE.replace("Path('/opt/zuse/project-caches')",'Path('+repr(str(base))+')')
  run=lambda:subprocess.run([sys.executable,'-I','-B','-c',program,key],capture_output=True,text=True)
  good=run();self.assertEqual(good.returncode,0,good.stderr);v=json.loads(good.stdout);self.assertEqual(v['marker']['sha256'],m.digest(marker.read_bytes()));self.assertTrue(v['readOnly'])
  original=self.root/'original-marker';marker.rename(original);marker.symlink_to(original);self.assertNotEqual(run().returncode,0);marker.unlink();marker.write_bytes(b'x'*4097);self.assertNotEqual(run().returncode,0)
  marker.unlink();marker.write_bytes(b'bounded');directory=marker.parent;directory.rename(checkout/'moved');directory.symlink_to(checkout/'moved');self.assertNotEqual(run().returncode,0)
  directory.unlink();(checkout/'moved').rename(directory);keydir=base/key;keydir.rename(base/'other');keydir.symlink_to(base/'other');self.assertNotEqual(run().returncode,0)
 def test_apply_and_inspect_never_reopen_the_validated_release(self):
  plan=self.plan();write(self.root/'plan.json',plan);approved=m.digest((self.root/'plan.json').read_bytes());(self.root/'dsn').write_text('postgresql://operator@127.0.0.1/db?sslmode=disable');(self.root/'dsn').chmod(0o600)
  original=m.read;calls=[]
  def race(path,*args):
   if Path(path)==self.root/'release.json':
    calls.append(1)
    if len(calls)>1:return m.canonical(dict(self.release,artifactId='swapped-after-validation'))
   return original(path,*args)
  with patch.object(m,'read',side_effect=race),patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],0,'{"status":"certified","schema":"zuse_staging"}\n','')):
   m.apply(self.root/'plan.json',approved,self.root/'release.json',self.root/'base-report.json',self.root,self.root/'dsn',self.root/'apply.json')
  self.assertEqual(len(calls),1);calls.clear()
  with patch.object(m,'read',side_effect=race),patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],0,'{"schema":"zuse_staging","environment":"staging","capabilityMatches":true,"baseStillVerified":true}\n','')):
   self.assertEqual(m.inspect_original(self.root/'plan.json',approved,self.root/'release.json',self.root/'base-report.json',self.root,self.root/'dsn',self.root/'apply.json')['status'],'original-commit-observed')
  self.assertEqual(len(calls),1)
 def test_tls_and_apply_hash_fail_before_database_call(self):
  plan=self.plan();write(self.root/'plan.json',plan);(self.root/'dsn').write_text('postgresql://operator:private@example.test/db?sslmode=require');(self.root/'dsn').chmod(0o600)
  with patch.object(m.subprocess,'run',side_effect=AssertionError('No SQL')):
   with self.assertRaises(m.Rejected):m.apply(self.root/'plan.json','0'*64,self.root/'release.json',self.root/'base-report.json',self.root,self.root/'dsn',self.root/'receipt.json')
   with self.assertRaises(ValueError):m.apply(self.root/'plan.json',m.digest((self.root/'plan.json').read_bytes()),self.root/'release.json',self.root/'base-report.json',self.root,self.root/'dsn',self.root/'receipt.json')
  self.assertFalse((self.root/'receipt.json.intent.json').exists())
if __name__=='__main__':unittest.main()
