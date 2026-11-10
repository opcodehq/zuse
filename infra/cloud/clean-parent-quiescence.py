#!/usr/bin/env python3
"""Retire ONLY an immutable, authority-free clean parent before a native overlay.

No task/resume path uses this helper. A stale pathname socket is not rebound.
The old clean process is fenced by pidfd, unit, cgroup, UID/start and no children;
its path-based Python teardown cannot delete a replacement winner's socket.
"""
import argparse,fcntl,hashlib,importlib.util,json,os,re,signal,stat,subprocess,time
from pathlib import Path

class Rejected(Exception):
 def __init__(self,code,facts=None):super().__init__(code);self.facts=facts
def require(value,code):
 if not value:raise Rejected(code)
def identity(meta):return meta.st_dev,meta.st_ino
def process(pid):
 try:
  raw=Path('/proc',str(pid),'stat').read_text();fields=raw[raw.rfind(')')+2:].split()
  status=Path('/proc',str(pid),'status').read_text().splitlines()
  return dict(state=fields[0],start=int(fields[19]),uids=[int(v) for line in status if line.startswith('Uid:') for v in line.split()[1:]])
 except FileNotFoundError:return None
def unit():
 result=subprocess.run(['systemctl','show','zuse-runtime-supervisor.service','--property=MainPID,ActiveState,SubState,Result,ExecMainCode,ExecMainStatus,UnitFileState,ControlGroup,Job'],capture_output=True,text=True,timeout=3,check=True,env={'PATH':'/usr/bin:/bin'})
 require(len(result.stdout)<4096,'parent_unit_unavailable')
 return dict(line.split('=',1) for line in result.stdout.splitlines() if '=' in line)
def stop_jobs():
 result=subprocess.run(['systemctl','list-jobs','--no-legend','--no-pager','--full'],capture_output=True,text=True,timeout=3,check=True,env={'PATH':'/usr/bin:/bin'})
 require(len(result.stdout)<=65536,'parent_jobs_unavailable')
 jobs=[]
 for line in result.stdout.splitlines():
  fields=line.split()
  if len(fields)==4 and fields[0].isdigit() and fields[1]=='zuse-runtime-supervisor.service' and fields[2] in ('stop','start','restart','reload','verify-active') and fields[3] in ('waiting','running'):
   jobs.append(dict(id=int(fields[0]),type=fields[2],state=fields[3]))
 require(len(jobs)<=4,'parent_jobs_ambiguous');return jobs
def terminal_facts(pid,start,group,*,service=unit,cgroup_members=None,jobs=stop_jobs):
 facts={'originalPid':pid,'originalStartTicks':start}
 try:
  value=service();facts['unit']={k:value[k] for k in ('MainPID','ActiveState','SubState','Result','ExecMainCode','ExecMainStatus','UnitFileState','Job','ControlGroup') if k in value and isinstance(value[k],str) and len(value[k])<=128 and re.fullmatch(r'[A-Za-z0-9_./:;= ()-]*',value[k])}
 except Exception as error:facts['unitErrorClass']=type(error).__name__
 try:
  current=process(pid);facts['originalProcess']=current;facts['originalIdentityPresent']=current is not None and current['start']==start
 except Exception as error:facts['processErrorClass']=type(error).__name__
 try:
  if cgroup_members is None:
   root=Path('/sys/fs/cgroup')/group.lstrip('/')
   if not root.exists():facts['cgroup']={'present':False,'empty':True}
   else:
    pids=members(group);facts['cgroup']={'present':True,'empty':not pids,'processCount':len(pids),'originalPidPresent':pid in pids}
  else:
   pids=cgroup_members(group);facts['cgroup']={'present':True,'empty':not pids,'processCount':len(pids),'originalPidPresent':pid in pids}
 except Exception as error:facts['cgroupErrorClass']=type(error).__name__
 try:facts['jobs']=jobs()
 except Exception as error:facts['jobsErrorClass']=type(error).__name__
 return facts
def empty_stopped_facts(facts,group):
 return facts.get('originalIdentityPresent') is False and facts.get('cgroup',{}).get('empty') is True and facts.get('cgroup',{}).get('processCount',0)==0 and facts.get('jobs')==[] and facts.get('unit',{}).get('Job')=='' and facts.get('unit',{}).get('ControlGroup') in ('',group) and not any(k.endswith('ErrorClass') for k in facts)
def confirm_stopped(pid,start,group,*,service=unit,cgroup_members=None,jobs=stop_jobs,timeout=5):
 until=time.monotonic()+timeout;after={}
 try:
  while time.monotonic()<until:
   after=service()
   if after.get('MainPID')=='0' and after.get('ActiveState')=='inactive':break
   time.sleep(.05)
 except Exception as error:
  facts=terminal_facts(pid,start,group,service=service,cgroup_members=cgroup_members,jobs=jobs);facts['observationErrorClass']=type(error).__name__
  raise Rejected('parent_stop_observation_failed',facts) from None
 facts=terminal_facts(pid,start,group,service=lambda:after,cgroup_members=cgroup_members,jobs=jobs)
 if not (after.get('MainPID')=='0' and after.get('ActiveState')=='inactive' and after.get('UnitFileState')=='enabled' and empty_stopped_facts(facts,group)):raise Rejected('parent_stop_job_unconfirmed',facts)
 return facts
def normalize_intentional_stop(pid,start,group,retired,*,authorize,service=unit,cgroup_members=None,jobs=stop_jobs,reset=None):
 """Only the factory's proved kill may clear its exact terminal signal failure.

 Failed is never accepted as stopped. This performs ONE fixed-unit reset then
 requires the original strict inactive/enabled predicate and no surviving work.
 """
 before=terminal_facts(pid,start,group,service=service,cgroup_members=cgroup_members,jobs=jobs)
 def empty(facts):
  return facts.get('originalProcess') is None and empty_stopped_facts(facts,group)
 def eligible(facts):
  expected=dict(MainPID='0',ActiveState='failed',SubState='failed',Result='signal',ExecMainCode='2',ExecMainStatus='9',UnitFileState='enabled')
  return empty(facts) and all(facts.get('unit',{}).get(k)==v for k,v in expected.items())
 proof=retired.get('postRetirementFacts',{})
 authorized=retired.get('residentPid')==pid and retired.get('residentStartTicks')==start and retired.get('residentUid')==10001 and retired.get('controlGroup')==group and all(retired.get(k) is True for k in ('unitHadOnlyResident','frozenCleanVerified','pidfdRetirement','retiredWithoutPathCleanup','visibleLockProtected')) and proof.get('originalPid')==pid and proof.get('originalStartTicks')==start and proof.get('originalIdentityPresent') is False and proof.get('stopSubmitted') is True and proof.get('pidfdKillSubmitted') is True
 if not authorized or not eligible(before):raise Rejected('parent_intentional_stop_normalization_denied',before)
 receipt={'version':1,'fixedUnit':'zuse-runtime-supervisor.service','resetSubmitted':False,'before':before}
 try:
  require(callable(authorize),'parent_normalization_authority_required');require(authorize() is not False,'parent_normalization_authority_changed')
  fresh=terminal_facts(pid,start,group,service=service,cgroup_members=cgroup_members,jobs=jobs)
  if not eligible(fresh):raise Rejected('parent_intentional_stop_normalization_denied',fresh)
  receipt['before']=fresh;receipt['resetSubmitted']=True
  if reset is None:subprocess.run(['systemctl','reset-failed','zuse-runtime-supervisor.service'],check=True,timeout=3,capture_output=True,env={'PATH':'/usr/bin:/bin'})
  else:reset()
  after=confirm_stopped(pid,start,group,service=service,cgroup_members=cgroup_members,jobs=jobs)
  receipt['after']=after
  if not empty(after):raise Rejected('parent_stop_not_empty_after_normalization',after)
  receipt['strictStoppedVerified']=True;return receipt
 except Exception as error:
  receipt['after']=terminal_facts(pid,start,group,service=service,cgroup_members=cgroup_members,jobs=jobs);receipt['errorClass']=type(error).__name__
  raise Rejected(str(error) if isinstance(error,Rejected) else 'parent_stop_normalization_failed',{'intentionalStopNormalization':receipt}) from None
def members(group):
 require(isinstance(group,str) and group.startswith('/') and '..' not in Path(group).parts and group.endswith('/zuse-runtime-supervisor.service'),'parent_cgroup_invalid')
 root=Path('/sys/fs/cgroup')/group.lstrip('/')
 require(not any(p.is_dir() for p in root.iterdir()),'parent_cgroup_has_descendants')
 return sorted(int(v) for v in (root/'cgroup.procs').read_text().split())
def lock_owners(meta):
 found=[]
 for line in Path('/proc/locks').read_text().splitlines():
  fields=line.split()
  if len(fields)<8 or fields[1:4]!=['FLOCK','ADVISORY','WRITE']:continue
  key=fields[5].split(':')
  if len(key)==3 and (int(key[0],16),int(key[1],16),int(key[2]))==(os.major(meta.st_dev),os.minor(meta.st_dev),meta.st_ino):found.append(int(fields[4]))
 return found

class VisibleLock:
 def __init__(self,data,uid):
  self.data=Path(data);self.uid=uid;self.fds=[]
  try:
   self.data_fd=os.open(self.data,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC);self.fds.append(self.data_fd)
   self.ipc_fd=os.open('.runtime-supervision',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=self.data_fd);self.fds.append(self.ipc_fd)
   self.lock_fd=os.open('supervisor.lock',os.O_RDWR|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=self.ipc_fd);self.fds.append(self.lock_fd)
   self.expected={'data':identity(os.fstat(self.data_fd)),'ipc':identity(os.fstat(self.ipc_fd)),'lock':identity(os.fstat(self.lock_fd))}
   self.guard()
  except BaseException:self.close();raise
 def guard(self):
  for path in (self.data,*self.data.parents):require(not path.is_symlink(),'parent_directory_symlink')
  current=[self.data.lstat(),os.stat('.runtime-supervision',dir_fd=self.data_fd,follow_symlinks=False),os.stat('supervisor.lock',dir_fd=self.ipc_fd,follow_symlinks=False)]
  for name,meta,fd in zip(('data','ipc','lock'),current,self.fds):
   require(identity(meta)==self.expected[name]==identity(os.fstat(fd)),'parent_visible_path_changed')
   require(meta.st_uid==self.uid and stat.S_IMODE(meta.st_mode)==(0o600 if name=='lock' else 0o700) and (stat.S_ISREG(meta.st_mode) and meta.st_nlink==1 if name=='lock' else stat.S_ISDIR(meta.st_mode)),'parent_visible_path_unsafe')
 def protect(self,expected_pid=None):
  self.guard()
  try:fcntl.flock(self.lock_fd,fcntl.LOCK_EX|fcntl.LOCK_NB);return True
  except BlockingIOError:
   require(expected_pid is not None and lock_owners(os.fstat(self.lock_fd))==[expected_pid],'parent_visible_lock_has_other_owner')
   return False
 def close(self):
  for fd in reversed(self.fds):os.close(fd)
  self.fds=[]

def retire_idle(pid,start,uid,lock,*,expected_argv,frozen_clean=None,service=unit,cgroup_members=members,stop_unit=None,timeout=5):
 """Real pidfd retirement; injectable unit reads allow isolated OS-lock tests."""
 require(hasattr(os,'pidfd_open') and hasattr(signal,'pidfd_send_signal'),'parent_pidfd_required')
 require(callable(frozen_clean),'parent_frozen_clean_check_required')
 expected_group=None
 def checked():
  fact=process(pid);require(fact is not None and fact['start']==start and fact['uids']==[uid]*4,'parent_process_identity_changed')
  require(Path('/proc',str(pid),'cmdline').read_bytes().rstrip(b'\0').split(b'\0')==[v.encode() for v in expected_argv],'parent_process_program_changed')
  require(not Path('/proc',str(pid),'task',str(pid),'children').read_text().split(),'parent_live_descendants')
  value=service();require(value.get('MainPID')==str(pid) and value.get('ActiveState')=='active' and value.get('UnitFileState')=='enabled','parent_unit_identity_changed')
  require(expected_group is None or value.get('ControlGroup')==expected_group,'parent_cgroup_identity_changed')
  require(cgroup_members(value.get('ControlGroup'))==[pid],'parent_cgroup_has_other_processes');lock.guard()
  return value
 initial=checked();expected_group=initial['ControlGroup'];lock.protect(expected_pid=pid)
 fd=os.pidfd_open(pid,0)
 frozen=False;stop_submitted=False;killed=False
 try:
  checked();signal.pidfd_send_signal(fd,signal.SIGSTOP)
  frozen=True
  until=time.monotonic()+1
  while time.monotonic()<until:
   frozen=process(pid)
   if frozen and frozen['start']==start and frozen['state'] in ('T','t'):break
   time.sleep(.005)
  require(frozen is not None and frozen['start']==start and frozen['state'] in ('T','t'),'parent_freeze_unconfirmed')
  # An idle process frozen at the validated identity cannot run legacy finally
  # code against a path replaced while systemd's stop job is being accepted.
  checked()
  require(frozen_clean() is not False,'parent_became_bound')
  checked()
  stop_submitted=True
  if stop_unit is None:subprocess.run(['systemctl','stop','--no-block','zuse-runtime-supervisor.service'],check=True,timeout=3,env={'PATH':'/usr/bin:/bin'})
  else:stop_unit()
  signal.pidfd_send_signal(fd,signal.SIGKILL)
  killed=True
  until=time.monotonic()+timeout
  while time.monotonic()<until:
   fact=process(pid)
   if fact is None or fact['state']=='Z' or fact['start']!=start:break
   time.sleep(.01)
  fact=process(pid);require(fact is None or fact['state']=='Z' or fact['start']!=start,'parent_retirement_unconfirmed')
  require(lock.protect(),'parent_visible_lock_unavailable')
  stopped_facts=terminal_facts(pid,start,expected_group,service=service,cgroup_members=cgroup_members,jobs=stop_jobs if stop_unit is None else lambda:[])
  stopped_facts.update(stopSubmitted=True,pidfdKillSubmitted=True)
  return dict(residentPid=pid,residentStartTicks=start,residentUid=uid,controlGroup=expected_group,unitHadOnlyResident=True,frozenCleanVerified=True,pidfdRetirement=True,retiredWithoutPathCleanup=True,visibleLockProtected=True,postRetirementFacts=stopped_facts)
 except Exception as error:
  if stop_submitted:
   facts=terminal_facts(pid,start,expected_group,service=service,cgroup_members=cgroup_members,jobs=stop_jobs if stop_unit is None else lambda:[])
   facts.update(stopSubmitted=True,pidfdKillSubmitted=killed)
   raise Rejected(str(error) if isinstance(error,Rejected) else 'parent_retirement_inspection_failed',facts) from None
  raise
 finally:
  if frozen and not stop_submitted and not killed:
   fact=process(pid)
   if fact is not None and fact['start']==start and fact['state'] in ('T','t'):
    signal.pidfd_send_signal(fd,signal.SIGCONT)
  os.close(fd)

def load(path,expected,name):
 meta=path.lstat();require(stat.S_ISREG(meta.st_mode) and meta.st_uid==0 and meta.st_nlink==1 and not meta.st_mode&0o022 and meta.st_size<=131072,'parent_operator_source_unsafe')
 raw=path.read_bytes();require(hashlib.sha256(raw).hexdigest()==expected,'parent_operator_source_changed')
 spec=importlib.util.spec_from_file_location(name,path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module

def already_disabled_parent(proof,operation_id,lineage_sha256,verify):
 require(proof.get('parentMode')=='post-hydration-first-start-v1' and proof.get('noResidentPresent')is True and proof.get('noAuthorityPresent')is True,'disabled_parent_proof_required')
 require(verify()==proof,'disabled_parent_changed_before_receipt')
 return dict(version=1,operationId=operation_id,runtimeSha256=proof['parentRuntimeSha256'],parentMode=proof['parentMode'],alreadyDisabledParent=True,noResidentPresent=True,noAuthorityPresent=True,startedOrStoppedProcess=False,legacyBuilder=False,atomicStopAcknowledged=False,enabledBootServiceRetained=False,disabledBootServiceRetained=True,bootServiceSha256=proof['bootServiceSha256'],parentImageProof=proof,parentLineageSha256=lineage_sha256)

def main():
 p=argparse.ArgumentParser();p.add_argument('--operation-id',required=True);p.add_argument('--lineage-sha256',required=True);p.add_argument('--bundle-sha256',required=True);p.add_argument('--validator-sha256',required=True);p.add_argument('--hygiene-sha256',required=True);p.add_argument('--report-only',action='store_true');a=p.parse_args()
 require(os.geteuid()==0 and re.fullmatch(r'zuse-base-build-[a-f0-9]{24}',a.operation_id),'parent_factory_operation_required')
 require(all(re.fullmatch('[a-f0-9]{64}',getattr(a,k)) for k in ('lineage_sha256','bundle_sha256','validator_sha256','hygiene_sha256')),'parent_factory_digest_invalid')
 root=Path('/var/lib/zuse/base-preflight');require(root.is_dir() and not root.is_symlink() and root.stat().st_uid==0 and stat.S_IMODE(root.stat().st_mode)==0o700,'parent_factory_input_required')
 verifier=load(root/'parent-validator.py',a.validator_sha256,'parent_verifier');hygiene=load(root/'parent-hygiene.py',a.hygiene_sha256,'parent_hygiene')
 proof=verifier.verify_parent(root,a.lineage_sha256,bundle_sha256=a.bundle_sha256,quiescence_image=True)
 require(proof['status']=='quiescence-image-verified','parent_full_image_not_verified')
 if proof.get('parentMode')=='post-hydration-first-start-v1':
  receipt=already_disabled_parent(proof,a.operation_id,a.lineage_sha256,lambda:verifier.verify_parent(root,a.lineage_sha256,bundle_sha256=a.bundle_sha256,quiescence_image=True))
  print(json.dumps(receipt,sort_keys=True));return
 data=Path('/home/zuse/.zuse-data');checkout=Path('/home/zuse/workspace');status=Path('/var/lib/zuse/workspace');recovery=Path('/var/lib/zuse/recovery');uid=10001
 hygiene.contents_clean(data,uid,checkout,status,recovery)
 unit_path=Path('/etc/systemd/system/zuse-runtime-supervisor.service');meta=unit_path.lstat();require(stat.S_ISREG(meta.st_mode) and meta.st_uid==0 and meta.st_nlink==1 and not meta.st_mode&0o022 and meta.st_size<=16384,'parent_unit_file_invalid')
 unit_sha=verifier.digest(unit_path);value=unit();pid=int(value.get('MainPID','0'));require(pid>1,'parent_resident_missing');fact=process(pid);require(fact is not None,'parent_resident_missing')
 record_path=data/'.runtime-supervision/supervisor.json';record_meta=record_path.lstat();require(stat.S_ISREG(record_meta.st_mode) and record_meta.st_uid==uid and record_meta.st_nlink==1 and stat.S_IMODE(record_meta.st_mode)==0o600 and record_meta.st_size<=4096,'parent_record_unsafe')
 record=json.loads(record_path.read_text());require(set(record)=={'version','residentPid','bootNonce'} and record['version']==1 and record['residentPid']==pid and re.fullmatch('[a-f0-9]{32}',record['bootNonce']),'parent_record_identity_changed')
 lock=VisibleLock(data,uid)
 try:
  hygiene.contents_clean(data,uid,checkout,status,recovery)
  def frozen_clean():
   hygiene.contents_clean(data,uid,checkout,status,recovery)
   current=verifier.verify_parent(root,a.lineage_sha256,bundle_sha256=a.bundle_sha256,quiescence_image=True)
   require(current==proof,'parent_image_changed_before_retirement')
  retired=retire_idle(pid,fact['start'],uid,lock,expected_argv=['/usr/bin/python3','/usr/local/lib/zuse/runtime-supervisor.py','serve'],frozen_clean=frozen_clean)
  try:after_facts=confirm_stopped(pid,fact['start'],retired['controlGroup'])
  except Rejected as error:
   if error.facts is not None:error.facts['postRetirementFacts']=retired['postRetirementFacts']
   if str(error)!='parent_stop_job_unconfirmed':raise
   def authorize_normalization():
    lock.guard();require(lock.protect(),'parent_visible_lock_unavailable')
    require(verifier.digest(unit_path)==unit_sha,'parent_boot_unit_changed')
    hygiene.contents_clean(data,uid,checkout,status,recovery)
    require(verifier.verify_parent(root,a.lineage_sha256,bundle_sha256=a.bundle_sha256,quiescence_image=True)==proof,'parent_image_changed_before_normalization')
   normalization=normalize_intentional_stop(pid,fact['start'],retired['controlGroup'],retired,authorize=authorize_normalization)
   retired['intentionalStopNormalization']=normalization;after_facts=normalization['after']
  require(verifier.digest(unit_path)==unit_sha,'parent_boot_unit_changed');lock.guard()
  receipt=hygiene.clean_stopped(data,uid,pid,fact['start'],checkout,status,recovery,expected_paths=lock.expected,held_lock=lock.lock_fd)
  receipt.update(retired,afterUnitFacts=after_facts,operationId=a.operation_id,runtimeSha256=proof['parentRuntimeSha256'],bootNonceSha256=hashlib.sha256(record['bootNonce'].encode()).hexdigest(),legacyBuilder=True,atomicStopAcknowledged=False,enabledBootServiceRetained=True,bootServiceSha256=unit_sha,parentImageProof=proof,parentLineageSha256=a.lineage_sha256)
  print(json.dumps(receipt,sort_keys=True))
 finally:lock.close()

if __name__=='__main__':
 try:main()
 except Exception as error:
  code=str(error) if isinstance(error,Rejected) else 'parent_quiescence_inspection_failed'
  result=dict(version=1,status='rejected',code=code,errorClass=type(error).__name__)
  if isinstance(error,Rejected) and error.facts is not None:result['afterUnitFacts']=error.facts
  print(json.dumps(result))
  if '--report-only' not in __import__('sys').argv:raise SystemExit(1)
