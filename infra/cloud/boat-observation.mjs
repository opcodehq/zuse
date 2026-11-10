// Read-only diagnostics for the original credential-free managed-base probe.
// Fixed paths and metadata only: never env dumps, process args or user histories.
export function boatObservationCommand(runId) {
 if(!/^zuse-base-build-[a-f0-9]{24}$/.test(runId))throw new Error('InvalidOriginalBuilderIdentity');
 const script=String.raw`
import os,json,stat,socket,struct,subprocess,hashlib,pwd,errno
from pathlib import Path
DATA='/home/zuse/.zuse-data';ADDRESS=DATA+'/.runtime-supervision/control.sock'
def error(e):return errno.errorcode.get(getattr(e,'errno',None),'UNAVAILABLE')
def fact(path):
 try:
  m=os.lstat(path);return dict(uid=m.st_uid,gid=m.st_gid,mode=oct(stat.S_IMODE(m.st_mode)),inode=str(m.st_ino),device=str(m.st_dev),mtimeNs=str(m.st_mtime_ns),kind='socket' if stat.S_ISSOCK(m.st_mode) else 'directory' if stat.S_ISDIR(m.st_mode) else 'file' if stat.S_ISREG(m.st_mode) else 'symlink' if stat.S_ISLNK(m.st_mode) else 'other')
 except OSError as e:return dict(error=error(e))
def unix(path):
 try:return [dict(inode=parts[6],flags=parts[3],type=parts[4],state=parts[5]) for line in Path(path).read_text().splitlines()[1:] for parts in [line.split()] if len(parts)>=8 and parts[7]==ADDRESS][:16]
 except OSError as e:return dict(error=error(e))
def process(pid):
 try:
  raw=Path('/proc',str(pid),'stat').read_text();p=raw[raw.rfind(')')+2:].split();status=Path('/proc',str(pid),'status').read_text().splitlines()
  result=dict(pid=pid,state=p[0],startTicks=int(p[19]),uids=[int(v) for line in status if line.startswith('Uid:') for v in line.split()[1:]],gids=[int(v) for line in status if line.startswith('Gid:') for v in line.split()[1:]])
  result['namespaces']={kind:os.readlink('/proc/'+str(pid)+'/ns/'+kind) for kind in ['mnt','user','net','pid']}
  result['socketFromProcessRoot']=fact('/proc/'+str(pid)+'/root'+ADDRESS);result['boundUnixPath']=unix('/proc/'+str(pid)+'/net/unix')
  result['socketFds']=[]
  for fd in Path('/proc',str(pid),'fd').iterdir():
   try:
    target=os.readlink(fd)
    if target.startswith('socket:[') and target.endswith(']'):result['socketFds'].append(dict(fd=int(fd.name),inode=target[8:-1]))
   except (OSError,ValueError):pass
  return result
 except (OSError,ValueError,IndexError) as e:return dict(pid=pid,error=error(e))
user_probe=r'''
import os,json,socket,struct,errno,hashlib
address='/home/zuse/.zuse-data/.runtime-supervision/control.sock'
r=dict(uid=os.getuid(),euid=os.geteuid())
r['traversal']={p:os.access(p,os.X_OK) for p in ['/','/home','/home/zuse','/home/zuse/.zuse-data','/home/zuse/.zuse-data/.runtime-supervision','/home/zuse/workspace','/usr','/usr/local','/usr/local/bin']}
r['readability']={p:dict(read=os.access(p,os.R_OK),execute=os.access(p,os.X_OK),write=os.access(p,os.W_OK)) for p in ['/home/zuse/workspace/supervisor-dirty-work','/usr/local/bin/zuse-boat-retention-canary','/home/zuse/.zuse-data/.runtime-supervision/control.sock']}
try:
 with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as s:
  s.settimeout(.5);s.connect(address);r['connected']=True;r['peer']=list(struct.unpack('3i',s.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12)))
  s.sendall(b'{"version":1,"action":"ping"}\n');body=b''
  while b'\n' not in body and len(body)<4096:
   part=s.recv(min(4096-len(body),4096))
   if not part:break
   body+=part
  v=json.loads(body.split(b'\n',1)[0]);r['ping']={k:v[k] for k in ['version','bound','childAlive','recoveryAlive','repairVersion','recreationVersion','residentPid'] if k in v}
  if isinstance(v.get('bootNonce'),str):r['ping']['bootNonceSha256']=hashlib.sha256(v['bootNonce'].encode()).hexdigest()
  if 'error' in v:r['rpcError']=v['error'] if isinstance(v['error'],str) and len(v['error'])<=128 and all(c in 'abcdefghijklmnopqrstuvwxyz0123456789_' for c in v['error']) else 'unclassified'
except OSError as e:r['connectError']=errno.errorcode.get(e.errno,'TIMEOUT' if isinstance(e,TimeoutError) else 'UNAVAILABLE')
except (ValueError,UnicodeDecodeError):r['responseError']='INVALID_JSON'
print(json.dumps(r))
'''
r=dict(schema=1,callerUid=os.getuid(),callerEffectiveUid=os.geteuid(),sudoCallerUid=int(os.environ['SUDO_UID']) if os.environ.get('SUDO_UID','').isdigit() else None,paths={key:fact(path) for key,path in [('data',DATA),('ipc',DATA+'/.runtime-supervision'),('socket',ADDRESS),('supervisor','/usr/local/lib/zuse/runtime-supervisor.py')]},boundUnixPath=unix('/proc/net/unix'))
try:r['bootId']=Path('/proc/sys/kernel/random/boot_id').read_text().strip()
except OSError:r['bootIdUnavailable']=True
r['callerNamespaces']={kind:os.readlink('/proc/self/ns/'+kind) for kind in ['mnt','user','net','pid']}
r['parentTraversal']={p:fact(p) for p in ['/','/home','/home/zuse',DATA,DATA+'/.runtime-supervision','/home/zuse/workspace','/usr','/usr/local','/usr/local/bin']}
r['dataOverride']={'present':'ZUSE_USER_DATA' in os.environ,'matchesCanonical':os.environ.get('ZUSE_USER_DATA',DATA)==DATA}
try:
 fd=os.open('/usr/local/lib/zuse/runtime-supervisor.py',os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
 try:
  m=os.fstat(fd);body=os.read(fd,65537) if stat.S_ISREG(m.st_mode) else b''
  if stat.S_ISREG(m.st_mode) and len(body)<=65536:r['supervisorSha256']=hashlib.sha256(body).hexdigest()
 finally:os.close(fd)
except OSError:r['supervisorHashUnavailable']=True
try:
 result=subprocess.run(['systemctl','show','zuse-runtime-supervisor.service','--property=LoadState,ActiveState,SubState,Result,ExecMainCode,ExecMainStatus,NRestarts,MainPID'],capture_output=True,text=True,timeout=3)
 r['service']={k:v for line in result.stdout.splitlines() if '=' in line for k,v in [line.split('=',1)] if k in ['LoadState','ActiveState','SubState','Result','ExecMainCode','ExecMainStatus','NRestarts','MainPID'] and len(v)<=96 and all(c.isalnum() or c in '_.-' for c in v)}
 pid=r['service'].get('MainPID','0');r['residentProcess']=process(int(pid)) if pid.isdigit() and int(pid)>0 else dict(error='NO_PID')
except (OSError,subprocess.TimeoutExpired):r['serviceUnavailable']=True
for label,cmd in [('rootPing',['/usr/bin/python3','-c',user_probe]),('userPing',['sudo','-n','-u','zuse','/usr/bin/python3','-c',user_probe])]:
 try:
  p=subprocess.run(cmd,capture_output=True,text=True,timeout=3,env={'PATH':'/usr/bin:/bin','HOME':'/home/zuse','LANG':'C.UTF-8'});r[label]=json.loads(p.stdout) if p.returncode==0 and len(p.stdout)<8192 else dict(exitCode=p.returncode,code='SUDO_OR_PROBE_FAILED')
 except subprocess.TimeoutExpired:r[label]=dict(code='PROBE_TIMEOUT')
 except (OSError,ValueError):r[label]=dict(code='PROBE_UNAVAILABLE')
marker=MARKER
for label,path in [('dirtyData','/home/zuse/workspace/supervisor-dirty-work'),('software','/usr/local/bin/zuse-boat-retention-canary')]:
 r[label]=dict(metadata=fact(path))
 try:
  fd=os.open(path,os.O_RDONLY|os.O_CLOEXEC|os.O_NOFOLLOW)
  try:
   m=os.fstat(fd)
   if not stat.S_ISREG(m.st_mode) or m.st_size>4096:raise ValueError()
   body=os.read(fd,4097);r[label].update(bytes=len(body),sha256=hashlib.sha256(body).hexdigest())
  finally:os.close(fd)
  if label=='dirtyData':r[label]['matchesExpected']=body==marker.encode()
  else:
   expected=('#!/bin/sh\nprintf "%s" '+marker+'\n').encode();r[label]['matchesExpected']=body==expected
   if body==expected and m.st_uid==0 and not m.st_mode&0o022:
    p=subprocess.run(['sudo','-n','-u','zuse',path],capture_output=True,timeout=3,env={'PATH':'/usr/bin:/bin','HOME':'/home/zuse','LANG':'C.UTF-8'});r[label].update(executed=p.returncode==0,outputMatches=p.stdout==marker.encode())
 except (OSError,ValueError,subprocess.TimeoutExpired) as e:r[label]['readCode']=error(e)
print(json.dumps(r,separators=(',',':')))
`.replace('marker=MARKER',`marker=${JSON.stringify('retained-work-'+runId)}`);
 const encoded=Buffer.from(script).toString('base64');
 return `printf %s ${encoded} | base64 -d | sudo -n python3`;
}
export async function observeBoatOriginal(adapter,id,runId) {
 const value=JSON.parse(await adapter.exec(id,boatObservationCommand(runId)));
 if(value?.schema!==1||typeof value.paths!=='object')throw new Error('OriginalObservationInvalid');
 return value;
}
