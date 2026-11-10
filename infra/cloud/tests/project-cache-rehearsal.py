#!/usr/bin/env python3
"""Offline rehearsal packet and rejection tests; no live evidence or effects."""
from pathlib import Path
import importlib.util,json,tempfile,time,unittest
from unittest.mock import patch
HERE=Path(__file__).resolve().parents[1]
def load(name,path):
 s=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
M=load('rehearsal_packet',HERE/'project-cache-rehearsal.py')
R=load('rehearsal_run',HERE/'project-cache-rehearsal-run.py')
F=load('rehearsal_fixtures',HERE/'tests/project-cache-certification.py')
class Packet(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=Path(self.tmp.name);self.now=int(time.time()*1000)
  candidate=F.fixture(self.root);candidate['runtimeSha256']=M.RUNTIME;candidate['initialRuntimeBootstrap']['runtimeSha256']=M.RUNTIME
  self.pin=dict(ownerId='synthetic-owner',provider='boxd',releaseId=candidate['releaseId'],artifactId=candidate['artifactId'],candidateSha256=M.S.digest(M.S.canonical(candidate)),manifestSha256=M.S.digest(M.S.canonical(candidate['manifest'])),expiresAt=self.now+1800000)
  self.bounds=dict(version=1,caseId='synthetic-case',ownerId='synthetic-owner',projectId='synthetic-project',protectedWorkspaceId='synthetic-original',protectedMachineId='synthetic-original-machine',protectedGeneration=1,protectedRetainedSha256='1'*64,apiSha256='2'*64,maxActiveMachines=2,maxOwnedMachines=1,maxDurationSeconds=900,cleanupReserveSeconds=180)
  self.source=dict(version=1,ownerId='synthetic-owner',actorId='synthetic-owner',projectId='synthetic-project',repositoryIdentity='github.com/example/private',sourceCommit='a'*40,recipeSource='project',dependencyInputs={'package-lock.json':'b'*40},environmentRevision=0,codexConnectionId='synthetic-connection',codexModel='gpt-6.1-sol',sourceObservedAtMs=self.now,repositoryRecipeAbsent=True,dedicatedProject=True)
  for n,v in [('candidate',candidate),('pin',self.pin),('source',self.source),('bounds',self.bounds)]:F.write(self.root/(n+'.json'),v)
 def packet(self):return M.packet(*(self.root/(n+'.json')for n in ('candidate','pin','source','bounds')),'1',self.now+600000)
 def test_exact_packet_has_no_api_binding_or_effects_and_separate_recipe_digests(self):
  with patch.object(M.ROLLOUT.subprocess,'run',side_effect=AssertionError('offline')):
   p=self.packet()
  self.assertIsNone(p['futureApiBinaryBinding']);self.assertFalse(p['effectsAuthorized']);self.assertEqual(len(p['pilot']['inputs']),2)
  self.assertNotEqual(*[i['recipeDigest']for i in p['pilot']['inputs']]);self.assertIsNone(p['tasks']['miss']['firstMessage'])
  self.assertIsNone(p['tasks']['restore']['firstMessage']);self.assertEqual(p['tasks']['hit']['agent'],'codex')
  self.assertEqual(R.validate(p)['status'],'offline-valid')
 def test_stale_or_existing_user_project_cannot_be_implicitly_adopted(self):
  for key,value in [('dedicatedProject',False),('repositoryRecipeAbsent',False),('sourceObservedAtMs',self.now-86400001),('actorId','other')]:
   original=self.source[key];self.source[key]=value;F.write(self.root/'source.json',self.source)
   with self.assertRaises(M.S.Rejected):self.packet()
   self.source[key]=original;F.write(self.root/'source.json',self.source)
 def test_prepared_operator_plan_preserves_candidate_namespace_and_default_off(self):
  p=self.packet();F.write(self.root/'pilot.json',p['pilot'])
  v=M.ROLLOUT.plan(self.root/'candidate.json',self.root/'bounds.json',self.root/'pilot.json',self.root/'pin.json')
  self.assertEqual(v['baseAdmission'],'staging-pilot');self.assertFalse(v['certifiesBaseOrCache'])
  self.assertEqual(v['workerEnvironmentOverlay']['ZUSE_CLOUD_PROJECT_PREPARATION_BACKGROUND'],'0')
 def test_root_go_requires_bound_fresh_boxd_proof_and_preserved_unrelated_baselines(self):
  import argparse,hashlib
  p=self.packet();binary=self.root/'api';binary.write_bytes(b'ELF-synthetic-test-only');binary.chmod(0o700)
  p['futureApiBinaryBinding']=dict(path=str(binary),sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
  packet=self.root/'packet.json';F.write(packet,p);packet_sha=M.S.digest(M.S.read(packet));baseline={k:'a'*64 for k in ('boxd','boat','e2b','daytona')}
  boxd=dict(kind='boxd-project-cache-provider-observation-v1',runId=p['runId'],packetSha256=packet_sha,readOnly=True,
   completeProviderInventory=True,noProviderMutations=True,controlledProviderCounts=dict(boxd=1),protectedMachineId=p['bounds']['protectedMachineId'],
   protectedOriginalPresent=True,baseSnapshotPreserved=True,observedAtMs=self.now,unrelatedFingerprint='a'*64,unrelatedPreserved=True,
   preparationMachinesIndependentlyAbsent=True)
  proof=self.root/'boxd.json';F.write(proof,boxd)
  evidence=dict(independentlyVerified=True,boxdAndDurableScopeVerified=True,originalScopeVerified=True,runId=p['runId'],observedAtMs=self.now,controlledProviderCounts=dict(boxd=1,boat=0,e2b=0,daytona=0),
   protectedWorkspaceId=p['bounds']['protectedWorkspaceId'],protectedMachineId=p['bounds']['protectedMachineId'],protectedGeneration=1,
   initialMessageNeverLeased=True,protectedRetainedSha256=p['bounds']['protectedRetainedSha256'],ownershipPolicy='protected-original-and-exact-case-resources-v1',
   unrelatedBaselinesUnchanged=True,unrelatedBaselineSha256=baseline,boxdProviderProof=dict(path=str(proof),sha256=M.S.digest(M.S.read(proof))),
   preparationMachinesIndependentlyAbsent=True,apiSha256=p['futureApiBinaryBinding']['sha256'],preparationBackgroundDisabled=True,foreignPreparationRequests=0,
   foreignOwnedMachines=0,exclusiveLane=True)
  original=dict(readOnly=True,packetSha256=packet_sha,ownerId=p['source']['ownerId'],projectId=p['source']['projectId'],observerSha256=M.S.digest((HERE/'project-cache-rehearsal-observe.py').read_bytes()),observedAtMs=self.now,observation=dict(workspaces=[],requests=[],entries=[],operations=[],foreignCacheWork=0,foreignDueComputerJobs=0))
  sql=self.root/'sql.json';F.write(sql,original);evidence['originalSqlProof']=dict(path=str(sql),sha256=M.S.digest(M.S.read(sql)))
  others=dict(kind='boxd-cache-other-provider-observation-v1',runId=p['runId'],packetSha256=packet_sha,readOnly=True,noProviderMutations=True,oldestObservedAtMs=self.now,controlledProviderCounts=dict(boat=0,e2b=0,daytona=0),providers={k:dict(status='verified',code=None,unrelatedFingerprint='a'*64,unrelatedPreserved=True)for k in ('boat','e2b','daytona')})
  other=self.root/'other.json';F.write(other,others);evidence['otherProviderProof']=dict(path=str(other),sha256=M.S.digest(M.S.read(other)))
  inventory=self.root/'inventory.json';F.write(inventory,evidence)
  go=self.root/'go.json';F.write(go,dict(approved=True,packetSha256=packet_sha,runId=p['runId'],expiresAtMs=p['expiresAtMs'],unrelatedBaselineSha256=baseline))
  a=argparse.Namespace(go=go,packet=packet,inventory=inventory,state=self.root)
  self.assertEqual(R.authorize(a,p,False),p['futureApiBinaryBinding'])
  # Restricted noncase providers stay UNKNOWN and do not block this Boxd-only flow.
  for k in ('boat','e2b','daytona'):
   others['providers'][k]=dict(status='unknown',code='restricted_or_unavailable')
   others['controlledProviderCounts'][k]=None;evidence['controlledProviderCounts'][k]=None
  F.write(other,others);evidence['otherProviderProof']['sha256']=M.S.digest(M.S.read(other));F.write(inventory,evidence)
  self.assertEqual(R.authorize(a,p,False),p['futureApiBinaryBinding'])
  self.assertEqual(R.authorize(a,p,True),p['futureApiBinaryBinding'])
  evidence.pop('otherProviderProof');evidence['otherProviderObservationStatus']='unknown';evidence['otherProviderUnknownReason']='preflight-failed'
  F.write(inventory,evidence)
  self.assertEqual(R.authorize(a,p,False),p['futureApiBinaryBinding'])
  # Cleanup cannot use a foreign task, even when unrelated provider reads fail.
  original['observation']['workspaces']=[dict(id='foreign',idempotencyKey='foreign-key',machineId='foreign')]
  F.write(sql,original);evidence['originalSqlProof']['sha256']=M.S.digest(M.S.read(sql));F.write(inventory,evidence)
  with self.assertRaises(R.S.Rejected):R.authorize(a,p,True)
  original['observation']['workspaces']=[];F.write(sql,original);evidence['originalSqlProof']['sha256']=M.S.digest(M.S.read(sql));F.write(inventory,evidence)
  boxd['completeProviderInventory']=False;F.write(proof,boxd)
  evidence['boxdProviderProof']['sha256']=M.S.digest(M.S.read(proof));F.write(inventory,evidence)
  with self.assertRaises(R.S.Rejected):R.authorize(a,p,False)
 def test_delete_uses_actual_lifecycle_route_and_uncertain_action_cannot_resubmit(self):
  import argparse,types
  p=self.packet();state=self.root/'state';state.mkdir(mode=0o700);owner=self.root/'owner';owner.mkdir(mode=0o700)
  F.write(state/'create-hit.outcome.json',dict(value=dict(workspace=dict(projectId='synthetic-project',workspaceId='synthetic-task'))))
  calls=[]
  class Http:
   def request(self,method,url,token=None,body=None):
    calls.append((method,url,body));raise TimeoutError('lost reply')
  class Canary:
   def __init__(self,*args):self.root=owner;self.flow=types.SimpleNamespace(http=Http())
   def credential(self):return 'synthetic-bearer-must-never-persist'
  module=types.SimpleNamespace(Canary=Canary)
  spec=types.SimpleNamespace(loader=types.SimpleNamespace(exec_module=lambda m:None))
  a=argparse.Namespace(action='delete-hit',state=state,owner_dir=owner)
  with patch.object(R.importlib.util,'spec_from_file_location',return_value=spec),patch.object(R.importlib.util,'module_from_spec',return_value=module):
   with self.assertRaises(R.S.Rejected):R.locked(a,p,{},True)
   with self.assertRaises(R.S.Rejected):R.locked(a,p,{},True)
  self.assertEqual(len(calls),1);self.assertEqual(calls[0][0],'POST');self.assertTrue(calls[0][1].endswith('/synthetic-task/delete'))
  self.assertEqual(calls[0][2]['commandId'],p['commandIds']['delete-hit'])
  self.assertFalse(any('synthetic-bearer' in f.read_text()for f in state.glob('*.json')))
BASE=load('rehearsal_database',HERE/'tests/database.py')
O=load('rehearsal_observer',HERE/'project-cache-rehearsal-observe.py')
class ObserverPG(BASE.DatabaseBootstrapTest):
 def test_readonly_observer_uses_real_optional_schema_columns(self):
  db=M.ROLLOUT.DB
  db.execute(self.url,db.render('staging'))
  db.execute(self.url,"SET search_path=zuse_staging,pg_catalog;"+(db.MIGRATIONS/M.ROLLOUT.MIGRATION).read_text())
  p=dict(source=dict(ownerId='synthetic-owner',projectId='synthetic-project',actorId='synthetic-owner'),pilot=dict(baseReleaseId='synthetic-base'),bounds=dict(protectedWorkspaceId='synthetic-original'))
  sql=O.statement(p)
  self.assertTrue(sql.startswith('BEGIN READ ONLY;'))
  v=db.execute(self.url,sql)
  self.assertEqual(len(v),1)
  self.assertEqual(v[0],dict(requests=[],operations=[],entries=[],workspaces=[],allocations=[],sourceObjects=[],base=None,foreignCacheWork=0,foreignDueComputerJobs=0,cacheCertificates=0))
  # Executing a write inside the same READ ONLY envelope must be rejected.
  with self.assertRaises(ValueError):
   db.execute(self.url,"BEGIN READ ONLY; SET LOCAL search_path=zuse_staging,pg_catalog; INSERT INTO cloud_managed_bases(provider,release_id,artifact_id,architecture,capabilities,state,created_at) VALUES('boxd','forbidden','forbidden','x86_64','{}','preparing',0); COMMIT;")
if __name__=='__main__':unittest.main(defaultTest='Packet')
