#!/usr/bin/env python3
"""Credential-free managed gh proof. Metadata, fixed wrapper bytes and --version only."""
import argparse,hashlib,json,os,stat,subprocess,time,errno
from pathlib import Path
WRAPPER=b'#!/bin/sh\nexec /usr/local/bin/zuse-cloud-runtime github-from-profile "$@"\n'
CANONICAL='/opt/zuse/bin/gh'
PROFILE=b'# Zuse managed commands; retain an existing task-specific PATH prefix.\ncase ":${PATH:-}:" in\n  *":/opt/zuse/bin:"*) ;;\n  *) PATH="/opt/zuse/bin:${PATH:-/usr/bin:/bin}" ;;\nesac\nexport PATH\n'
ENV={'PATH':'/usr/bin:/bin','HOME':'/home/zuse','LANG':'C.UTF-8','GH_CONFIG_DIR':'/nonexistent'}
def metadata(path,limit):
 try:
  m=path.lstat();r=dict(lexists=True,regular=stat.S_ISREG(m.st_mode),directory=stat.S_ISDIR(m.st_mode),symlink=stat.S_ISLNK(m.st_mode),uid=m.st_uid,gid=m.st_gid,mode=oct(stat.S_IMODE(m.st_mode)),bytes=m.st_size,nlink=m.st_nlink)
  if not r['regular'] or m.st_size>limit:return r,None
  fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
  try:
   now=os.fstat(fd)
   if (now.st_ino,now.st_dev)!=(m.st_ino,m.st_dev):return dict(code='FILE_REPLACED'),None
   h=hashlib.sha256();body=bytearray() if limit<=1024 else None
   with os.fdopen(fd,'rb',closefd=False) as f:
    for chunk in iter(lambda:f.read(1024*1024),b''):
     h.update(chunk)
     if body is not None:body.extend(chunk)
   r['sha256']=h.hexdigest();return r,bytes(body) if body is not None else None
  finally:os.close(fd)
 except OSError as e:return dict(lexists=os.path.lexists(path),code=__import__('errno').errorcode.get(e.errno,'UNAVAILABLE')),None

def valid_receipt(receipt):
 if not isinstance(receipt,dict):return False
 fields=set(receipt)
 if fields not in ({'version','sha256'},{'version','sha256','wrapperSha256','loginShellResolvedManaged'}):return False
 version=receipt['version'];digest=receipt['sha256']
 if not isinstance(version,str) or not version.startswith('gh version 2.') or len(version)>160 or any(ord(c)<32 or ord(c)>126 for c in version):return False
 if not isinstance(digest,str) or len(digest)!=64 or any(c not in '0123456789abcdef' for c in digest):return False
 if 'wrapperSha256' in receipt:
  wrapper=receipt['wrapperSha256']
  if not isinstance(wrapper,str) or len(wrapper)!=64 or any(c not in '0123456789abcdef' for c in wrapper) or receipt['loginShellResolvedManaged'] is not True:return False
 return True

def inspect(receipt,*,filesystem=Path('/'),run=subprocess.run):
 if not valid_receipt(receipt):return dict(version=1,required=True,status='rejected',code='managed_github_receipt_invalid',accountRequired=False,modelCalled=False)
 at=lambda p:filesystem/p.lstrip('/')
 global_file,body=metadata(at(CANONICAL),1024);system,_=metadata(at('/usr/bin/gh'),128*1024*1024)
 global_file['contentMatches']=body==WRAPPER
 result=dict(version=1,required=True,status='rejected',globalShim=global_file,system=system,accountRequired=False,modelCalled=False)
 result['canonicalPath']=CANONICAL
 result['ancestors']={p:metadata(at(p),0)[0] for p in ('/opt','/opt/zuse','/opt/zuse/bin','/etc','/etc/profile.d')}
 profile,profile_body=metadata(at('/etc/profile.d/zz-zuse-managed-bin.sh'),1024)
 result['profile']=profile
 result['ancestorsControlled']=all(v.get('directory') and not v.get('symlink') and v.get('uid')==0 and not int(v.get('mode','0o777')[2:],8)&0o022 and int(v.get('mode','0o000')[2:],8)&0o005==0o005 for v in result['ancestors'].values())
 result['profileMatches']=profile.get('regular') and not profile.get('symlink') and profile.get('uid')==0 and profile.get('nlink')==1 and profile.get('mode')=='0o644' and profile_body==PROFILE

 def rejected(code):result['code']=code;return result
 if not result['ancestorsControlled']:return rejected('managed_github_ancestor_invalid')
 if not result['profileMatches']:return rejected('managed_github_profile_invalid')
 if not(global_file.get('regular') and not global_file.get('symlink') and global_file.get('uid')==0 and global_file.get('nlink')==1 and global_file.get('mode')=='0o755' and body==WRAPPER):return rejected('managed_github_global_shim_invalid')
 if 'wrapperSha256' in receipt and receipt['wrapperSha256']!=global_file.get('sha256'):return rejected('managed_github_wrapper_receipt_invalid')
 if not(system.get('regular') and not system.get('symlink') and system.get('uid')==0 and not int(system.get('mode','0o777')[2:],8)&0o022 and system.get('sha256')==receipt['sha256']):return rejected('managed_github_system_receipt_invalid')
 observations=[];result['probeObservations']=observations
 def execute(stage,args):
  started=time.monotonic();fact=dict(stage=stage)
  try:
   value=run(args,capture_output=True,text=True,timeout=5,env=ENV)
   fact.update(outcome='completed',exitCode=value.returncode);return value
  except subprocess.TimeoutExpired:
   fact.update(outcome='timeout');return None
  except OSError as error:
   fact.update(outcome='os-error',errno=errno.errorcode.get(error.errno,'UNAVAILABLE'));return None
  finally:
   fact['elapsedMs']=round((time.monotonic()-started)*1000,2);observations.append(fact)
 login=execute('login-path',['sudo','-n','-u','zuse','/bin/bash','-lc','command -v gh'])
 if login is None:return rejected('managed_github_cli_probe_unavailable')
 result['loginShellMatchesGlobal']=login.returncode==0 and login.stdout.strip()==CANONICAL and len(login.stdout)<256
 if not result['loginShellMatchesGlobal']:return rejected('managed_github_login_path_invalid')
 version=execute('managed-version',['sudo','-n','-u','zuse',CANONICAL,'--version'])
 if version is None:return rejected('managed_github_cli_probe_unavailable')
 result['globalVersionMatchesReceipt']=version.returncode==0 and len(version.stdout)<1024 and version.stdout.splitlines() and version.stdout.splitlines()[0]==receipt['version']
 if result['globalVersionMatchesReceipt'] is not True:return rejected('managed_github_global_version_invalid')
 result.update(status='passed',systemReceipt=receipt);return result

def main():
 p=argparse.ArgumentParser();p.add_argument('--receipt',required=True);a=p.parse_args();r=json.loads(a.receipt)
 if os.geteuid()!=0 or not valid_receipt(r):raise SystemExit('managed_github_receipt_invalid')
 print(json.dumps(inspect(r),sort_keys=True))
if __name__=='__main__':main()
