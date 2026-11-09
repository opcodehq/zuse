#!/usr/bin/env python3
"""Bounded, credential-free native/guest/wrapper --version diagnosis.

Never emits child stdout/stderr or inherited environment. No profile/account RPC.
"""
import argparse,hashlib,json,os,stat,subprocess,time,errno
from pathlib import Path
COMMANDS={'system':['/usr/bin/gh','--version'],'runtime':['/usr/local/bin/zuse-cloud-runtime','github-from-profile','--version'],'wrapper':['/opt/zuse/bin/gh','--version'],'runtime-version':['/usr/local/bin/zuse-cloud-runtime','--version']}
ENV={'PATH':'/opt/zuse/bin:/usr/local/bin:/usr/bin:/bin','LANG':'C.UTF-8','GH_CONFIG_DIR':'/nonexistent'}
def diagnose(expected,probe,user,*,filesystem=Path('/'),run=subprocess.run):
 assert probe in COMMANDS and user in ('root','zuse')
 path=filesystem/'usr/local/bin/zuse-cloud-runtime';fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
 try:
  m=os.fstat(fd);assert stat.S_ISREG(m.st_mode) and m.st_uid==0 and not m.st_mode&0o022
  h=hashlib.sha256()
  with os.fdopen(fd,'rb',closefd=False) as file:
   for block in iter(lambda:file.read(1024*1024),b''):h.update(block)
  assert h.hexdigest()==expected,'diagnostic_runtime_hash_mismatch'
 finally:os.close(fd)
 argv=['sudo','-n','-u',user,'--','/usr/bin/env','-i',*[k+'='+v for k,v in ENV.items()],'HOME='+('/root' if user=='root' else '/home/zuse'),*COMMANDS[probe]]
 result={'version':1,'probe':probe,'user':user,'deadlineSeconds':5,'runtimeSha256':expected,'accountRequired':False,'modelCalled':False}
 start=time.monotonic()
 try:
  p=run(argv,stdin=subprocess.DEVNULL,capture_output=True,timeout=5)
  result.update(outcome='completed',exitCode=p.returncode,stdoutBytes=len(p.stdout),stderrBytes=len(p.stderr),versionPrefix=p.stdout.startswith(b'gh version 2.') if probe!='runtime-version' else p.stdout.startswith(b'zuse-cloud-runtime '))
 except subprocess.TimeoutExpired:result.update(outcome='timeout')
 except OSError as error:result.update(outcome='os-error',errno=errno.errorcode.get(error.errno,'UNAVAILABLE'))
 result['elapsedMs']=round((time.monotonic()-start)*1000,2)
 return result
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--runtime-sha256',required=True);p.add_argument('--probe',choices=COMMANDS,required=True);p.add_argument('--as-user',choices=('root','zuse'),required=True);a=p.parse_args()
 if os.geteuid()!=0 or len(a.runtime_sha256)!=64 or any(c not in '0123456789abcdef' for c in a.runtime_sha256):raise SystemExit('diagnostic_scope_invalid')
 print(json.dumps(diagnose(a.runtime_sha256,a.probe,a.as_user),sort_keys=True))
