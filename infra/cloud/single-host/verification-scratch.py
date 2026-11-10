#!/usr/bin/env python3
"""Inspect or clean bounded staging verification scratch; no mount/service actions.

The default command is read-only. Cleanup requires both staging services stopped
and every original per-workspace effects flock. Locks themselves are preserved.
"""
import argparse,fcntl,json,os,re,shutil,stat,subprocess
from pathlib import Path
UID=10001
MIN_FREE=128*1024*1024
MIN_LIMIT=256*1024*1024
MAX_LIMIT=1024*1024*1024

def require(ok,code):
 if not ok:raise ValueError(code)
def private(path,uid,kind):
 for parent in (path,*path.parents):require(not parent.is_symlink(),'verification_scratch_symlink')
 meta=path.lstat();require(meta.st_uid==uid and stat.S_IMODE(meta.st_mode)==(0o700 if kind=='directory' else 0o600) and (stat.S_ISDIR(meta.st_mode) if kind=='directory' else stat.S_ISREG(meta.st_mode) and meta.st_nlink==1),'verification_scratch_owner_invalid');return meta

def inspect(root,name,settings,*,run=subprocess.run,mountpoint=os.path.ismount,space=os.statvfs,require_free=True):
 configured=settings.get('VERIFICATION_SCRATCH_DIR','');limit=settings.get('VERIFICATION_SCRATCH_LIMIT_BYTES','')
 if not configured:
  require(not limit,'verification_scratch_incomplete');return None
 require(name=='staging','verification_scratch_staging_only')
 mountroot=root/name/'verification-scratch-fs';path=mountroot/'private';backing=root/name/'verification-scratch.img'
 require(configured==str(path) and limit.isdigit() and MIN_LIMIT<=int(limit)<=MAX_LIMIT,'verification_scratch_configuration_invalid')
 private(mountroot,0,'directory');private(path,UID,'directory');file=private(backing,0,'file');require(file.st_size==int(limit),'verification_scratch_backing_size_invalid')
 require(mountpoint(mountroot),'verification_scratch_not_mounted')
 result=run(['findmnt','--json','--mountpoint',str(mountroot),'--output','TARGET,SOURCE,FSTYPE,OPTIONS'],capture_output=True,text=True,timeout=5)
 require(result.returncode==0,'verification_scratch_mount_unavailable');mounts=json.loads(result.stdout).get('filesystems',[]);require(len(mounts)==1,'verification_scratch_mount_invalid');mount=mounts[0]
 require(mount.get('target')==str(mountroot) and mount.get('fstype')=='ext4' and re.fullmatch(r'/dev/loop[0-9]+',mount.get('source','')) and {'rw','nodev','nosuid','noexec'}<=set(mount.get('options','').split(',')),'verification_scratch_mount_invalid')
 result=run(['losetup','--json','--output','NAME,BACK-FILE',mount['source']],capture_output=True,text=True,timeout=5)
 require(result.returncode==0,'verification_scratch_loop_unavailable');loops=json.loads(result.stdout).get('loopdevices',[])
 require(len(loops)==1 and loops[0].get('name')==mount['source'] and loops[0].get('back-file')==str(backing),'verification_scratch_backing_mismatch')
 fs=space(path);total=fs.f_blocks*fs.f_frsize;free=fs.f_bavail*fs.f_frsize
 require(MIN_LIMIT//2<=total<=int(limit) and (not require_free or free>=MIN_FREE),'verification_scratch_capacity_insufficient')
 return {'version':1,'environment':'staging','path':str(path),'limitBytes':int(limit),'filesystemBytes':total,'freeBytes':free,'private':True,'hardQuota':True}

def cleanup(root,settings,*,run=subprocess.run):
 proof=inspect(root,'staging',settings,run=run,require_free=False);require(proof is not None,'verification_scratch_not_configured')
 # docker ps lists running only. No stop/restart or provider request is issued.
 for service in ('api','worker'):
  result=run(['docker','ps','-q','--filter','label=com.docker.compose.project=zuse-staging','--filter','label=com.docker.compose.service='+service],capture_output=True,text=True,timeout=5)
  require(result.returncode==0 and not result.stdout.strip(),'verification_scratch_services_running')
 base=Path(proof['path'])/'zuse-cloud-effects';private(base,UID,'directory');verification=base/'verification';locks=base/'locks';private(verification,UID,'directory');private(locks,UID,'directory')
 entries=list(verification.iterdir());held=[];targets=[]
 try:
  for entry in entries:
   require(re.fullmatch('[a-f0-9]{64}',entry.name),'verification_scratch_unknown_workspace');private(entry,UID,'directory')
   lockdir=locks/entry.name;private(lockdir,UID,'directory');lockfile=lockdir/'.workspace-runtime.lock';private(lockfile,UID,'file')
   fd=os.open(lockfile,os.O_RDWR|os.O_NOFOLLOW|os.O_CLOEXEC);held.append(fd)
   try:fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
   except BlockingIOError:raise ValueError('verification_scratch_workspace_busy')
   for child in entry.iterdir():
    require(re.fullmatch(r'\.tmp[A-Za-z0-9_-]{6,64}',child.name),'verification_scratch_unknown_temporary');private(child,UID,'directory');targets.append(child)
  # All guards precede deletion. shutil.rmtree does not follow archive symlinks.
  for child in targets:shutil.rmtree(child)
 finally:
  for fd in reversed(held):os.close(fd)
 return {'status':'cleaned','temporaryDirectoriesRemoved':len(targets),'effectsLocksPreserved':True}

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--config-root',type=Path,default=Path('/etc/zuse/cloud'));p.add_argument('--cleanup-stopped',action='store_true');a=p.parse_args()
 require(os.geteuid()==0,'verification_scratch_root_required')
 settings={}
 for line in (a.config_root/'staging/deploy.env').read_text().splitlines():
  if line.startswith(('VERIFICATION_SCRATCH_DIR=','VERIFICATION_SCRATCH_LIMIT_BYTES=')):
   key,value=line.split('=',1);require(key not in settings,'verification_scratch_duplicate_setting');settings[key]=value
 print(json.dumps(cleanup(a.config_root,settings) if a.cleanup_stopped else inspect(a.config_root,'staging',settings),sort_keys=True))
if __name__=='__main__':
 try:main()
 except Exception:print(json.dumps({'status':'rejected','errorCode':'verification_scratch_preflight_failed'}));raise SystemExit(1)
