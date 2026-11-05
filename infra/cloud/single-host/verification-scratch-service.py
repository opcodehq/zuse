#!/usr/bin/env python3
"""Boot only the staging pair after its existing bounded scratch FS is mounted.
Never creates/formats a backing file, changes credentials, or touches production.
"""
import importlib.util,json,os,subprocess
from pathlib import Path
HERE=Path(__file__).resolve().parent
ROOT=Path('/etc/zuse/cloud')
def load(name,path):
 s=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m

def start(root=ROOT,*,run=subprocess.run,mountpoint=os.path.ismount):
 if os.geteuid()!=0:raise ValueError('scratch_root_required')
 deploy=load('scratch_deploy',HERE/'deploy.py');scratch=load('scratch_preflight',HERE/'verification-scratch.py')
 settings=deploy.read_env(root/'staging/deploy.env');scratch.require(settings.get('ZUSE_ENV')=='staging','scratch_staging_required')
 mountroot=root/'staging/verification-scratch-fs';backing=root/'staging/verification-scratch.img'
 scratch.private(mountroot,0,'directory');metadata=scratch.private(backing,0,'file')
 scratch.require(settings.get('VERIFICATION_SCRATCH_DIR')==str(mountroot/'private') and settings.get('VERIFICATION_SCRATCH_LIMIT_BYTES')==str(metadata.st_size) and scratch.MIN_LIMIT<=metadata.st_size<=scratch.MAX_LIMIT,'scratch_binding_invalid')
 if not mountpoint(mountroot):
  # This CHILD exists only inside the mounted filesystem. A reboot cannot bind
  # the underlying host directory and silently spill verification onto rootfs.
  scratch.require(not os.path.lexists(mountroot/'private'),'scratch_unmounted_child_present')
  filesystem=run(['blkid','-p','-s','TYPE','-o','value',str(backing)],capture_output=True,text=True,timeout=5)
  scratch.require(filesystem.returncode==0 and filesystem.stdout.strip()=='ext4','scratch_filesystem_invalid')
  result=run(['mount','-o','loop,nodev,nosuid,noexec',str(backing),str(mountroot)],capture_output=True,text=True,timeout=15)
  scratch.require(result.returncode==0,'scratch_mount_failed')
 scratch.inspect(root,'staging',settings)
 command,env=deploy.compose(root,'staging')
 env['DOCKER_HOST']='unix:///var/run/docker.sock'
 for key in ('DOCKER_CONTEXT','DOCKER_TLS_VERIFY','DOCKER_CERT_PATH'):env.pop(key,None)
 result=run(command+['up','--detach','--no-deps','--no-build','--pull','never','--wait','--wait-timeout','60','api','worker'],env=env,capture_output=True,text=True,timeout=90)
 scratch.require(result.returncode==0,'scratch_stage_pair_start_failed')
 return {'status':'staging-pair-started','authWorkerChanged':False,'productionChanged':False,'backingFormatted':False}
if __name__=='__main__':
 try:print(json.dumps(start(),sort_keys=True))
 except Exception:print(json.dumps({'status':'rejected','errorCode':'staging_scratch_boot_failed'}));raise SystemExit(1)
