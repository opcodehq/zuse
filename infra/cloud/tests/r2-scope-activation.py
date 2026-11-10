#!/usr/bin/env python3
import copy,importlib.util,json,os,shlex,tempfile,time,unittest
from pathlib import Path
from unittest.mock import patch
HERE=Path(__file__).resolve().parents[1]
def load(name,path):
 spec=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
m=load('activation',HERE/'r2-scope-activation.py');fake=load('fake_scope',HERE/'tests/r2-scope-validation.py')
class Host:
 def __init__(self):
  self.events=[];self.fail_health=0;self.fail_idle_at=None;self.idles=0;self.stopped=False;self.image_tag="image-id"
  self.settings={'files':{'stage-api':'sha','stage-runtime':'cohort-pinned'},'image':'exact-image'}
  self.rows={key:{'id':key,'image':'image-id','configuredImage':'exact-image','startedAt':'original','running':True} for key in ('zuse-staging:api','zuse-staging:worker','zuse-staging:auth-worker','zuse-production:api')}
 def pins(self):return copy.deepcopy(self.settings)
 def containers(self):return copy.deepcopy(self.rows)
 def idle(self):
  self.idles+=1;self.events.append('idle')
  if self.fail_idle_at is not None and self.idles>=self.fail_idle_at:raise m.scope.Rejected('staging_not_idle')
  return {'environment':'staging','workspaces':0,'providerJobs':0,'recoveryOperations':0}
 def verify_image(self,pins,containers):
  m.require(self.image_tag==containers['zuse-staging:api']['image'],'local_image_pin_changed')
 def stop(self):self.events.append('stop');self.stopped=True
 def restart(self):self.events.append('restart');self.stopped=False
 def health(self):
  self.events.append('health')
  if self.fail_health:self.fail_health-=1;raise m.scope.Rejected('health_failed')
class Activation(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.candidates=self.root/'candidates';self.candidates.mkdir(mode=0o700);self.folder=self.candidates/'candidate.fixture';self.folder.mkdir(mode=0o700)
  self.active=self.root/'storage.env';self.candidate=self.folder/'candidate.env';self.operation=self.root/'operation';self.inventory=self.root/'provider.json'
  self.control_inventory=self.root/'production-control.private.json';self.control_inventory.write_text(json.dumps({'sourceBucket':m.scope.PRODUCTION,'entries':[{'sourceKey':fake.KEY,'sourceETag':fake.KNOWN['etag'],'bytes':fake.KNOWN['bytes']}]}));self.control_inventory.chmod(0o600)
  self.old=("# keep exact unrelated settings\nCLOUD_TRANSCRIPT_R2_ENDPOINT=https://example.r2.cloudflarestorage.com\nCLOUD_TRANSCRIPT_R2_BUCKET=zuse-rust-staging\nCLOUD_TRANSCRIPT_R2_ACCESS_KEY_ID=old-id\nCLOUD_TRANSCRIPT_R2_SECRET_ACCESS_KEY=old-secret\nUNRELATED=$literal ' quote # value\n").encode()
  self.active.write_bytes(self.old);self.active.chmod(0o600)
  self.values={m.scope.PREFIX+k:v for k,v in {'ENDPOINT':'https://example.r2.cloudflarestorage.com','BUCKET':'zuse-rust-staging','ACCESS_KEY_ID':'new-id','SECRET_ACCESS_KEY':'''a b'$"#literal\\secret'''}.items()}
  self.candidate.write_text(''.join(k+'='+shlex.quote(v)+'\n' for k,v in self.values.items()));self.candidate.chmod(0o600)
  self.client=fake.Candidate();self.control=fake.Control();self.old_client=fake.Candidate()
  report=m.scope.validate(self.client,self.control,fake.KNOWN,self.folder/'scope.intent.json',m.require_hash(self.candidate.read_bytes()))
  m.scope.private_write(self.folder/'scope.report.json',report,initial=True)
  self.write_inventory();self.host=Host()
  self.patches=[patch.object(m,'ACTIVE',self.active),patch.object(m.scope,'CANDIDATES',self.candidates),patch.object(m.scope,'INVENTORY',self.control_inventory),patch.object(m.scope,'sdk',self.sdk),patch.object(m.scope,'control_object',lambda _:fake.KNOWN)]
  for p in self.patches:p.start()
 def tearDown(self):
  for p in reversed(self.patches):p.stop()
  self.tmp.cleanup()
 def sdk(self,values):
  self.assertTrue(self.host.stopped,'scope PUT/GET proof occurs only inside API/worker maintenance')
  if values[m.scope.PREFIX+'ACCESS_KEY_ID']=='old-id':
   owner=self
   class Both:
    def head_object(_,**args):return owner.control.head_object(**args) if args['Bucket']==m.scope.PRODUCTION else owner.old_client.head_object(**args)
    def __getattr__(_,key):return getattr(owner.old_client,key)
   return Both()
  return self.client
 def write_inventory(self,**changes):
  proof={'version':1,'environment':'staging','ownershipScope':'zuse-owned-staging-canaries-and-publishers','independentlyVerified':True,'observedAtMs':int(time.time()*1000),'activeOwnedResources':0,'uncertainOperations':0,'activeRecoveryOperations':0,'providerCounts':dict.fromkeys(('boxd','boat','e2b','daytona'),0)};proof.update(changes)
  self.inventory.write_text(json.dumps(proof));self.inventory.chmod(0o600)
 def prepare(self):return m.prepare(self.candidate,self.operation,self.host)['planSha256']
 def apply(self,sha):return m.execute(self.operation,sha,self.inventory,m.require_hash(self.inventory.read_bytes()),self.host)
 def test_default_dryrun_pins_proofs_preserves_all_active_bytes_no_services_or_sdk(self):
  self.prepare();self.assertEqual(self.active.read_bytes(),self.old);self.assertNotIn('stop',self.host.events);self.assertNotIn('restart',self.host.events);self.assertFalse((self.operation/'storage.before.env').exists())
 def test_success_converts_shell_quotes_to_raw_and_only_restarts_stage_after_key_proof(self):
  sha=self.prepare();report=self.apply(sha)
  self.assertEqual(report['status'],'complete');self.assertEqual(m.raw_environment(self.active.read_bytes())[m.scope.PREFIX+'SECRET_ACCESS_KEY'],self.values[m.scope.PREFIX+'SECRET_ACCESS_KEY'])
  self.assertIn(b"UNRELATED=$literal ' quote # value\n",self.active.read_bytes());self.assertEqual((self.operation/'storage.before.env').read_bytes(),self.old)
  self.assertEqual(self.active.stat().st_mode&0o777,0o600);self.assertEqual((self.operation/'storage.before.env').stat().st_mode&0o777,0o600)
  self.assertEqual(self.host.events.count('stop'),1);self.assertEqual(self.host.events.count('restart'),1)
  self.assertTrue(all(call[0]=='HEAD' for call in self.client.calls if call[1]==m.scope.PRODUCTION));self.assertTrue(all(call[0]=='HEAD' for call in self.control.calls));self.assertFalse(self.client.data)
 def test_changed_candidate_or_report_or_journal_rejected_before_stop(self):
  for leaf in ('candidate.env','scope.report.json','scope.intent.json'):
   with self.subTest(leaf=leaf):
    sha=self.prepare();path=self.folder/leaf;original=path.read_bytes();path.write_bytes(original+b' ')
    with self.assertRaises(m.scope.Rejected):self.apply(sha)
    self.assertNotIn('stop',self.host.events);path.write_bytes(original)
    import shutil;shutil.rmtree(self.operation)
 def test_changed_active_env_pins_or_service_identity_rejected_before_stop(self):
  sha=self.prepare();self.active.write_bytes(self.old+b'NEW_SETTING=changed\n')
  with self.assertRaisesRegex(m.scope.Rejected,'active_storage_changed'):self.apply(sha)
  self.active.write_bytes(self.old);self.host.settings['image']='different'
  with self.assertRaisesRegex(m.scope.Rejected,'deployment_configuration_changed'):self.apply(sha)
  self.host.settings['image']='exact-image';self.host.rows['zuse-staging:api']['id']='new-container'
  with self.assertRaisesRegex(m.scope.Rejected,'active_services_changed'):self.apply(sha)
  self.assertNotIn('stop',self.host.events)
 def test_tampered_plan_not_replayed_and_production_never_targeted(self):
  sha=self.prepare();path=self.operation/'intent.json';plan=json.loads(path.read_bytes());plan['environment']='production';m.scope.private_write(path,plan)
  with self.assertRaisesRegex(m.scope.Rejected,'activation_plan_changed'):self.apply(sha)
  with self.assertRaisesRegex(m.scope.Rejected,'activation_scope_invalid'):self.apply(m.require_hash(path.read_bytes()))
  self.assertEqual(self.active.read_bytes(),self.old);self.assertNotIn('stop',self.host.events)
 def test_known_prod_404_or_200_fails_and_restores_backup_without_revocation(self):
  for prod in (200,404):
   with self.subTest(prod=prod):
    sha=self.prepare();self.client.prod=prod
    # Rollback uses a staging-only client, not a production-denial claim.
    old=fake.Candidate();real_sdk=self.sdk
    def sdk(values):
     if values[m.scope.PREFIX+'ACCESS_KEY_ID']=='old-id':
      # Scope validation control still must HEAD the known production object.
      class Both:
       def head_object(_,**args):return self.control.head_object(**args) if args['Bucket']==m.scope.PRODUCTION else old.head_object(**args)
       def __getattr__(_,key):return getattr(old,key)
      return Both()
     return real_sdk(values)
    with patch.object(m.scope,'sdk',sdk):
     with self.assertRaisesRegex(m.scope.Rejected,'activation_failed_rolled-back'):self.apply(sha)
    self.assertEqual(self.active.read_bytes(),self.old);self.assertEqual(json.loads((self.operation/'intent.json').read_bytes())['phase'],'rolled-back')
    import shutil;shutil.rmtree(self.operation);self.host=Host()
 def test_idle_or_owned_provider_resources_prevent_any_service_stop(self):
  sha=self.prepare()
  for changes in ({'activeOwnedResources':1},{'observedAtMs':0},{'ownershipScope':'entire-account'},{'providerCounts':{'boxd':0}}):
   self.write_inventory(**changes)
   with self.assertRaises(m.scope.Rejected):self.apply(sha)
   self.assertNotIn('stop',self.host.events)
 def test_zero_idle_after_stop_race_never_installs_candidate(self):
  sha=self.prepare();self.host.fail_idle_at=3
  with self.assertRaises(m.scope.Rejected):self.apply(sha)
  self.assertEqual(self.active.read_bytes(),self.old);self.assertEqual(json.loads((self.operation/'intent.json').read_bytes())['phase'],'inspection-required');self.assertEqual(self.host.events.count('stop'),1)
 def test_raw_conversion_rejects_newline_and_preserves_literal_metacharacters(self):
  bad=dict(self.values);bad[m.scope.PREFIX+'SECRET_ACCESS_KEY']='bad\nNEW_KEY=oops'
  with self.assertRaisesRegex(m.scope.Rejected,'candidate_raw_value_invalid'):m.render_storage(self.old,bad)
  parsed=m.raw_environment(m.render_storage(self.old,self.values));self.assertEqual(parsed[m.scope.PREFIX+'SECRET_ACCESS_KEY'],self.values[m.scope.PREFIX+'SECRET_ACCESS_KEY'])
 def test_explicit_rollback_uses_same_pinned_image_and_original_bytes(self):
  self.apply(self.prepare());sha=m.require_hash((self.operation/'intent.json').read_bytes());old=fake.Candidate()
  with patch.object(m.scope,'sdk',lambda _:old):report=m.execute(self.operation,sha,self.inventory,m.require_hash(self.inventory.read_bytes()),self.host,rollback=True)
  self.assertEqual(report['status'],'rolled-back');self.assertEqual(self.active.read_bytes(),self.old);self.assertFalse(old.data)
 def test_health_failure_automatically_restores_old_bytes_and_same_services(self):
  sha=self.prepare();self.host.fail_health=1
  with self.assertRaisesRegex(m.scope.Rejected,'activation_failed_rolled-back'):self.apply(sha)
  self.assertEqual(self.active.read_bytes(),self.old);self.assertEqual(self.host.events.count('stop'),2);self.assertEqual(self.host.events.count('restart'),2)
  self.assertEqual(json.loads((self.operation/'intent.json').read_bytes())['phase'],'rolled-back');self.assertFalse(self.old_client.data)
 def test_completed_original_operation_cannot_be_reapplied(self):
  self.apply(self.prepare());stops=self.host.events.count('stop');sha=m.require_hash((self.operation/'intent.json').read_bytes())
  with self.assertRaises(m.scope.Rejected):self.apply(sha)
  self.assertEqual(self.host.events.count('stop'),stops)
 def test_production_bucket_candidate_is_refused_before_plan_or_service_effects(self):
  self.candidate.write_text(self.candidate.read_text().replace('zuse-rust-staging','zuse-cloud-transcripts'))
  with self.assertRaisesRegex(m.scope.Rejected,'staging_bucket_not_pinned'):self.prepare()
  self.assertFalse(self.operation.exists());self.assertNotIn('stop',self.host.events)
 def test_changed_production_control_inventory_is_rejected_before_stop(self):
  sha=self.prepare();self.control_inventory.write_text('{}')
  with self.assertRaisesRegex(m.scope.Rejected,'candidate_proof_changed'):self.apply(sha)
  self.assertNotIn('stop',self.host.events)
 def test_changed_local_image_tag_is_rejected_before_service_stop(self):
  sha=self.prepare();self.host.image_tag='different-image-id'
  with self.assertRaisesRegex(m.scope.Rejected,'local_image_pin_changed'):self.apply(sha)
  self.assertNotIn('stop',self.host.events)
 def test_actual_host_idle_query_uses_restricted_role_identity_not_admin_metadata(self):
  from types import SimpleNamespace
  host=object.__new__(m.Host);seen={}
  host.deploy=SimpleNamespace(isolated_schema_url=lambda url,environment:environment=='staging')
  host.database=SimpleNamespace(connection_environment=lambda url:{'PGUSER':'zuse_cloud_staging','PGSSLMODE':'verify-full'})
  runtime=self.root/'runtime.env';runtime.write_text('ZUSE_CLOUD_DATABASE_URL=private-dsn-not-an-argument\n');runtime.chmod(0o600)
  def run(args,**kwargs):
   seen.update(args=args,**kwargs)
   return SimpleNamespace(returncode=0,stdout=json.dumps({'role':'zuse_cloud_staging','schema':'zuse_staging','workspaces':1,'providerJobs':0,'recoveryOperations':0})+'\n')
  with patch.object(m,'STAGE',self.root),patch.object(m.subprocess,'run',run):
   with self.assertRaisesRegex(m.scope.Rejected,'staging_not_idle'):host.idle()
  self.assertIn('BEGIN READ ONLY',seen['input']);self.assertIn('current_user',seen['input']);self.assertIn('current_schema()',seen['input']);self.assertNotIn('zuse_cloud_schema_identity',seen['input']);self.assertNotIn('private-dsn-not-an-argument',str(seen['args']))
  self.assertEqual(seen['env']['PGSSLMODE'],'verify-full');self.assertEqual(seen['env']['PGSSLROOTCERT'],'/etc/zuse/cloud/certs/supabase-ca.crt')
 def test_idle_result_rejects_admin_role_or_production_schema_even_with_zero_counts(self):
  good={'role':'zuse_cloud_staging','schema':'zuse_staging','workspaces':0,'providerJobs':0,'recoveryOperations':0}
  self.assertEqual(m.verify_idle_counts(good),good)
  for key,value in [('role','postgres'),('schema','zuse_production')]:
   with self.assertRaisesRegex(m.scope.Rejected,'staging_database_identity_invalid'):m.verify_idle_counts(dict(good,**{key:value}))
 def test_real_host_service_commands_are_stage_only_no_build_pull_or_dependencies(self):
  host=object.__new__(m.Host);host.command=['docker','compose','--project-name','zuse-staging','--file','pinned-compose'];commands=[];host.run=lambda args:commands.append(args)
  host.stop();host.restart()
  self.assertEqual(commands[0][-5:],['stop','--timeout','30','api','worker'])
  self.assertEqual(commands[1][-2:],['api','worker']);self.assertIn('--no-deps',commands[1]);self.assertIn('--no-build',commands[1]);self.assertEqual(commands[1][commands[1].index('--pull')+1],'never')
  self.assertNotIn('auth-worker',commands[1]);self.assertNotIn('zuse-production',commands[1])
 def test_protected_auth_or_production_change_rejects(self):
  sha=self.prepare()
  for key in ('zuse-staging:auth-worker','zuse-production:api'):
   original=self.host.rows[key]['id'];self.host.rows[key]['id']='changed'
   with self.assertRaisesRegex(m.scope.Rejected,'protected_service_changed'):self.apply(sha)
   self.host.rows[key]['id']=original
  self.assertNotIn('stop',self.host.events)
if __name__=='__main__':unittest.main()
