#!/usr/bin/env python3
"""Reviewed staging R2 cutover tooling. Default is a private, effects-free dry run.

No key revocation, production writes, provider operations, or image changes.
Apply and rollback require an existing hash-pinned plan and fresh idle evidence.
"""
import argparse,fcntl,hashlib,importlib.util,json,logging,os,re,secrets,stat,subprocess,time,urllib.request
from pathlib import Path
HERE=Path(__file__).resolve().parent
VALIDATOR_SHA='6bf5aace3c76cf2c47398db5d2bc7e6cde6d9ec9eeb2daee5480fe2eb9399eaf'
def load(name,path):
 spec=importlib.util.spec_from_file_location(name,path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module
require_hash=lambda raw:hashlib.sha256(raw).hexdigest()
validator_path=HERE/'r2-scope-validation.py'
if require_hash(validator_path.read_bytes())!=VALIDATOR_SHA:raise RuntimeError('validator_source_mismatch')
scope=load('r2_scope_validation',validator_path)
require=scope.require
ROOT=Path('/etc/zuse/cloud');STAGE=ROOT/'staging';ACTIVE=STAGE/'storage.env'
OPS=STAGE/'r2-scope-activations'
KEYS={scope.PREFIX+field for field in scope.FIELDS}
IDLE_SQL="""BEGIN READ ONLY;SET LOCAL search_path=zuse_staging,pg_catalog;SET LOCAL statement_timeout='5s';
SELECT json_build_object('role',current_user,'schema',current_schema(),'workspaces',(SELECT count(*) FROM api_cloud_workspaces WHERE state<>'deleted'),'providerJobs',(SELECT count(*) FROM zuse_rust_jobs WHERE state IN ('pending','running') AND (kind LIKE 'workspace-%' OR kind IN ('mailbox-wake','task-command-drain'))),'recoveryOperations',(SELECT count(*) FROM zuse_rust_recovery_operations r JOIN api_cloud_workspaces w USING(workspace_id) WHERE w.state<>'deleted' AND r.phase NOT IN ('cancelled','restored','retired')));COMMIT;"""

def verify_idle_counts(counts):
 require(counts.get('role')=='zuse_cloud_staging' and counts.get('schema')=='zuse_staging','staging_database_identity_invalid')
 require(counts=={'role':'zuse_cloud_staging','schema':'zuse_staging','workspaces':0,'providerJobs':0,'recoveryOperations':0},'staging_not_idle')
 return counts

def raw_environment(raw):
 values={}
 for line in raw.decode().splitlines():
  if not line.strip() or line.lstrip().startswith('#'):continue
  key,sep,value=line.partition('=')
  require(sep and re.fullmatch('[A-Z][A-Z0-9_]*',key) and key not in values,'raw_env_invalid')
  require('\0' not in value and '\r' not in value,'raw_env_invalid');values[key]=value
 return values

def render_storage(baseline,candidate):
 # Candidate is shell-quoted; storage.env is Docker format:raw. Never source it.
 old=raw_environment(baseline)
 require(KEYS<=old.keys(),'active_r2_configuration_missing')
 for key in KEYS:
  value=candidate[key];require(isinstance(value,str) and value and not any(c in value for c in '\r\n\0'),'candidate_raw_value_invalid')
 result=[]
 for line in baseline.decode().splitlines(keepends=True):
  key=line.partition('=')[0]
  result.append(key+'='+candidate[key]+'\n' if key in KEYS else line)
 rendered=''.join(result).encode()
 parsed=raw_environment(rendered)
 require(all(parsed[key]==candidate[key] for key in KEYS),'raw_env_conversion_failed')
 require({k:v for k,v in old.items() if k not in KEYS}=={k:v for k,v in parsed.items() if k not in KEYS},'unrelated_setting_changed')
 return rendered

def verify_scope(candidate):
 candidate=Path(candidate)
 require(candidate.name=='candidate.env' and candidate.parent.parent==scope.CANDIDATES and not candidate.parent.is_symlink(),'candidate_path_not_pinned')
 raw=scope.private_read(candidate);values=scope.environment(candidate)
 journal_path=candidate.with_name('scope.intent.json');report_path=candidate.with_name('scope.report.json')
 journal_raw=scope.private_read(journal_path);report_raw=scope.private_read(report_path)
 journal=json.loads(journal_raw);report=json.loads(report_raw)
 require(report==scope.public_report(journal),'scope_report_mismatch')
 require(journal.get('version')==1 and journal.get('phase')=='finished' and journal.get('candidateSha256')==require_hash(raw),'scope_candidate_binding_invalid')
 require(journal.get('stagingBucket')==scope.STAGING and scope.own_key(journal.get('syntheticKey')),'scope_bucket_invalid')
 require(journal.get('productionHeads')==2 and journal.get('productionHeadStatus')==403 and journal.get('roundtripVerified') is True and journal.get('cleanupVerified') is True and journal.get('cleanupHeadStatus')==404 and journal.get('putRequests')==1 and journal.get('bytes')==256 and not journal.get('putUnobserved',False) and journal.get('errorCode') is None and journal.get('cleanupError') is None,'scope_not_successful')
 require(report.get('status')=='passed' and report.get('productionGets')==0 and report.get('productionWrites')==0,'scope_not_successful')
 return values,{'candidateSha256':require_hash(raw),'scopeIntentSha256':require_hash(journal_raw),'scopeReportSha256':require_hash(report_raw)}

def atomic_bytes(path,data,expected=None):
 current=scope.private_read(path)
 require(expected is None or require_hash(current)==expected,'active_storage_changed')
 temporary=path.with_name(path.name+'.next-'+secrets.token_hex(8))
 fd=os.open(temporary,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
 try:
  with os.fdopen(fd,'wb') as out:out.write(data);out.flush();os.fsync(out.fileno())
  os.replace(temporary,path)
  directory=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY)
  try:os.fsync(directory)
  finally:os.close(directory)
 finally:temporary.unlink(missing_ok=True)

def backup_bytes(path,data):
 fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
 with os.fdopen(fd,'wb') as out:out.write(data);out.flush();os.fsync(out.fileno())

def provider_absence(path,fingerprint):
 raw=scope.private_read(path);require(require_hash(raw)==fingerprint,'provider_inventory_changed')
 proof=json.loads(raw)
 require(proof.get('version')==1 and proof.get('environment')=='staging' and proof.get('ownershipScope')=='zuse-owned-staging-canaries-and-publishers' and proof.get('independentlyVerified') is True,'provider_inventory_invalid')
 require(isinstance(proof.get('observedAtMs'),int) and 0<=int(time.time()*1000)-proof['observedAtMs']<=60000,'provider_inventory_stale')
 require(proof.get('activeOwnedResources')==0 and proof.get('uncertainOperations')==0 and proof.get('activeRecoveryOperations')==0,'provider_resources_active')
 require(proof.get('providerCounts')==dict.fromkeys(('boxd','boat','e2b','daytona'),0),'provider_inventory_incomplete')
 return require_hash(raw)

def positive_roundtrip(client,path,fingerprint):
 # Reuse the reviewed SDK head/body/ownership cleanup code. Exactly one PUT;
 # uncertain effects remain in the original private journal, never blindly retry.
 header=b'zuse-staging-activation-v1\n';body=header+secrets.token_bytes(256-len(header))
 journal={'version':1,'phase':'intent','stagingBucket':scope.STAGING,'candidateSha256':fingerprint,'syntheticKey':'scope-verification/'+secrets.token_hex(16)+'/roundtrip.bin','bytes':256,'syntheticSha256':require_hash(body),'putRequests':0,'cleanupVerified':False}
 scope.private_write(path,journal,initial=True);save=lambda:scope.private_write(path,journal)
 eligible=False
 try:
  require(scope.head(client,scope.STAGING,journal['syntheticKey'])[0]==404,'activation_probe_collision')
  journal['phase']='putting';journal['putRequests']=1;save();eligible=True
  try:client.put_object(Bucket=scope.STAGING,Key=journal['syntheticKey'],Body=body,ContentLength=256,IfNoneMatch='*');journal['putReply']='completed'
  except Exception:journal['putReply']='uncertain';journal['putUnobserved']=True
  save()
  require(scope.head(client,scope.STAGING,journal['syntheticKey'])[0]==200,'activation_put_unobserved')
  require(scope.bounded_body(client.get_object(Bucket=scope.STAGING,Key=journal['syntheticKey']),256)==body,'activation_get_mismatch')
  journal['putUnobserved']=False;journal['roundtripVerified']=True;save()
 finally:
  if eligible:scope.cleanup(client,journal,save)
 require(journal.get('roundtripVerified') and journal.get('cleanupVerified') and journal.get('cleanupHeadStatus')==404,'activation_probe_not_clean')
 return {'bytes':256,'putRequests':1,'roundtripVerified':True,'cleanupHeadStatus':404}

class Host:
 def __init__(self):
  self.deploy=load('r2_deploy',HERE/'single-host/deploy.py');self.database=load('r2_database',HERE/'database.py')
  self.command,self.env=self.deploy.compose(ROOT,'staging')
  # Cutover is local to this host; inherited Docker contexts cannot retarget it.
  for key in ('DOCKER_CONTEXT','DOCKER_TLS_VERIFY','DOCKER_CERT_PATH'):self.env.pop(key,None)
  self.env['DOCKER_HOST']='unix:///var/run/docker.sock'
 def run(self,args,**kwargs):
  try:result=subprocess.run(args,capture_output=True,text=True,timeout=90,env=self.env,**kwargs)
  except Exception:raise scope.Rejected('host_command_unavailable') from None
  require(result.returncode==0,'host_command_failed');return result.stdout
 def pins(self):
  # Hash private source bytes, never serialize their values.
  paths=[STAGE/name for name in ('deploy.env','runtime.env','api.env','worker.env')]
  paths += [HERE/'single-host'/name for name in ('deploy.py','compose.yaml','github-api.yaml','account-events.yaml')]
  paths += [HERE/'database.py',validator_path,Path(__file__).resolve()]
  hashes={str(path):require_hash(scope.private_read(path) if str(path).startswith(str(ROOT)) else path.read_bytes()) for path in paths}
  stage=raw_environment(scope.private_read(STAGE/'deploy.env'))
  require(stage.get('ZUSE_ENV')=='staging','not_staging')
  effective=raw_environment(scope.private_read(STAGE/'runtime.env'));effective.update(raw_environment(scope.private_read(STAGE/'api.env')))
  return {'files':hashes,'image':stage['ZUSE_CLOUD_IMAGE']}
 def containers(self):
  ids=self.run(['docker','ps','-aq','--filter','label=com.docker.compose.project=zuse-staging']).split()
  ids+=self.run(['docker','ps','-aq','--filter','label=com.docker.compose.project=zuse-production']).split()
  require(ids,'containers_missing')
  rows=json.loads(self.run(['docker','inspect',*ids]));result={}
  for row in rows:
   labels=row['Config'].get('Labels') or {};project=labels.get('com.docker.compose.project');service=labels.get('com.docker.compose.service')
   require(project in ('zuse-staging','zuse-production'),'container_project_invalid')
   key=project+':'+service;require(key not in result,'replicated_service_requires_review')
   result[key]={'id':row['Id'],'image':row['Image'],'configuredImage':row['Config']['Image'],'startedAt':row['State']['StartedAt'],'running':row['State']['Running']}
  require(result.get('zuse-staging:auth-worker',{}).get('running'),'auth_worker_not_running')
  require(any(key.startswith('zuse-production:') for key in result),'production_identity_missing')
  return result
 def idle(self):
  runtime=raw_environment(scope.private_read(STAGE/'runtime.env'));url=runtime.get('ZUSE_CLOUD_DATABASE_URL','')
  require(self.deploy.isolated_schema_url(url,'staging'),'staging_database_identity_invalid')
  env=self.database.connection_environment(url)
  # Use the deployed verified CA, retaining verify-full and explicit stage path.
  env['PGSSLROOTCERT']='/etc/zuse/cloud/certs/supabase-ca.crt'
  try:r=subprocess.run(['psql','-X','--no-password','-Atq','-v','ON_ERROR_STOP=1'],input=IDLE_SQL,env=env,capture_output=True,text=True,timeout=15)
  except Exception:raise scope.Rejected('idle_database_unavailable') from None
  require(r.returncode==0,'idle_database_unavailable')
  rows=[json.loads(line) for line in r.stdout.splitlines() if line.startswith('{')];require(len(rows)==1,'idle_database_invalid');counts=rows[0]
  return verify_idle_counts(counts)
 def verify_image(self,pins,containers):
  current=self.run(['docker','image','inspect',pins['image'],'--format','{{.Id}}']).strip()
  require(all(containers['zuse-staging:'+service]['image']==current for service in ('api','worker')),'local_image_pin_changed')
 def stop(self):self.run(self.command+['stop','--timeout','30','api','worker'])
 def restart(self):self.run(self.command+['up','-d','--no-deps','--no-build','--pull','never','--force-recreate','api','worker'])
 def health(self):
  port=raw_environment(scope.private_read(STAGE/'deploy.env')).get('APP_HOST_PORT','')
  require(port.isdigit() and 1024<=int(port)<=65535,'staging_health_port_invalid')
  deadline=time.monotonic()+60
  while time.monotonic()<deadline:
   try:
    for route in ('healthz','readyz'):
     with urllib.request.urlopen(f'http://127.0.0.1:{port}/{route}',timeout=3) as response:require(response.status==200,'health_not_ready')
    return
   except Exception:time.sleep(1)
  raise scope.Rejected('staging_health_failed')

def protected(rows):return {k:v for k,v in rows.items() if k.startswith('zuse-production:') or k=='zuse-staging:auth-worker'}
def verify_containers(original,current,pins,running=True):
 require(protected(original)==protected(current),'protected_service_changed')
 for service in ('api','worker'):
  key='zuse-staging:'+service
  if not running and key not in current:continue
  require(key in current and current[key]['image']==original[key]['image'] and current[key]['configuredImage']==pins['image'],'staging_image_changed')
  require(not running or current[key]['running'],'staging_service_not_running')

def prepare(candidate,operation,host):
 values,binding=verify_scope(candidate);binding['controlInventorySha256']=require_hash(scope.private_read(scope.INVENTORY));baseline=scope.private_read(ACTIVE)
 old=raw_environment(baseline)
 require(old.get(scope.PREFIX+'BUCKET')==scope.STAGING and old.get(scope.PREFIX+'ENDPOINT','').rstrip('/')==values[scope.PREFIX+'ENDPOINT'],'active_scope_mismatch')
 rendered=render_storage(baseline,values);pins=host.pins();containers=host.containers();host.idle();verify_containers(containers,containers,pins);host.verify_image(pins,containers)
 require(not operation.exists(),'operation_exists_inspect_original')
 operation.mkdir(mode=0o700)
 plan={'version':1,'environment':'staging','candidate':str(candidate),'activePath':str(ACTIVE),'activeSha256':require_hash(baseline),'renderedSha256':require_hash(rendered),'binding':binding,'pins':pins,'containers':containers,'phase':'prepared','dryRun':True,'createdAtMs':int(time.time()*1000)}
 scope.private_write(operation/'intent.json',plan,initial=True)
 return {'status':'dry-run-ready','planSha256':require_hash(scope.private_read(operation/'intent.json')),'servicesChanged':False,'credentialsActivated':False,'providerAbsenceRequiredAtApply':True}

def execute(operation,plan_sha,inventory,inventory_sha,host,rollback=False):
 require(operation.is_dir() and not operation.is_symlink() and operation.stat().st_uid==os.getuid() and operation.stat().st_mode&0o077==0,'operation_directory_unsafe')
 path=operation/'intent.json';raw=scope.private_read(path);require(require_hash(raw)==plan_sha,'activation_plan_changed')
 plan=json.loads(raw);require(plan.get('version')==1 and plan.get('environment')=='staging' and plan.get('activePath')==str(ACTIVE),'activation_scope_invalid')
 values,binding=verify_scope(Path(plan['candidate']));binding['controlInventorySha256']=require_hash(scope.private_read(scope.INVENTORY));require(binding==plan['binding'],'candidate_proof_changed')
 require(host.pins()==plan['pins'],'deployment_configuration_changed')
 current=host.containers();verify_containers(plan['containers'],current,plan['pins']);host.verify_image(plan['pins'],plan['containers']);require(rollback or current==plan['containers'],'active_services_changed');host.idle();provider_absence(inventory,inventory_sha)
 baseline=scope.private_read(ACTIVE)
 require(require_hash(baseline)==(plan['renderedSha256'] if rollback else plan['activeSha256']),'active_storage_changed')
 require(rollback or plan['phase']=='prepared','original_activation_requires_inspection')
 backup=operation/'storage.before.env'
 if not rollback:backup_bytes(backup,baseline)
 else:require(require_hash(scope.private_read(backup))==plan['activeSha256'],'rollback_backup_changed')
 def save(phase,**fields):plan.update(phase=phase,**fields);scope.private_write(path,plan)
 save('rolling-back' if rollback else 'stopping',dryRun=False)
 try:
  host.stop();host.idle();provider_absence(inventory,inventory_sha)
  require(host.pins()==plan['pins'] and require_hash(scope.private_read(ACTIVE))==(plan['renderedSha256'] if rollback else plan['activeSha256']),'prewrite_fence_changed')
  rendered=scope.private_read(backup) if rollback else render_storage(baseline,values)
  require(require_hash(rendered)==(plan['activeSha256'] if rollback else plan['renderedSha256']),'rendered_configuration_changed')
  save('restoring' if rollback else 'activating')
  atomic_bytes(ACTIVE,rendered,plan['renderedSha256'] if rollback else plan['activeSha256'])
  active_values=raw_environment(rendered)
  if rollback:probe=positive_roundtrip(scope.sdk(active_values),operation/'rollback-object.intent.json',require_hash(rendered))
  else:
   report=scope.validate(scope.sdk(active_values),scope.sdk(raw_environment(scope.private_read(backup))),scope.control_object(scope.INVENTORY),operation/'active-scope.intent.json',binding['candidateSha256'])
   scope.private_write(operation/'active-scope.report.json',report,initial=True);require(report['status']=='passed','active_scope_validation_failed');probe={'bytes':256,'cleanupHeadStatus':404,'productionHeadStatus':403}
  host.verify_image(plan['pins'],plan['containers']);host.restart();host.health();verify_containers(plan['containers'],host.containers(),plan['pins']);require(host.pins()==plan['pins'],'deployment_configuration_changed');host.idle()
  save('rolled-back' if rollback else 'complete',objectProof=probe)
  return {'status':plan['phase'],'stageOnly':True,'productionChanged':False,'keysRevoked':False,'objectProof':probe}
 except Exception:
  # Automatic rollback never overwrites someone else's config or changes images.
  if not rollback and require_hash(scope.private_read(ACTIVE)) in (plan['activeSha256'],plan['renderedSha256']) and host.pins()==plan['pins']:
   try:
    verify_containers(plan['containers'],host.containers(),plan['pins'],running=False);host.idle();host.stop();host.idle();atomic_bytes(ACTIVE,scope.private_read(backup))
    positive_roundtrip(scope.sdk(raw_environment(scope.private_read(backup))),operation/'rollback-object.intent.json',plan['activeSha256'])
    host.verify_image(plan['pins'],plan['containers']);host.restart();host.health();verify_containers(plan['containers'],host.containers(),plan['pins']);host.idle();save('rolled-back',errorCode='activation_failed')
   except Exception:save('inspection-required',errorCode='rollback_not_verified')
  else:save('inspection-required',errorCode='activation_not_verified')
  raise scope.Rejected('activation_failed_'+plan['phase']) from None

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--candidate',type=Path);p.add_argument('--operation',type=Path,required=True);p.add_argument('--apply',action='store_true');p.add_argument('--rollback',action='store_true');p.add_argument('--plan-sha256');p.add_argument('--provider-absence',type=Path);p.add_argument('--provider-absence-sha256');a=p.parse_args()
 logging.disable(logging.CRITICAL);os.umask(0o077);require(os.geteuid()==0,'root_required')
 require(a.operation.parent==OPS and re.fullmatch('[a-f0-9]{32}',a.operation.name),'operation_path_not_pinned');require(not(a.apply and a.rollback),'action_invalid')
 require(OPS.is_dir() and not OPS.is_symlink() and OPS.stat().st_mode&0o077==0 and OPS.stat().st_uid==0,'operation_parent_unsafe')
 lock=os.open(STAGE/'r2-scope-activation.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
 with os.fdopen(lock,'w') as held:
  fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB);host=Host()
  if a.apply or a.rollback:
   require(a.plan_sha256 and a.provider_absence and a.provider_absence_sha256,'reviewed_plan_and_inventory_required')
   report=execute(a.operation,a.plan_sha256,a.provider_absence,a.provider_absence_sha256,host,a.rollback)
  else:
   require(a.candidate is not None,'candidate_required');report=prepare(a.candidate,a.operation,host)
 print(json.dumps(report,sort_keys=True))
if __name__=='__main__':
 try:main()
 except scope.Rejected as error:
  code=str(error);code=code if re.fullmatch('[a-z_]+',code) else 'activation_preflight_or_operation_failed'
  print(json.dumps({'status':'rejected','errorCode':code,'secretsPrinted':False}));raise SystemExit(1)
 except Exception:print(json.dumps({'status':'rejected','errorCode':'activation_preflight_or_operation_failed','secretsPrinted':False}));raise SystemExit(1)
