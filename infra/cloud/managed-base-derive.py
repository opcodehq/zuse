#!/usr/bin/env python3
"""Validate a restored, credential-free managed base before a guest-only overlay.

The pinned parent proof is an operator input, never a task image or a certification.
No network, credential import, account login, deletion or provider operation occurs.
"""
import argparse, hashlib, importlib.util, json, os, stat, subprocess, re, time, contextlib, errno
from pathlib import Path

SKIP = {'claude', 'codex', 'grok', 'codex-assets.tar'}
INSTALLED = {'runtime':'/usr/local/bin/zuse-cloud-runtime',
 'harness-release.py':'/usr/local/lib/zuse/harness-release.py',
 'launch-runtime.sh':'/usr/local/lib/zuse/launch-rust-runtime-core',
 'launch-recovery.sh':'/usr/local/lib/zuse/launch-rust-recovery',
 'runtime-supervisor.py':'/usr/local/lib/zuse/runtime-supervisor.py',
 'runtime-repair.py':'/usr/local/lib/zuse/runtime-repair.py',
 'runtime-recreation.py':'/usr/local/lib/zuse/runtime-recreation.py'}
DISABLED_PARENT_MODE='post-hydration-first-start-v1'
DISABLED_IMAGE_PINS={
 '/usr/local/bin/zuse-cloud-runtime':('runtimeSha256',False,0o755),
 '/usr/local/lib/zuse/runtime-supervisor.py':('supervisorSha256',False,0o555),
 '/usr/local/lib/zuse/managed-base-fresh-boot.py':('helperSha256',False,0o555),
 '/etc/systemd/system/zuse-runtime-supervisor.service':('serviceSha256',False,0o644),
 '/opt/zuse/preparation/status/manifest.json':('manifestSha256',True,0o600),
}

def canonical(value):return json.dumps(value,sort_keys=True,separators=(',',':')).encode()
def digest(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  for block in iter(lambda:f.read(1024*1024),b''):h.update(block)
 return h.hexdigest()
def require(ok,code):
 if not ok:raise ValueError(code)
class InspectionFailure(ValueError):
 def __init__(self,stage,error,elapsed_ms):
  super().__init__('parent_preflight_failed')
  self.facts=dict(stage=stage,errorClass=type(error).__name__,elapsedMs=elapsed_ms)
  if isinstance(error,OSError) and error.errno is not None:self.facts['errno']=errno.errorcode.get(error.errno,'UNKNOWN')
  if isinstance(error,subprocess.CalledProcessError):self.facts['exitStatus']=error.returncode
@contextlib.contextmanager
def inspection(stage):
 start=time.monotonic()
 try:yield
 except (OSError,subprocess.SubprocessError) as error:
  raise InspectionFailure(stage,error,round((time.monotonic()-start)*1000,2)) from None

PARENT_PING_TIMEOUT_SECONDS=15 # Supervisor connect3 + read10 + process-start margin.
def read_parent_ping(*,run=subprocess.run,timeout=PARENT_PING_TIMEOUT_SECONDS):
 # Observation only. Never launch/restart/stop a resident or retry a mutation.
 # A complete RPC may consume3s connecting plus10s awaiting a response. The
 # caller must allow that entire bounded contract; no authority checks change.
 with inspection('resident-idle-ping'):
  result=run(['sudo','-n','-u','zuse','python3','/usr/local/lib/zuse/runtime-supervisor.py','ping'],capture_output=True,timeout=timeout,check=True)
 return json.loads(result.stdout)
class PathRejected(ValueError):
 def __init__(self,path,m):
  super().__init__('derivation_path_not_operator_owned');self.facts=dict(path=str(path),uid=m.st_uid,gid=m.st_gid,mode=oct(stat.S_IMODE(m.st_mode)),symlink=stat.S_ISLNK(m.st_mode),inode=str(m.st_ino),device=str(m.st_dev))
def secure(path,owner):
 for p in (path,*path.parents):
  m=p.lstat()
  if stat.S_ISLNK(m.st_mode) or m.st_uid not in ({owner} if p==path else {0,owner}) or m.st_mode&0o022:raise PathRejected(p,m)
 return path.lstat()
def parent_binding(lineage,bundle):
 require(lineage.get('schema')==1 and lineage.get('provider')=='boat','invalid_clean_parent_lineage')
 for key in ('proof','builder','parentBundle','cleanup'):
  text=lineage['documents'][key];require(isinstance(text,str) and len(text)<262144 and hashlib.sha256(text.encode()).hexdigest()==lineage['digests'][key], 'parent_lineage_digest_mismatch')
  lineage[key]=json.loads(text)
 proof=lineage['proof'];builder=lineage['builder'];parent=lineage['parentBundle'];cleanup=lineage['cleanup']
 require(proof.get('status')=='mechanism-verified-uncertified' and proof.get('cleanupVerified') is True and proof.get('artifactRetained') is True and proof.get('sourceMachineAbsent') is True and proof.get('agentAcceptanceVerified') is False,'parent_not_clean_mechanical_artifact')
 require(builder.get('status')=='prepared-uncertified' and builder.get('cleanupVerified') is True and builder.get('artifactRetained') is True and builder.get('provider')=='boat','parent_builder_not_clean')
 require(proof.get('artifactId')==builder.get('artifactId')==proof.get('snapshotName')==builder.get('snapshotName')==builder.get('runId'),'parent_artifact_identity_mismatch')
 require(builder.get('bundleSha256')==lineage.get('parentBundleFileSha256') and proof.get('runtimeSha256')==builder.get('runtimeSha256')==parent.get('runtimeSha256'),'parent_bundle_identity_mismatch')
 require(bundle.get('harnessRelease')==parent.get('harnessRelease')==builder.get('harnessRelease'),'changed_harness_requires_full_publication')
 m=builder['manifest'];require(m.get('managedBase') is True and m.get('credentialsRemoved') is True and m.get('repositories')==[],'parent_contains_task_state')
 rows=cleanup.get('previousOperations',[])
 for identity in (builder.get('machineId'),proof.get('restoreMachineId')):
  require(any(r.get('kind')=='sandboxes' and r.get('id')==identity and r.get('httpStatus')==404 and r.get('absent') is True for r in rows),'parent_machine_not_verified_absent')
 require(cleanup.get('readOnly') is True and cleanup.get('allMachinesAbsent') is True and any(r.get('kind')=='saved-snapshots' and r.get('id')==proof['artifactId'] and r.get('httpStatus')==200 for r in rows),'parent_artifact_not_verified')
 old={e['path']:e for e in parent['files']};new={e['path']:e for e in bundle['files']}
 for name in SKIP|{'harness-manifest.json'}:
  require(name in old and name in new and old[name]==new[name],'changed_harness_requires_full_publication')
 parent_mode(lineage)
 return old,new

def context(input_root,expected,owner):
 input_root=Path(input_root);secure(input_root,owner)
 lineage_path=input_root/'derivation.json';secure(lineage_path,owner)
 bundle_path=input_root/'bundle.json';secure(bundle_path,owner)
 require(digest(lineage_path)==expected,'derivation_input_digest_mismatch')
 lineage=json.loads(lineage_path.read_text());bundle=json.loads(bundle_path.read_text());old,new=parent_binding(lineage,bundle)
 return input_root,lineage,bundle,old,new

def parent_mode(lineage):
 parent=lineage['parentBundle'];builder=lineage['builder'];proof=lineage['proof']
 if parent.get('managedBaseFreshBootVersion') is None and parent.get('managedBaseFreshBootMode') is None and builder.get('initialRuntimeBootstrap') is None and proof.get('initialRuntimeBootstrap') is None:return 'legacy-enabled'
 require(parent.get('managedBaseFreshBootVersion')==1 and parent.get('managedBaseFreshBootMode')==DISABLED_PARENT_MODE,'unsupported_clean_parent_mode')
 pins=builder.get('initialRuntimeBootstrap');require(isinstance(pins,dict) and pins==proof.get('initialRuntimeBootstrap') and pins.get('version')==1 and pins.get('mode')==DISABLED_PARENT_MODE,'disabled_parent_bootstrap_binding_invalid')
 require(pins.get('runtimeSha256')==builder['runtimeSha256'] and all(re.fullmatch('[a-f0-9]{64}',pins.get(k,''))for k in ('runtimeSha256','supervisorSha256','serviceSha256','manifestSha256','helperSha256')),'disabled_parent_bootstrap_pins_invalid')
 files={x['path']:x for x in parent['files']}
 require(files.get('runtime-supervisor.py',{}).get('sha256')==pins['supervisorSha256'] and files.get('managed-base-fresh-boot.py',{}).get('sha256')==pins['helperSha256'],'disabled_parent_bootstrap_file_mismatch')
 require(builder['manifest'].get('rustRuntimeSupervisorVersion')==1 and builder['manifest'].get('commandAdmission',{}).get('runtimeSha256')==pins['runtimeSha256'],'disabled_parent_runtime_manifest_mismatch')
 return DISABLED_PARENT_MODE

def parent_processes():
 facts=[]
 for proc in Path('/proc').iterdir():
  if not proc.name.isdecimal():continue
  try:
   text=(proc/'stat').read_text();state=text[text.rfind(')')+2:].split()[0]
   uids=[int(v)for line in(proc/'status').read_text().splitlines()if line.startswith('Uid:')for v in line.split()[1:]]
   argv=[v.decode(errors='replace')for v in(proc/'cmdline').read_bytes().split(bytes([0]))if v]
   facts.append(dict(pid=int(proc.name),state=state,uids=uids,argv=argv))
  except(FileNotFoundError,ProcessLookupError):continue
 return facts

def disabled_pin_metadata(name,meta,observed_sha,pins,owner=0):
 require(name in DISABLED_IMAGE_PINS,'unsupported_disabled_parent_pin_path')
 key,agent_owned,mode=DISABLED_IMAGE_PINS[name];uid=(10001 if owner==0 else owner)if agent_owned else owner
 require(stat.S_ISREG(meta.st_mode) and meta.st_nlink==1 and meta.st_uid==uid and meta.st_gid==uid and stat.S_IMODE(meta.st_mode)==mode and observed_sha==pins[key],'disabled_parent_image_pin_mismatch')

def disabled_parent_state(lineage,filesystem,owner,service_fact=None,process_facts=None):
 require(parent_mode(lineage)==DISABLED_PARENT_MODE,'disabled_parent_mode_required')
 at=lambda path:filesystem/path.lstrip('/');pins=lineage['builder']['initialRuntimeBootstrap']
 for name,(_,agent_owned,_) in DISABLED_IMAGE_PINS.items():
  path=at(name);meta=secure(path,(10001 if owner==0 else owner)if agent_owned else owner);disabled_pin_metadata(name,meta,digest(path),pins,owner)
 if service_fact is None:
  require(filesystem==Path('/'),'parent_service_inspection_required')
  result=subprocess.run(['systemctl','show','zuse-runtime-supervisor.service','--property=MainPID,ActiveState,UnitFileState,Job'],capture_output=True,text=True,check=True,timeout=5,env={'PATH':'/usr/bin:/bin'})
  require(len(result.stdout)<4096,'parent_service_inspection_invalid');service_fact=dict(x.split('=',1)for x in result.stdout.splitlines()if '='in x)
 require(service_fact.get('MainPID')=='0' and service_fact.get('ActiveState')=='inactive' and service_fact.get('UnitFileState')=='disabled' and service_fact.get('Job','')in('','0'),'disabled_parent_service_not_inactive')
 if process_facts is None:
  require(filesystem==Path('/'),'parent_process_inspection_required');process_facts=parent_processes()
 programs={'/usr/local/bin/zuse-cloud-runtime','/usr/local/lib/zuse/runtime-supervisor.py','/usr/local/lib/zuse/runtime-repair.py','/usr/local/lib/zuse/runtime-recreation.py','/usr/local/lib/zuse/launch-rust-runtime-core','/usr/local/lib/zuse/launch-rust-recovery'}
 require(isinstance(process_facts,list) and all(isinstance(x,dict) and isinstance(x.get('uids'),list) and isinstance(x.get('argv'),list) and 10001 not in x['uids'] and not programs.intersection(x['argv'])for x in process_facts),'disabled_parent_process_present')
 for name in('/home/zuse/.zuse-data','/home/zuse/workspace','/var/lib/zuse/workspace','/var/lib/zuse/recovery'):
  path=at(name)
  if os.path.lexists(path):meta=secure(path,10001 if owner==0 else owner);require(stat.S_ISDIR(meta.st_mode) and not any(path.iterdir()),'disabled_parent_authority_present')
 return dict(unitDisabled=True,noResidentPresent=True,noAuthorityPresent=True,bootServiceSha256=pins['serviceSha256'])

def parent_metadata_failures(filesystem,bundle,harness,old,owner,stopped=False):
 # Collect every ancestry violation before reading installed code or payloads.
 # Fixed installed files + signed distribution paths; never conversation paths.
 at=lambda name:filesystem/name.lstrip('/')
 user=owner if filesystem!=Path('/') else 10001
 paths=[(at('/opt/zuse/preparation/status/manifest.json'),user)]
 paths += [(at(path),owner) for name,path in INSTALLED.items() if name in old]
 paths += [(at('/home/zuse'+suffix),user) for suffix in ('','/.zuse-data','/workspace')]
 if not stopped:paths.append((at('/home/zuse/.zuse-data/.runtime-supervision'),user))
 release=at('/opt/zuse/harnesses/releases/'+bundle['harnessRelease'])
 paths += [(release/name,owner) for name in ('manifest.json','claude','codex','grok')]
 assets=harness.get('harnesses',{}).get('codex',{}).get('assets',{})
 require(isinstance(assets,dict) and len(assets)<=128,'invalid_codex_assets')
 for name in assets:
  relative=Path(name);require(isinstance(name,str) and name.startswith('codex-package/') and not relative.is_absolute() and '..' not in relative.parts,'unsafe_distribution_asset_path')
  paths.append((release/relative,owner))
 paths.append((release/'codex-package/bin/codex',owner))
 failures={}
 for path,expected in paths:
  for index,part in enumerate((path,*path.parents)):
   try:
    m=part.lstat();allowed={expected} if index==0 else {0,expected}
    if stat.S_ISLNK(m.st_mode) or m.st_uid not in allowed or m.st_mode&0o022:
     failures[(str(part),expected)]=dict(path='/'+str(part.relative_to(filesystem)) if filesystem!=Path('/') else str(part),expectedOwner=expected,uid=m.st_uid,gid=m.st_gid,mode=oct(stat.S_IMODE(m.st_mode)),symlink=stat.S_ISLNK(m.st_mode),inode=str(m.st_ino),device=str(m.st_dev))
   except OSError as e:failures[(str(part),expected)]=dict(path=str(part),expectedOwner=expected,code=__import__('errno').errorcode.get(e.errno,'UNAVAILABLE'))
   if part==filesystem:break
 return list(failures.values())

def stopped_parent(input_root,expected,bundle,lineage,filesystem,owner,operation_id,service_fact=None,process_facts=None):
 require(isinstance(operation_id,str) and re.fullmatch(r'[A-Za-z0-9_.:-]{1,160}',operation_id),'original_builder_operation_required')
 path=input_root/'parent-stopped.json';meta=secure(path,owner)
 require(stat.S_ISREG(meta.st_mode) and meta.st_nlink==1 and stat.S_IMODE(meta.st_mode)==0o600 and meta.st_size<=16384,'parent_stop_receipt_not_protected')
 receipt=json.loads(path.read_text());h=receipt.get('hygiene',{})
 require(receipt.get('version')==1 and receipt.get('operationId')==operation_id and isinstance(receipt.get('machineId'),str) and 0<len(receipt['machineId'])<=256 and receipt.get('parentLineageSha256')==expected and receipt.get('targetBundleSha256')==digest(input_root/'bundle.json'),'parent_stop_binding_mismatch')
 if parent_mode(lineage)==DISABLED_PARENT_MODE:
  require(h.get('version')==1 and h.get('operationId')==operation_id and h.get('runtimeSha256')==lineage['builder']['runtimeSha256'] and h.get('parentMode')==DISABLED_PARENT_MODE and h.get('alreadyDisabledParent')is True and h.get('noResidentPresent')is True and h.get('noAuthorityPresent')is True and h.get('startedOrStoppedProcess')is False and h.get('enabledBootServiceRetained')is False and h.get('disabledBootServiceRetained')is True,'disabled_parent_stop_receipt_invalid')
  facts=disabled_parent_state(lineage,filesystem,owner,service_fact,process_facts);require(h.get('bootServiceSha256')==facts['bootServiceSha256'],'parent_boot_unit_changed');return receipt
 require(h.get('version')==1 and h.get('operationId')==operation_id and h.get('runtimeSha256')==lineage['builder']['runtimeSha256'] and h.get('legacyBuilder') is True and h.get('originalResidentExited') is True and h.get('supervisorLockReleased') is True and h.get('runtimeDataAbsent') is True and h.get('enabledBootServiceRetained') is True,'parent_stop_receipt_invalid')
 require(isinstance(h.get('residentPid'),int) and h['residentPid']>1 and isinstance(h.get('residentStartTicks'),int) and h['residentStartTicks']>0 and isinstance(h.get('removed'),list) and 'supervisor.lock' in h['removed'] and set(h['removed'])<={'supervisor.lock','supervisor.json','control.sock'},'parent_stop_identity_invalid')
 require(re.fullmatch('[a-f0-9]{64}',h.get('bootNonceSha256','')) and re.fullmatch('[a-f0-9]{64}',h.get('bootServiceSha256','')),'parent_stop_identity_invalid')
 unit=filesystem/'etc/systemd/system/zuse-runtime-supervisor.service';unit_meta=secure(unit,owner)
 require(stat.S_ISREG(unit_meta.st_mode) and unit_meta.st_nlink==1 and digest(unit)==h['bootServiceSha256'],'parent_boot_unit_changed')
 if service_fact is None:
  require(filesystem==Path('/'),'parent_service_inspection_required')
  result=subprocess.run(['systemctl','show','zuse-runtime-supervisor.service','--property=MainPID,ActiveState,UnitFileState'],capture_output=True,text=True,check=True,timeout=5,env={'PATH':'/usr/bin:/bin'})
  require(len(result.stdout)<4096,'parent_service_inspection_invalid');service_fact=dict(line.split('=',1) for line in result.stdout.splitlines() if '=' in line)
 require(service_fact.get('MainPID')=='0' and service_fact.get('ActiveState')=='inactive' and service_fact.get('UnitFileState')=='enabled','parent_resident_not_stopped')
 # An old PID cannot be accepted while its original process still exists.
 if filesystem==Path('/'):
  try:
   raw=Path('/proc',str(h['residentPid']),'stat').read_text();fields=raw[raw.rfind(')')+2:].split()
   require(fields[0]=='Z' or int(fields[19])!=h['residentStartTicks'],'parent_resident_still_alive')
  except FileNotFoundError:pass
 return receipt

def verify_parent(input_root,expected,*,bundle_sha256=None,filesystem=Path('/'),owner=0,ping=None,operation_id=None,service_fact=None,process_facts=None,quiescence_image=False):
 with inspection('parent-context-binding'):input_root,lineage,bundle,old,new=context(input_root,expected,owner)
 disabled=parent_mode(lineage)==DISABLED_PARENT_MODE
 stopped=os.path.lexists(input_root/'parent-stopped.json')
 if stopped:stopped_parent(input_root,expected,bundle,lineage,filesystem,owner,operation_id,service_fact,process_facts)
 if bundle_sha256 is not None:require(digest(input_root/'bundle.json')==bundle_sha256,'target_bundle_metadata_digest_mismatch')
 # These two small inputs are enough to verify the complete already-installed
 # release. No new runtime/native/archive payload is needed for parent admission.
 for name in ('harness-release.py','harness-manifest.json'):
  path=input_root/name;secure(path,owner);pin=new[name]
  require(path.is_file() and path.stat().st_size==pin['bytes'] and digest(path)==pin['sha256'],'preflight_input_integrity_mismatch')
 harness=json.loads((input_root/'harness-manifest.json').read_text())
 with inspection('protected-parent-metadata'):failures=parent_metadata_failures(filesystem,bundle,harness,old,owner,stopped=stopped or disabled)
 if failures:return dict(version=1,status='rejected',code='parent_metadata_not_protected',failures=failures)
 def at(path):return filesystem/path.lstrip('/')
 installed=at('/opt/zuse/preparation/status/manifest.json')
 with inspection('installed-parent-manifest'):require(json.loads(installed.read_text())==lineage['builder']['manifest'],'restored_manifest_mismatch')
 if disabled:disabled_parent_state(lineage,filesystem,owner,service_fact,process_facts)
 for name,path in INSTALLED.items():
  if name not in old:continue
  with inspection('installed-parent-'+name):
   target=at(path);require(target.is_file() and digest(target)==old[name]['sha256'],'restored_binary_integrity_mismatch')
 home=at('/home/zuse');data=home/'.zuse-data';workspace=home/'workspace'
 require(all(p.is_dir() and not p.is_symlink() for p in (home,data,workspace)),'restored_state_directory_invalid')
 require(not any(workspace.iterdir()),'parent_contains_repository')
 require(not any(data.iterdir()) if stopped or disabled else all(p.name=='.runtime-supervision' and p.is_dir() and not p.is_symlink() for p in data.iterdir()),'parent_contains_runtime_identity')
 ipc=data/'.runtime-supervision';entries=[] if stopped or disabled else list(ipc.iterdir());require(len(entries)<=3,'parent_contains_runtime_ipc_state')
 user=owner if filesystem!=Path('/') else 10001
 for path in entries:
  m=path.lstat();require(m.st_uid==user and not m.st_mode&0o077 and ((path.name=='control.sock' and stat.S_ISSOCK(m.st_mode)) or (path.name in ('supervisor.lock','supervisor.json') and stat.S_ISREG(m.st_mode))),'parent_contains_runtime_ipc_state')
 for name in ('.codex/auth.json','.claude/.credentials.json','.grok/auth.json','.config/grok/auth.json','.config/gh','.zuse-github-cli','.git-credentials','.netrc','.zuse/cloud-auth','.zuse-image/provider-secrets.json'):
  require(not os.path.lexists(home/name),'parent_contains_account_credentials')
 ssh=home/'.ssh'
 if os.path.lexists(ssh):require(ssh.is_dir() and not ssh.is_symlink() and not any(ssh.iterdir()),'parent_contains_account_credentials')
 if ping is None and not stopped and not disabled and not quiescence_image:
  ping=read_parent_ping()
 if not stopped and not disabled and not quiescence_image:require(ping.get('version')==1 and ping.get('bound') is False and ping.get('childAlive') is False and ping.get('recoveryAlive') is False,'parent_resident_not_idle')
 root=at('/opt/zuse/harnesses');release=root/'releases'/bundle['harnessRelease']
 with inspection('native-distribution-ancestry'):
  for path in release.rglob('*'):secure(path,owner)
 manager_path=input_root/'harness-release.py'
 with inspection('native-distribution-manager-import'):
  spec=importlib.util.spec_from_file_location('derive_harness_manager',manager_path);manager=importlib.util.module_from_spec(spec);spec.loader.exec_module(manager)
 with inspection('native-distribution-full-integrity'):directory,metadata=manager.release(root,bundle['harnessRelease'],verify=True)
 require(metadata['harnesses']['codex'].get('assets') is not None,'complete_codex_distribution_required')
 require(harness==metadata,'installed_harness_manifest_mismatch')
 for name in ('claude','codex','grok'):require(digest(directory/name)==new[name]['sha256'],'installed_harness_binary_mismatch')
 result=dict(version=1,status='quiescence-image-verified' if quiescence_image else 'passed',parentArtifactId=lineage['proof']['artifactId'],parentLineageSha256=expected,parentRuntimeSha256=lineage['builder']['runtimeSha256'],runtimeSha256=bundle['runtimeSha256'],harnessRelease=bundle['harnessRelease'],completeCodexAssetsVerified=True,sourceClean=True,reusedFiles=sorted(SKIP))
 if disabled:result.update(parentMode=DISABLED_PARENT_MODE,noResidentPresent=True,noAuthorityPresent=True,bootServiceSha256=lineage['builder']['initialRuntimeBootstrap']['serviceSha256'])
 return result

def verify(input_root,expected,*,filesystem=Path('/'),owner=0,ping=None,operation_id=None,service_fact=None):
 input_root,lineage,bundle,old,new=context(input_root,expected,owner)
 # Overlay admission remains independently mandatory, even after the small
 # preflight passed earlier; a parent receipt cannot certify missing new bytes.
 for name,entry in new.items():
  if name in SKIP:continue
  p=input_root/name;secure(p,owner)
  require(p.is_file() and p.stat().st_size==entry['bytes'] and digest(p)==entry['sha256'],'overlay_integrity_mismatch')
 receipt=verify_parent(input_root,expected,filesystem=filesystem,owner=owner,ping=ping,operation_id=operation_id,service_fact=service_fact)
 require(receipt['status']=='passed','parent_metadata_not_protected')
 return receipt

def main():
 p=argparse.ArgumentParser();p.add_argument('--input',type=Path,required=True);p.add_argument('--sha256',required=True);p.add_argument('--parent-preflight',action='store_true');p.add_argument('--quiescence-image',action='store_true');p.add_argument('--bundle-sha256');p.add_argument('--operation-id');a=p.parse_args()
 if a.parent_preflight:
  require(os.geteuid()==0 and a.input==Path('/var/lib/zuse/base-preflight') and a.bundle_sha256 is not None,'parent_preflight_requires_isolated_root_builder')
  try:receipt=verify_parent(a.input,a.sha256,bundle_sha256=a.bundle_sha256,quiescence_image=a.quiescence_image,operation_id=a.operation_id)
  except (ValueError,OSError,subprocess.SubprocessError,InspectionFailure) as error:
   receipt=dict(version=1,status='rejected',code=str(error) if isinstance(error,ValueError) and str(error).replace('_','').isalnum() else 'parent_preflight_failed')
   if isinstance(error,PathRejected):receipt['failures']=[error.facts]
   if isinstance(error,InspectionFailure):receipt['inspection']=error.facts
   elif isinstance(error,(OSError,subprocess.SubprocessError)):receipt['inspection']=InspectionFailure('parent-preflight-unclassified',error,0).facts
  print(json.dumps(receipt,sort_keys=True));return
 require(os.geteuid()==0 and a.input==Path('/opt/zuse-base-input'),'derivation_requires_isolated_root_builder')
 require(os.path.lexists(a.input/'parent-stopped.json'),'verified_parent_stop_required')
 receipt=verify(a.input,a.sha256,operation_id=a.operation_id)
 # This isolated base has no task child. Retire the old resident before replacing
 # its code, then preserve the exact verified old manifest for lineage inspection.
 subprocess.run(['systemctl','stop','zuse-runtime-supervisor.service'],check=True,timeout=20)
 lineage=json.loads((a.input/'derivation.json').read_text());parent_binding(lineage,json.loads((a.input/'bundle.json').read_text()))
 source=Path('/opt/zuse/preparation/status/manifest.json')
 require(json.loads(source.read_text())==lineage['builder']['manifest'],'restored_manifest_changed')
 previous=source.with_name('parent-'+a.sha256+'.json');require(not os.path.lexists(previous),'derivation_already_prepared')
 source.rename(previous)
 (a.input/'derivation-validated.json').write_text(json.dumps(receipt,sort_keys=True)+'\n')
 print(json.dumps(receipt,sort_keys=True))
if __name__=='__main__':
 try:main()
 except PathRejected as e:
  print(json.dumps(dict(code=str(e),pathFacts=e.facts)),file=__import__('sys').stderr);raise SystemExit(1)
