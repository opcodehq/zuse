#!/usr/bin/env python3
"""Initial clean memory-snapshot bootstrap, before any task credential delivery.

Retained Boat startup is a separate explicitly bound action. No process retirement, restart, unlink,
credential, account, repository or model operations are supported.
"""
import argparse,base64,errno,fcntl,hashlib,json,os,re,socket,stat,struct,subprocess,time,urllib.parse
from pathlib import Path
VERSION=1
UNIT='zuse-runtime-supervisor.service'
HELPER='/usr/local/lib/zuse/managed-base-fresh-boot.py'
PIN_NAMES={'runtimeSha256','supervisorSha256','serviceSha256','manifestSha256','helperSha256'}
# Optional sixth pin: absent on bases without the root runtime swap helper,
# verified whenever present. runtimeSha256 may be the computer's own runtime
# release (after an in-place update), not the base's initial runtime.
OPTIONAL_PIN_NAMES={'runtimeUpdateHelperSha256'}
UPDATE_HELPER='/usr/local/libexec/zuse/runtime-update.py'
FIELDS={'version','provider','purpose','operationId','workspaceId','generation','storageIncarnationId','sourceMachineId','imagePins'}
PURPOSES={'initial-allocation','initial-restore','publication-proof','cache-preparation'}
SAFE=re.compile(r'[A-Za-z0-9_.:-]{1,180}\Z')
class Rejected(Exception):pass
class Pending(Exception):pass
def require(value,code):
 if not value:raise Rejected(code)
def canonical(value):return json.dumps(value,sort_keys=True,separators=(',',':')).encode()
def digest(value):return hashlib.sha256(value).hexdigest()
def validated(request):
 require(isinstance(request,dict) and set(request)==FIELDS and type(request.get('version')) is int and request['version']==1,'fresh_boot_request_invalid')
 require(request['provider'] in ('boxd','boat') and (request['purpose'] in PURPOSES or request['provider']=='boat' and request['purpose']=='retained-resume'),'fresh_boot_scope_invalid')
 require(type(request['generation']) is int and 1<=request['generation']<2**63,'fresh_boot_generation_invalid')
 for key in ('operationId','workspaceId','storageIncarnationId','sourceMachineId'):require(isinstance(request[key],str) and SAFE.fullmatch(request[key]),'fresh_boot_binding_invalid')
 pins=request['imagePins'];require(isinstance(pins,dict) and PIN_NAMES<=set(pins)<=PIN_NAMES|OPTIONAL_PIN_NAMES and all(isinstance(v,str) and re.fullmatch('[a-f0-9]{64}',v) for v in pins.values()),'fresh_boot_pins_invalid')
 return request
class Layout:
 def __init__(self,root=Path('/'),owner=0,user=10001):self.root=Path(root);self.owner=owner;self.user=user
 def at(self,path):return self.root/path.lstrip('/')
 @property
 def data(self):return self.at('/home/zuse/.zuse-data')
 @property
 def journal(self):return self.at('/opt/zuse/runtime-bootstrap')
def no_symlink(path):
 for current in (path,*path.parents):
  if os.path.lexists(current):require(not current.is_symlink(),'fresh_boot_path_symlink')
def protected_bytes(path,expected,owners,*,limit=1048576,retain=True):
 no_symlink(path);fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
 try:
  before=os.fstat(fd);require(stat.S_ISREG(before.st_mode) and before.st_uid in owners and before.st_nlink==1 and not before.st_mode&0o022 and before.st_size<=limit,'fresh_boot_file_unsafe')
  h=hashlib.sha256();body=[];size=0
  while part:=os.read(fd,min(1048576,limit+1-size)):
   size+=len(part);require(size<=limit,'fresh_boot_file_too_large');h.update(part)
   if retain:body.append(part)
  after=os.fstat(fd);require((before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns,before.st_ctime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns,after.st_ctime_ns),'fresh_boot_file_changed')
  require(h.hexdigest()==expected,'fresh_boot_file_hash_mismatch');return b''.join(body)
 finally:os.close(fd)
def private_directory(path,uid):
 no_symlink(path);m=path.lstat();require(stat.S_ISDIR(m.st_mode) and m.st_uid==uid and stat.S_IMODE(m.st_mode)==0o700,'fresh_boot_private_directory_invalid')
def authority_free(layout):
 private_directory(layout.data,layout.user)
 require(set(p.name for p in layout.data.iterdir())<= {'.runtime-supervision'},'fresh_boot_runtime_authority_present')
 for name in ('/home/zuse/workspace','/var/lib/zuse/workspace','/var/lib/zuse/recovery'):
  path=layout.at(name);no_symlink(path);m=path.lstat();require(stat.S_ISDIR(m.st_mode) and m.st_uid==layout.user and not any(path.iterdir()),'fresh_boot_task_data_present')
 # Credential-free project cache payloads live under /opt/zuse/project-caches;
 # they are intentionally neither removed nor altered by this helper.
 for name in tuple('/home/zuse/'+name for name in ('.codex/auth.json','.claude/.credentials.json','.grok/auth.json','.config/grok/auth.json','.config/gh','.zuse-github-cli','.git-credentials','.netrc','.zuse/cloud-auth','.zuse-image/provider-secrets.json','.zuse-data/cloud-runtime-identity.json','.zuse-data/rust-repository-ready.json')):
  require(not os.path.lexists(layout.at(name)),'fresh_boot_agent_authority_present')
 ssh=layout.at('/home/zuse/.ssh')
 if os.path.lexists(ssh):
  no_symlink(ssh);require(stat.S_ISDIR(ssh.lstat().st_mode) and not any(ssh.iterdir()),'fresh_boot_ssh_authority_present')
def retained(request):return request['provider']=='boat' and request['purpose']=='retained-resume'
def private_json(path,uid,limit=2097152):
 no_symlink(path);fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
 try:
  m=os.fstat(fd);require(stat.S_ISREG(m.st_mode) and m.st_uid==uid and m.st_nlink==1 and stat.S_IMODE(m.st_mode)==0o600 and m.st_size<=limit,'retained_boot_identity_file_invalid')
  raw=os.read(fd,limit+1);n=os.fstat(fd);require(len(raw)<=limit and (m.st_dev,m.st_ino,m.st_size,m.st_mtime_ns,m.st_ctime_ns)==(n.st_dev,n.st_ino,n.st_size,n.st_mtime_ns,n.st_ctime_ns),'retained_boot_identity_file_changed');return json.loads(raw)
 finally:os.close(fd)
def retained_identity(layout,request):
 # Parse authority only in RAM. Never return/hash/log bearer/private-key fields.
 private_directory(layout.data,layout.user)
 stored=private_json(layout.data/'cloud-runtime-identity.json',layout.user);record=private_json(layout.data/'cloud-runtime-supervision.json',layout.user);identity=record.get('identity',{});boot=stored.get('bootstrap',{})
 api=stored.get('apiUrl');require(isinstance(api,str) and 0<len(api)<=512,'retained_boot_api_invalid');url=urllib.parse.urlsplit(api)
 require(url.scheme=='https' and url.hostname and not url.username and not url.password and not url.query and not url.fragment and api==api.rstrip('/'),'retained_boot_api_invalid')
 expected=dict(version=1,workspaceId=request['workspaceId'],generation=request['generation'],storageIncarnationId=request['storageIncarnationId'],apiUrl=api)
 require(stored.get('version')==1 and stored.get('workspaceId')==request['workspaceId'] and stored.get('generation')==request['generation'] and stored.get('storageIncarnationId')==request['storageIncarnationId'] and stored.get('bootstrapAcknowledged') is True and record.get('version')==1 and identity==expected,'retained_boot_identity_mismatch')
 require(isinstance(boot,dict) and boot.get('workspaceId')==request['workspaceId'] and boot.get('runtimeGeneration')==request['generation'] and boot.get('runtimeStorage',{}).get('storageIncarnationId')==request['storageIncarnationId'] and stored.get('accountId')==boot.get('zuseAccountId') and isinstance(stored.get('accountId'),str) and bool(stored['accountId']),'retained_boot_enrollment_mismatch')
 require(boot.get('providerSandboxId')==request['sourceMachineId'],'retained_boot_source_mismatch')
 if boot.get('sourceSandboxId') is not None:require(boot['sourceSandboxId']==request['sourceMachineId'],'retained_boot_source_mismatch')
 receipts=record.get('receipts');require(isinstance(receipts,dict) and 1<=len(receipts)<=32 and all(SAFE.fullmatch(k) and isinstance(v,str) and re.fullmatch('[a-f0-9]{64}',v) for k,v in receipts.items()),'retained_boot_receipts_invalid')
 recovery=layout.data/'cloud-runtime-supervisor-recovery.json'
 if os.path.lexists(recovery):
  binding=private_json(recovery,layout.user,65536);require(all(binding.get(k)==expected[k] for k in ('workspaceId','generation','storageIncarnationId')),'retained_boot_recovery_mismatch')
 repairs=layout.data/'cloud-runtime-supervisor-repairs.json'
 if os.path.lexists(repairs):require(all(v.get('phase')=='ready' for v in private_json(repairs,layout.user,65536).values()),'retained_boot_repair_active')
 for name in ('cloud-runtime-supervisor-recreation.json','cloud-runtime-recreation.json'):
  if os.path.lexists(layout.data/name):raise Rejected('retained_boot_recreation_requires_inspection')
 return expected
def retained_stopped(layout,state):
 require(state.get('ControlGroup') in ('','/system.slice/'+UNIT),'retained_boot_cgroup_invalid')
 cg=Path('/sys/fs/cgroup/system.slice')/UNIT
 if cg.exists():
  require(all(not p.read_text().split() for p in cg.rglob('cgroup.procs')),'retained_boot_cgroup_not_empty')
 ipc=layout.data/'.runtime-supervision';private_directory(ipc,layout.user)
 require({p.name for p in ipc.iterdir()}<={'supervisor.json','supervisor.lock','control.sock'},'retained_boot_ipc_unknown')
 old=private_json(ipc/'supervisor.json',layout.user,4096);require(old.get('version')==1 and isinstance(old.get('bootNonce'),str) and re.fullmatch('[a-f0-9]{32}',old['bootNonce']),'retained_boot_ipc_record_invalid');pid=old.get('residentPid');require(type(pid) is int and pid>1 and not (Path('/proc')/str(pid)).exists(),'retained_boot_original_resident_present')
 path=ipc/'supervisor.lock';no_symlink(path);fd=os.open(path,os.O_RDWR|os.O_NOFOLLOW|os.O_CLOEXEC)
 try:
  m=os.fstat(fd);require(stat.S_ISREG(m.st_mode) and m.st_uid==layout.user and m.st_nlink==1 and stat.S_IMODE(m.st_mode)==0o600,'retained_boot_lock_invalid')
  try:fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
  except BlockingIOError:raise Rejected('retained_boot_lock_held') from None
  n=path.lstat();require((m.st_dev,m.st_ino)==(n.st_dev,n.st_ino),'retained_boot_lock_changed')
  address=ipc/'control.sock'
  if os.path.lexists(address):
   before=address.lstat();require(stat.S_ISSOCK(before.st_mode) and before.st_uid==layout.user and stat.S_IMODE(before.st_mode)==0o600,'retained_boot_socket_invalid')
   with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as probe:
    probe.settimeout(.25)
    try:probe.connect(str(address))
    except OSError as error:require(error.errno in (errno.ECONNREFUSED,errno.ENOENT),'retained_boot_socket_uncertain')
    else:raise Rejected('retained_boot_socket_live')
   after=address.lstat();require((before.st_dev,before.st_ino)==(after.st_dev,after.st_ino),'retained_boot_socket_changed')
 finally:os.close(fd)
def verify_image(layout,request):
 p=request['imagePins'];root={layout.owner}
 protected_bytes(layout.at('/usr/local/bin/zuse-cloud-runtime'),p['runtimeSha256'],root,limit=536870912,retain=False)
 supervisor=protected_bytes(layout.at('/usr/local/lib/zuse/runtime-supervisor.py'),p['supervisorSha256'],root,limit=131072)
 protected_bytes(layout.at('/etc/systemd/system/'+UNIT),p['serviceSha256'],root,limit=16384)
 protected_bytes(layout.at(HELPER),p['helperSha256'],root,limit=131072)
 if 'runtimeUpdateHelperSha256' in p:protected_bytes(layout.at(UPDATE_HELPER),p['runtimeUpdateHelperSha256'],root,limit=131072)
 manifest=json.loads(protected_bytes(layout.at('/opt/zuse/preparation/status/manifest.json'),p['manifestSha256'],{layout.owner,layout.user}))
 require(manifest.get('managedBase') is True and manifest.get('credentialsRemoved') is True and manifest.get('repositories')==[] and manifest.get('rustRuntimeSupervisorVersion')==1,'fresh_boot_managed_manifest_invalid')
 (retained_identity(layout,request) if retained(request) else authority_free(layout));return supervisor
def peer_ping(address,pid,uid):
 try:
  with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:
   client.settimeout(3);client.connect(str(address))
   peer=struct.unpack('3i',client.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12));require(peer[0]==pid and peer[1]==uid,'fresh_boot_ipc_peer_mismatch')
   client.settimeout(10);client.sendall(canonical({'version':1,'action':'ping'})+b'\n');raw=bytearray()
   while b'\n' not in raw:
    block=client.recv(min(4096,65537-len(raw)));require(block and len(raw)+len(block)<=65536,'fresh_boot_ipc_response_invalid');raw.extend(block)
  ping=json.loads(raw.split(b'\n',1)[0]);require('error' not in ping,'fresh_boot_ipc_rejected')
 except (OSError,ValueError):raise Pending('fresh_boot_ipc_not_ready') from None
 return ping
def owned_visible_lock(path,pid):
 no_symlink(path);before=path.lstat();identity=(before.st_dev,before.st_ino)
 found_fd=False
 for fd in (Path('/proc')/str(pid)/'fd').iterdir():
  try:m=fd.stat()
  except FileNotFoundError:continue
  if (m.st_dev,m.st_ino)==identity:found_fd=True
 require(found_fd,'fresh_boot_visible_lock_not_owned')
 found_lock=False
 for line in Path('/proc/locks').read_text().splitlines():
  p=line.split()
  if len(p)<8 or p[1]!='FLOCK' or p[2]!='ADVISORY' or p[3]!='WRITE' or p[4]!=str(pid):continue
  parts=p[5].split(':')
  if len(parts)==3 and (int(parts[0],16),int(parts[1],16),int(parts[2]))==(os.major(before.st_dev),os.minor(before.st_dev),before.st_ino) and p[6:] == ['0','EOF']:found_lock=True
 require(found_lock,'fresh_boot_visible_lock_not_held')
 after=path.lstat();require((after.st_dev,after.st_ino)==identity,'fresh_boot_visible_lock_changed')
def resident_ipc(layout):
 ipc=layout.data/'.runtime-supervision';private_directory(ipc,layout.user)
 expected={'supervisor.json','supervisor.lock','control.sock'};names={p.name for p in ipc.iterdir()};extra=names-expected
 # The original supervisor creates its directory/lock/socket before atomically
 # publishing the identity record. Incomplete known state is pending, never proof.
 require(not extra or len(extra)==1 and all(re.fullmatch(r'\.runtime-write-supervisor-[a-f0-9]{32}',name) for name in extra),'fresh_boot_ipc_state_unknown')
 for name in names:
  m=(ipc/name).lstat();kind=stat.S_ISSOCK(m.st_mode) if name=='control.sock' else stat.S_ISREG(m.st_mode) and m.st_nlink==1
  require(m.st_uid==layout.user and stat.S_IMODE(m.st_mode)==0o600 and kind,'fresh_boot_ipc_metadata_invalid')
 if names!=expected:raise Pending('fresh_boot_ipc_not_ready')
 return ipc
class Units:
 def state(self):
  result=subprocess.run(['systemctl','show',UNIT,'--property=MainPID,ActiveState,SubState,UnitFileState,Result,ExecMainStatus,Job,ControlGroup'],capture_output=True,text=True,check=True,timeout=3,env={'PATH':'/usr/bin:/bin'})
  require(len(result.stdout)<=8192,'fresh_boot_unit_response_invalid');return dict(line.split('=',1) for line in result.stdout.splitlines() if '=' in line)
 def disable(self):subprocess.run(['systemctl','disable',UNIT],capture_output=True,check=True,timeout=10,env={'PATH':'/usr/bin:/bin'})
 def start(self):subprocess.run(['systemctl','start',UNIT],capture_output=True,check=True,timeout=15,env={'PATH':'/usr/bin:/bin'})
 def resident(self,layout,supervisor,state,request=None):
  try:pid=int(state['MainPID'])
  except (KeyError,ValueError):raise Rejected('fresh_boot_resident_identity_invalid') from None
  require(pid>1,'fresh_boot_resident_identity_invalid');proc=Path('/proc')/str(pid);raw=(proc/'stat').read_text();parts=raw[raw.rfind(')')+2:].split();start=int(parts[19]);uids=[int(v) for line in (proc/'status').read_text().splitlines() if line.startswith('Uid:') for v in line.split()[1:]]
  is_retained=request is not None and retained(request)
  require(uids==[layout.user]*4 and (is_retained or not (proc/'task'/str(pid)/'children').read_text().split()),'fresh_boot_resident_not_owned_idle')
  require((proc/'cmdline').read_bytes().rstrip(b'\0').split(b'\0')==[b'/usr/bin/python3',b'/usr/local/lib/zuse/runtime-supervisor.py',b'serve'],'fresh_boot_resident_program_changed')
  group=state.get('ControlGroup');require(isinstance(group,str) and group.startswith('/') and '..' not in Path(group).parts and group.endswith('/'+UNIT),'fresh_boot_resident_cgroup_invalid')
  cg=Path('/sys/fs/cgroup')/group.lstrip('/');members=[int(v) for p in cg.rglob('cgroup.procs') for v in p.read_text().split()]
  if is_retained:
   for member in members:
    current=member;seen=set()
    while current!=pid:
     require(current>1 and current not in seen and len(seen)<512,'retained_boot_cgroup_not_owned');seen.add(current)
     try:r=(Path('/proc')/str(current)/'stat').read_text()
     except FileNotFoundError:break
     current=int(r[r.rfind(')')+2:].split()[1])
  require(pid in members and (is_retained or not any(p.is_dir() for p in cg.iterdir()) and sorted(members)==[pid]),'fresh_boot_resident_cgroup_not_idle')
  ipc=resident_ipc(layout)
  directories=[layout.data,ipc];directory_ids=[(p.stat().st_dev,p.stat().st_ino) for p in directories]
  snapshots={}
  for name in ('supervisor.json','supervisor.lock','control.sock'):
   m=(ipc/name).lstat();require(m.st_uid==layout.user and stat.S_IMODE(m.st_mode)==0o600 and (stat.S_ISSOCK(m.st_mode) if name=='control.sock' else stat.S_ISREG(m.st_mode) and m.st_nlink==1),'fresh_boot_ipc_metadata_invalid');snapshots[name]=(m.st_dev,m.st_ino,m.st_mtime_ns)
  owned_visible_lock(ipc/'supervisor.lock',pid)
  ping=peer_ping(ipc/'control.sock',pid,layout.user)
  require(ping.get('version')==1 and ping.get('residentPid')==pid and ping.get('bound') is is_retained and type(ping.get('childAlive')) is bool and (is_retained or ping['childAlive'] is False) and ping.get('recoveryAlive') is False and isinstance(ping.get('bootNonce'),str) and re.fullmatch('[a-f0-9]{32}',ping['bootNonce']),'fresh_boot_resident_bound_or_changed')
  record_fd=os.open(ipc/'supervisor.json',os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
  try:
   m=os.fstat(record_fd);require(m.st_size<=4096 and (m.st_dev,m.st_ino,m.st_mtime_ns)==snapshots['supervisor.json'],'fresh_boot_ipc_record_changed');record=json.loads(os.read(record_fd,4097))
  finally:os.close(record_fd)
  require(record.get('residentPid')==pid and record.get('bootNonce')==ping['bootNonce'],'fresh_boot_ipc_identity_mismatch')
  require(self.state()==state,'fresh_boot_unit_changed');owned_visible_lock(ipc/'supervisor.lock',pid)
  for name,identity in snapshots.items():
   m=(ipc/name).lstat();require((m.st_dev,m.st_ino,m.st_mtime_ns)==identity,'fresh_boot_ipc_path_changed')
  for path,identity in zip(directories,directory_ids):
   require(not path.is_symlink() and (path.stat().st_dev,path.stat().st_ino)==identity,'fresh_boot_ipc_directory_changed')
  again=(proc/'stat').read_text();require(int(again[again.rfind(')')+2:].split()[19])==start,'fresh_boot_resident_identity_changed')
  if is_retained:
   expected=retained_identity(layout,request);require(ping.get('binding')==expected,'retained_boot_live_binding_mismatch')
   if ping['childAlive']:
    child=ping.get('childPid');require(type(child) is int and child in members,'retained_boot_live_child_mismatch')
    raw=(Path('/proc')/str(child)/'stat').read_text();require(int(raw[raw.rfind(')')+2:].split()[1])==pid,'retained_boot_live_child_mismatch')
    child_proc=Path('/proc')/str(child);uids=[int(v) for line in (child_proc/'status').read_text().splitlines() if line.startswith('Uid:') for v in line.split()[1:]];require(uids==[layout.user]*4 and os.path.samefile(child_proc/'exe',layout.at('/usr/local/bin/zuse-cloud-runtime')),'retained_boot_live_child_program_mismatch')
  return dict(residentPid=pid,residentStartTicks=start,bootNonceSha256=digest(ping['bootNonce'].encode()),ipcVerified=True,unbound=not is_retained,childAlive=ping['childAlive'],recoveryAlive=False,**({'identityVerified':True} if is_retained else {}))
def atomic(path,value):
 tmp=path.with_name('.write-'+os.urandom(12).hex());fd=os.open(tmp,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,0o600)
 try:
  with os.fdopen(fd,'wb') as f:f.write(canonical(value)+b'\n');f.flush();os.fsync(f.fileno())
  os.replace(tmp,path);directory=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY|os.O_CLOEXEC)
  try:os.fsync(directory)
  finally:os.close(directory)
 finally:tmp.unlink(missing_ok=True)
def observe_resident(layout,supervisor,state,request,units):
 # systemd's active receipt can precede the resident's IPC socket. Observe the
 # same process briefly rather than forcing another provider exec and pin scan.
 # Every attempt retains the full resident proof; rejection is never retried.
 deadline=time.monotonic()+0.25
 while True:
  try:return units.resident(layout,supervisor,state,request)
  except (FileNotFoundError,ConnectionError):pass
  except Pending as error:
   if str(error)!='fresh_boot_ipc_not_ready':raise
  if time.monotonic()>=deadline:raise Pending('fresh_boot_starting')
  time.sleep(0.01)
  require(units.state()==state,'fresh_boot_unit_changed')
def bootstrap(request,layout=None,units=None):
 request=validated(request);layout=layout or Layout();units=units or Units();supervisor=verify_image(layout,request)
 no_symlink(layout.journal);layout.journal.mkdir(mode=0o700,exist_ok=True);private_directory(layout.journal,layout.owner)
 lock=os.open(layout.journal/'.lock',os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW|os.O_CLOEXEC,0o600)
 try:
  m=os.fstat(lock);require(stat.S_ISREG(m.st_mode) and m.st_uid==layout.owner and m.st_nlink==1 and stat.S_IMODE(m.st_mode)==0o600,'fresh_boot_journal_lock_invalid')
  try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
  except BlockingIOError:raise Pending('fresh_boot_original_busy') from None
  files=list(layout.journal.iterdir());require(len(files)<=(4096 if request['provider']=='boat' else 32),'fresh_boot_journal_limit')
  key=digest(canonical(request));path=layout.journal/(key+'.json');record=None
  if os.path.lexists(path):
   m=path.lstat();require(stat.S_ISREG(m.st_mode) and m.st_uid==layout.owner and m.st_nlink==1 and stat.S_IMODE(m.st_mode)==0o600 and m.st_size<=65536,'fresh_boot_journal_invalid');record=json.loads(path.read_bytes());require(record.get('request')==request and record.get('requestSha256')==key,'fresh_boot_journal_binding_changed')
  # A copied receipt is metadata only. Every replay/new-machine key must freshly
  # prove current image/authority, including when a prior receipt says ready.
  supervisor=verify_image(layout,request);state=units.state();expected_unit='disabled' if request['provider']=='boat' else 'enabled';require(state.get('UnitFileState')==expected_unit,'fresh_boot_enabled_unit_required')
  require(isinstance(state.get('Job'),str),'fresh_boot_unit_state_invalid')
  if state['Job'] or state.get('ActiveState') in ('activating','deactivating'):raise Pending('fresh_boot_unit_job_pending')
  if state.get('ActiveState')=='active' and state.get('SubState')=='running':
   resident=observe_resident(layout,supervisor,state,request,units)
   verify_image(layout,request)
   receipt={**request,'status':'ready','requestSha256':key,'unitStartSubmitted':bool(record and record.get('unitStartSubmitted')),'resident':resident}
   atomic(path,{'request':request,'requestSha256':key,'phase':'ready','unitStartSubmitted':receipt['unitStartSubmitted'],'receipt':receipt});return receipt
  if record is not None:raise Pending('fresh_boot_original_start_pending')
  require(state.get('ActiveState')=='inactive' and state.get('SubState')=='dead' and state.get('MainPID')=='0' and state.get('Result')=='success' and state.get('ExecMainStatus')=='0','fresh_boot_stopped_image_required')
  if retained(request):retained_stopped(layout,state)
  else:require(not any(layout.data.iterdir()),'fresh_boot_stale_ipc_not_admitted')
  record={'request':request,'requestSha256':key,'phase':'start-dispatched','unitStartSubmitted':True};atomic(path,record)
  # No restart/stop/unlink exists. An uncertain reply is inspected on a later
  # invocation of THIS request; the journal forbids issuing start again.
  try:units.start()
  except Exception:
   record['phase']='start-uncertain';atomic(path,record);raise Pending('fresh_boot_original_start_uncertain') from None
  state=units.state();require(state.get('UnitFileState')==expected_unit and isinstance(state.get('Job'),str),'fresh_boot_unit_changed_after_start')
  if state['Job']:raise Pending('fresh_boot_unit_job_pending')
  if state.get('ActiveState')!='active' or state.get('SubState')!='running':raise Pending('fresh_boot_starting')
  resident=observe_resident(layout,supervisor,state,request,units)
  verify_image(layout,request)
  receipt={**request,'status':'ready','requestSha256':key,'unitStartSubmitted':True,'resident':resident};record.update(phase='ready',receipt=receipt);atomic(path,record);return receipt
 finally:os.close(lock)
def _disable_capture_locked(request,stop_receipt,layout=None,units=None):
 request=validated(request);require(request['provider']=='boat' and request['purpose']=='publication-proof','capture_disable_scope_invalid');layout=layout or Layout();units=units or Units();verify_image(layout,request)
 require(stop_receipt.get('operationId')==request['operationId'] and stop_receipt.get('runtimeSha256')==request['imagePins']['runtimeSha256'] and stop_receipt.get('bootServiceSha256')==request['imagePins']['serviceSha256'] and stop_receipt.get('atomicStopAcknowledged') is True and stop_receipt.get('legacyBuilder') is False and all(stop_receipt.get(k) is True for k in ('originalResidentExited','supervisorLockReleased','runtimeDataAbsent','enabledBootServiceRetained')),'capture_disable_atomic_stop_required')
 old=stop_receipt.get('residentPid');require(type(old) is int and old>1 and type(stop_receipt.get('residentStartTicks')) is int,'capture_disable_original_identity_invalid')
 proc=Path('/proc')/str(old)/'stat'
 if proc.exists():
  raw=proc.read_text();require(int(raw[raw.rfind(')')+2:].split()[19])!=stop_receipt['residentStartTicks'],'capture_disable_original_still_present')
 def stopped(expected):
  state=units.state();require(state.get('MainPID')=='0' and state.get('ActiveState')=='inactive' and state.get('SubState')=='dead' and state.get('UnitFileState')==expected and state.get('Job')=='' and state.get('ControlGroup') in ('','/system.slice/'+UNIT),'capture_disable_unit_not_stopped');require(not any(layout.data.iterdir()),'capture_disable_data_not_empty')
  cg=Path('/sys/fs/cgroup/system.slice')/UNIT
  if cg.exists():require(all(not p.read_text().split() for p in cg.rglob('cgroup.procs')),'capture_disable_cgroup_not_empty')
  verify_image(layout,request);return state
 no_symlink(layout.journal);layout.journal.mkdir(mode=0o700,exist_ok=True);private_directory(layout.journal,layout.owner)
 key=digest(canonical(request));journal=layout.journal/('capture-'+key+'.json')
 if os.path.lexists(journal):
  saved=private_json(journal,layout.owner,65536);require(saved.get('requestSha256',saved.get('receipt',{}).get('requestSha256'))==key and saved.get('atomicStopReceiptSha256',saved.get('receipt',{}).get('atomicStopReceiptSha256'))==digest(canonical(stop_receipt)),'capture_disable_original_mismatch');state=units.state()
  if state.get('UnitFileState')!='disabled':raise Pending('capture_disable_original_pending')
  after=stopped('disabled');receipt=dict(version=1,operationId=request['operationId'],sourceMachineId=request['sourceMachineId'],requestSha256=key,runtimeSha256=request['imagePins']['runtimeSha256'],helperSha256=request['imagePins']['helperSha256'],serviceSha256=request['imagePins']['serviceSha256'],unitDisabled=True,atomicStopAcknowledged=True,noRuntimeData=True,atomicStopReceiptSha256=digest(canonical(stop_receipt)),unit=after);atomic(journal,dict(version=1,phase='disabled',receipt=receipt));return receipt
 stopped('enabled')
 atomic(journal,dict(version=1,phase='disable-dispatched',requestSha256=digest(canonical(request)),atomicStopReceiptSha256=digest(canonical(stop_receipt))))
 units.disable();after=stopped('disabled');receipt=dict(version=1,operationId=request['operationId'],sourceMachineId=request['sourceMachineId'],requestSha256=key,runtimeSha256=request['imagePins']['runtimeSha256'],helperSha256=request['imagePins']['helperSha256'],serviceSha256=request['imagePins']['serviceSha256'],unitDisabled=True,atomicStopAcknowledged=True,noRuntimeData=True,atomicStopReceiptSha256=digest(canonical(stop_receipt)),unit=after);atomic(journal,dict(version=1,phase='disabled',receipt=receipt));return receipt
def disable_capture(request,stop_receipt,layout=None,units=None):
 layout=layout or Layout();no_symlink(layout.journal);layout.journal.mkdir(mode=0o700,exist_ok=True);private_directory(layout.journal,layout.owner)
 fd=os.open(layout.journal/'.lock',os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW|os.O_CLOEXEC,0o600)
 try:
  meta=os.fstat(fd);require(stat.S_ISREG(meta.st_mode) and meta.st_uid==layout.owner and meta.st_nlink==1 and stat.S_IMODE(meta.st_mode)==0o600,'capture_disable_lock_invalid')
  try:fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
  except BlockingIOError:raise Pending('capture_disable_original_busy') from None
  return _disable_capture_locked(request,stop_receipt,layout,units)
 finally:os.close(fd)
def main():
 require(os.geteuid()==0,'fresh_boot_root_required');p=argparse.ArgumentParser();p.add_argument('--request-base64');p.add_argument('--capture-disable-base64');args=p.parse_args()
 if args.capture_disable_base64:
  raw=base64.b64decode(args.capture_disable_base64,validate=True);require(len(raw)<=24576,'capture_disable_request_too_large');value=json.loads(raw);require(isinstance(value,dict) and set(value)=={'request','stopReceipt'},'capture_disable_request_invalid');print(json.dumps(disable_capture(value['request'],value['stopReceipt']),sort_keys=True));return
 raw=base64.b64decode(args.request_base64,validate=True) if args.request_base64 else __import__('sys').stdin.buffer.read(24577);require(len(raw)<=24576,'fresh_boot_request_too_large');request=validated(json.loads(raw))
 try:result=bootstrap(request)
 except Pending as error:result={**request,'status':'starting','requestSha256':digest(canonical(request)),'code':str(error),'inspectionRequired':True}
 print(json.dumps(result,sort_keys=True))
if __name__=='__main__':
 try:main()
 except Exception as error:
  code=str(error) if isinstance(error,Rejected) else 'fresh_boot_inspection_failed'
  print(json.dumps({'version':1,'status':'rejected','code':code,'errorClass':type(error).__name__}));raise SystemExit(1)
