#!/usr/bin/env python3
"""Builder-only idle shutdown and exact ephemeral supervision cleanup.

No runtime identity, user data, service unit, boot command or account profile is
removed. A retained task is rejected. This is never a task lifecycle action.
"""
import argparse,errno,fcntl,hashlib,importlib.util,json,os,stat,subprocess,time
from pathlib import Path
NAMES={'supervisor.lock','supervisor.json','control.sock'}
class Rejected(Exception):pass
def require(value,code):
 if not value:raise Rejected(code)
def directory(path,uid):
 for parent in (path,*path.parents):require(not parent.is_symlink(),'publication_directory_symlink')
 meta=path.lstat();require(stat.S_ISDIR(meta.st_mode) and meta.st_uid==uid and stat.S_IMODE(meta.st_mode)==0o700,'publication_directory_owner_invalid')
 return meta
def contents_clean(data,uid,checkout,status,recovery):
 directory(data,uid)
 for path in (checkout,status,recovery):
  meta=path.lstat();require(stat.S_ISDIR(meta.st_mode) and meta.st_uid==uid and not path.is_symlink() and not any(path.iterdir()),'publication_runtime_data_present')
 require(set(p.name for p in data.iterdir())<= {'.runtime-supervision'},'publication_runtime_data_present')
def process_fact(pid):
 try:
  text=Path('/proc',str(pid),'stat').read_text();fields=text[text.rfind(')')+2:].split();return fields[0],int(fields[19])
 except FileNotFoundError:return None
def service():
 value=subprocess.run(['systemctl','show','zuse-runtime-supervisor.service','--property=MainPID,ActiveState,UnitFileState,Result,ExecMainCode,ExecMainStatus'],capture_output=True,text=True,timeout=3,env={'PATH':'/usr/bin:/bin'})
 require(value.returncode==0 and len(value.stdout)<4096,'publication_boot_owner_unavailable')
 return dict(line.split('=',1) for line in value.stdout.splitlines() if '=' in line)
def verify_stopped_service(after,expected_unit):
 # systemd may complete an explicit stop by SIGTERM after the private idle
 # stop acknowledgement. Successful stop, original identity, released lock and
 # empty task data are still independently required before snapshot capture.
 outcome=(after.get('ExecMainCode'),after.get('ExecMainStatus'))
 require(after.get('MainPID')=='0' and after.get('ActiveState')=='inactive' and after.get('UnitFileState')==expected_unit and after.get('Result')=='success' and outcome in (('1','0'),('2','15')),'publication_boot_owner_not_retained')

def clean_stopped(data,uid,pid,start_ticks,checkout,status,recovery,*,expected_paths=None,held_lock=None):
 """Remove only verified-owned known ephemeral leaves after original exit."""
 contents_clean(data,uid,checkout,status,recovery)
 fact=process_fact(pid);require(fact is None or fact[0]=='Z' or fact[1]!=start_ticks,'publication_resident_still_alive')
 data_fd=os.open(data,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC)
 opened_data=os.fstat(data_fd)
 ipc_fd=None;lock_fd=None
 try:
  require(opened_data.st_uid==uid and stat.S_IMODE(opened_data.st_mode)==0o700,'publication_directory_owner_invalid')
  if expected_paths:require((opened_data.st_dev,opened_data.st_ino)==expected_paths['data'],'publication_data_directory_changed')
  ipc=data/'.runtime-supervision';directory(ipc,uid)
  ipc_fd=os.open('.runtime-supervision',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=data_fd)
  if expected_paths:require((os.fstat(ipc_fd).st_dev,os.fstat(ipc_fd).st_ino)==expected_paths['ipc'],'publication_ipc_directory_changed')
  require(set(os.listdir(ipc_fd))<=NAMES,'publication_unknown_ipc')
  metadata={}
  for name in os.listdir(ipc_fd):
   meta=os.stat(name,dir_fd=ipc_fd,follow_symlinks=False);socket_file=name=='control.sock'
   require(meta.st_uid==uid and meta.st_nlink==1 and stat.S_IMODE(meta.st_mode)==0o600 and (stat.S_ISSOCK(meta.st_mode) if socket_file else stat.S_ISREG(meta.st_mode)),'publication_ipc_owner_invalid')
   if name in ('supervisor.lock','control.sock'):require(meta.st_size==0,'publication_ipc_payload_invalid')
   metadata[name]=(meta.st_dev,meta.st_ino)
  if 'supervisor.json' in metadata:
   record_fd=os.open('supervisor.json',os.O_RDONLY|os.O_CLOEXEC|os.O_NOFOLLOW,dir_fd=ipc_fd)
   with os.fdopen(record_fd,'rb') as source:
    body=source.read(4097);require(len(body)<=4096,'publication_ipc_record_invalid');record=json.loads(body)
   require(isinstance(record,dict) and set(record)=={'version','bootNonce','residentPid'} and record['version']==1 and isinstance(record['residentPid'],int) and record['residentPid']>0 and isinstance(record['bootNonce'],str) and len(record['bootNonce'])==32 and all(c in '0123456789abcdef' for c in record['bootNonce']),'publication_ipc_record_invalid')
  require('supervisor.lock' in metadata,'publication_lock_missing')
  lock_fd=os.dup(held_lock) if held_lock is not None else os.open('supervisor.lock',os.O_RDWR|os.O_CLOEXEC|os.O_NOFOLLOW,dir_fd=ipc_fd)
  try:fcntl.flock(lock_fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
  except BlockingIOError:raise Rejected('publication_lock_still_held')
  require((os.fstat(lock_fd).st_dev,os.fstat(lock_fd).st_ino)==metadata['supervisor.lock'],'publication_lock_changed')
  if expected_paths:require(metadata['supervisor.lock']==expected_paths['lock'],'publication_lock_changed')
  contents_clean(data,uid,checkout,status,recovery)
  require(set(os.listdir(ipc_fd))==set(metadata),'publication_ipc_changed')
  for name,(device,inode) in metadata.items():
   meta=os.stat(name,dir_fd=ipc_fd,follow_symlinks=False);require((meta.st_dev,meta.st_ino)==(device,inode),'publication_ipc_changed')
  current=os.stat('.runtime-supervision',dir_fd=data_fd,follow_symlinks=False);opened=os.fstat(ipc_fd)
  require((current.st_dev,current.st_ino)==(opened.st_dev,opened.st_ino),'publication_ipc_directory_changed')
  current_data=data.lstat();require((current_data.st_dev,current_data.st_ino)==(opened_data.st_dev,opened_data.st_ino),'publication_data_directory_changed')
  for name in sorted(metadata):
   current_data=data.lstat();current_ipc=os.stat('.runtime-supervision',dir_fd=data_fd,follow_symlinks=False);current_leaf=os.stat(name,dir_fd=ipc_fd,follow_symlinks=False)
   require((current_data.st_dev,current_data.st_ino)==(opened_data.st_dev,opened_data.st_ino) and (current_ipc.st_dev,current_ipc.st_ino)==(opened.st_dev,opened.st_ino),'publication_ipc_directory_changed')
   require((current_leaf.st_dev,current_leaf.st_ino)==metadata[name],'publication_ipc_changed')
   os.unlink(name,dir_fd=ipc_fd)
  os.rmdir('.runtime-supervision',dir_fd=data_fd)
  current_data=data.lstat();require((current_data.st_dev,current_data.st_ino)==(opened_data.st_dev,opened_data.st_ino),'publication_data_directory_changed')
  require(not os.listdir(data_fd),'publication_cleanup_incomplete')
 finally:
  if lock_fd is not None:os.close(lock_fd)
  if ipc_fd is not None:os.close(ipc_fd)
  os.close(data_fd)
 return dict(version=1,originalResidentExited=True,supervisorLockReleased=True,removed=sorted(metadata),runtimeDataAbsent=True)
def stop_builder(args):
 require(os.geteuid()==0,'publication_root_required')
 data=Path('/home/zuse/.zuse-data');checkout=Path('/home/zuse/workspace');status=Path('/var/lib/zuse/workspace');recovery=Path('/var/lib/zuse/recovery');uid=10001
 manifest_path=Path('/opt/zuse/preparation/status/manifest.json');manifest_fd=os.open(manifest_path,os.O_RDONLY|os.O_CLOEXEC|os.O_NOFOLLOW)
 with os.fdopen(manifest_fd,'rb') as source:
  meta=os.fstat(source.fileno());require(stat.S_ISREG(meta.st_mode) and meta.st_uid in (0,uid) and meta.st_nlink==1 and not meta.st_mode&0o022 and meta.st_size<=1048576,'publication_manifest_owner_invalid');manifest=json.load(source)
 require(manifest.get('managedBase') is True and manifest.get('credentialsRemoved') is True and manifest.get('repositories')==[],'publication_clean_base_required')
 runtime=Path('/usr/local/bin/zuse-cloud-runtime');meta=runtime.lstat();require(stat.S_ISREG(meta.st_mode) and meta.st_uid==0 and not meta.st_mode&0o022,'publication_runtime_owner_invalid')
 h=hashlib.sha256()
 with runtime.open('rb') as source:
  while chunk:=source.read(1048576):h.update(chunk)
 require(h.hexdigest()==args.runtime_sha256,'publication_runtime_sha_mismatch')
 contents_clean(data,uid,checkout,status,recovery)
 helper=Path('/usr/local/lib/zuse/runtime-supervisor.py');meta=helper.lstat();require(stat.S_ISREG(meta.st_mode) and meta.st_uid==0 and not meta.st_mode&0o022,'publication_helper_owner_invalid')
 spec=importlib.util.spec_from_file_location('builder_supervisor',helper);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
 unit=Path('/etc/systemd/system/zuse-runtime-supervisor.service');unit_meta=unit.lstat();require(stat.S_ISREG(unit_meta.st_mode) and unit_meta.st_uid==0 and unit_meta.st_nlink==1 and not unit_meta.st_mode&0o022,'publication_boot_unit_owner_invalid')
 unit_body=unit.read_bytes();require(len(unit_body)<=16384,'publication_boot_unit_invalid');unit_sha256=hashlib.sha256(unit_body).hexdigest()
 disabled=getattr(args,'disabled_boat_owner',False);require(not disabled or not args.legacy_builder,'publication_disabled_legacy_forbidden');expected_unit='disabled' if disabled else 'enabled'
 before=service();require(before.get('ActiveState')=='active' and before.get('UnitFileState')==expected_unit,'publication_enabled_boot_owner_required')
 ping=module.rpc(data,{'version':1,'action':'ping'});pid=ping.get('residentPid');require(isinstance(pid,int) and pid>1 and str(pid)==before.get('MainPID'),'publication_resident_identity_mismatch')
 require(ping.get('bound') is False and ping.get('childAlive') is False and ping.get('recoveryAlive') is False,'publication_runtime_bound')
 fact=process_fact(pid);require(fact is not None,'publication_resident_missing');start_ticks=fact[1]
 acknowledged=False
 if args.legacy_builder:
  # This compatibility path is ONLY a clean, authority-free parent builder.
  # No enrollment/token/data may exist; it never handles a user task.
  contents_clean(data,uid,checkout,status,recovery)
 else:
  require(ping.get('publicationStopVersion')==1,'publication_supervisor_update_required')
  try:
   receipt=module.rpc(data,dict(version=1,action='publication-stop',operationId=args.operation_id,expectedBootNonce=ping['bootNonce']))
   require(receipt.get('state')=='idle-stopping' and receipt.get('residentPid')==pid and receipt.get('bootNonce')==ping['bootNonce'] and receipt.get('operationId')==args.operation_id,'publication_stop_receipt_invalid');acknowledged=True
  except module.Rejected as error:
   code=str(error);raise Rejected(code if len(code)<=96 and code.startswith('publication_') and all(c in 'abcdefghijklmnopqrstuvwxyz_' for c in code) else 'publication_stop_rejected')
  except OSError:
   current=process_fact(pid);require(current is None or current[0]=='Z' or current[1]!=start_ticks,'publication_stop_outcome_uncertain')
 current=service();require(current.get('MainPID') in ('0',str(pid)),'publication_resident_changed')
 current_fact=process_fact(pid);require(current.get('MainPID')=='0' or current_fact is not None and current_fact[1]==start_ticks,'publication_resident_changed')
 subprocess.run(['systemctl','stop','zuse-runtime-supervisor.service'],check=True,timeout=12,env={'PATH':'/usr/bin:/bin'})
 deadline=time.monotonic()+5
 while time.monotonic()<deadline:
  fact=process_fact(pid)
  if fact is None or fact[0]=='Z' or fact[1]!=start_ticks:break
  time.sleep(.05)
 after=service();verify_stopped_service(after,expected_unit)
 require(hashlib.sha256(unit.read_bytes()).hexdigest()==unit_sha256,'publication_boot_unit_changed')
 receipt=clean_stopped(data,uid,pid,start_ticks,checkout,status,recovery)
 receipt.update(operationId=args.operation_id,residentPid=pid,residentStartTicks=start_ticks,bootNonceSha256=hashlib.sha256(ping['bootNonce'].encode()).hexdigest(),atomicStopAcknowledged=acknowledged,legacyBuilder=args.legacy_builder,enabledBootServiceRetained=not disabled,**({'disabledBootServiceRetained':True} if disabled else {}),bootServiceSha256=unit_sha256,runtimeSha256=args.runtime_sha256)
 return receipt
def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--runtime-sha256',required=True);parser.add_argument('--operation-id',required=True);parser.add_argument('--legacy-builder',action='store_true');parser.add_argument('--disabled-boat-owner',action='store_true');parser.add_argument('--report-only',action='store_true');args=parser.parse_args()
 try:
  require(len(args.runtime_sha256)==64 and all(c in '0123456789abcdef' for c in args.runtime_sha256),'publication_runtime_sha_invalid')
  require(1<=len(args.operation_id)<=160 and all(c in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.:-' for c in args.operation_id),'publication_operation_invalid')
  result=stop_builder(args);success=True
 except Rejected as error:result=dict(version=1,status='rejected',code=str(error));success=False
 except (OSError,ValueError,subprocess.SubprocessError) as error:result=dict(version=1,status='rejected',code='publication_inspection_unavailable',errorClass=type(error).__name__);success=False
 print(json.dumps(result,sort_keys=True))
 if not success and not args.report_only:raise SystemExit(1)
if __name__=='__main__':main()
