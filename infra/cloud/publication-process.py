#!/usr/bin/env python3
"""Linux one-shot publication process ownership, independent of provider state.

Only the launched session/process group is retired. No API reconciliation or
resource mutation is performed here. A pidfd holds the original leader identity
while its group is reaped; unrelated operator children are left alone.
"""
import ctypes,os,signal,subprocess,sys,time

def _subreaper():
 if sys.platform!='linux' or not hasattr(os,'pidfd_open'):
  raise RuntimeError('publication_linux_process_ownership_required')
 libc=ctypes.CDLL(None,use_errno=True)
 if libc.prctl(36,1,0,0,0)!=0:raise RuntimeError('publication_subreaper_required')
 test=os.pidfd_open(os.getpid());os.close(test)

def _facts(pid):
 try:
  with open('/proc/'+str(pid)+'/stat') as file:fields=file.read().rsplit(')',1)[1].split()
  return dict(parent=int(fields[1]),group=int(fields[2]),session=int(fields[3]),start=int(fields[19]))
 except (FileNotFoundError,ProcessLookupError):return None

def _members(group,started):
 facts={}
 for name in os.listdir('/proc'):
  if name.isdigit():
   value=_facts(int(name))
   if value:facts[int(name)]=value
 selected={pid:v for pid,v in facts.items() if v['group']==group}
 for pid,value in selected.items():
  if value['session']!=group or value['start']<started:raise RuntimeError('publication_owned_group_changed')
  parent=value['parent'];seen=set()
  while parent!=os.getpid():
   if parent in seen or parent not in facts:raise RuntimeError('publication_owned_group_changed')
   seen.add(parent);parent=facts[parent]['parent']
 return selected

def _signal(group,started,sig):
 # Signal stable pidfd identities, never a numeric group that could be reused.
 for pid,before in _members(group,started).items():
  try:fd=os.pidfd_open(pid)
  except ProcessLookupError:continue
  try:
   after=_facts(pid)
   if after is None:continue
   if any(after[k]!=before[k] for k in ('start','group','session')):raise RuntimeError('publication_owned_process_changed')
   signal.pidfd_send_signal(fd,sig)
  except ProcessLookupError:pass
  finally:os.close(fd)

def _reap(group,process):
 while True:
  try:pid,status=os.waitpid(-group,os.WNOHANG)
  except ChildProcessError:return
  if pid==0:return
  if pid==process.pid:process.returncode=os.waitstatus_to_exitcode(status)

def _retire(process,started,grace):
 group=process.pid
 _signal(group,started,signal.SIGTERM)
 end=time.monotonic()+grace
 while time.monotonic()<end:
  process.poll();_reap(group,process)
  if not _members(group,started):return
  time.sleep(.01)
 _signal(group,started,signal.SIGKILL)
 process.wait(timeout=grace)
 end=time.monotonic()+grace
 while time.monotonic()<end:
  _reap(group,process)
  if not _members(group,started):return
  time.sleep(.01)
 raise RuntimeError('publication_owned_group_retirement_unconfirmed')

def bounded(command,env,log,seconds,*,grace=5):
 _subreaper()
 with log.open('x') as output:
  os.fchmod(output.fileno(),0o600)
  process=subprocess.Popen(command,env=env,stdin=subprocess.DEVNULL,stdout=output,stderr=subprocess.STDOUT,start_new_session=True)
  identity=os.pidfd_open(process.pid)
  started=_facts(process.pid)['start']
  timed_out=False
  try:
   try:process.wait(timeout=seconds)
   except subprocess.TimeoutExpired:timed_out=True
   finally:_retire(process,started,grace)
   return dict(exitCode=process.returncode,timedOut=timed_out,groupRetired=True)
  finally:os.close(identity)

def memory_ceiling(directory,*,hierarchy_root=None,limit=512*1024*1024):
 """Require an enforced cgroup-v2 aggregate RAM limit and disabled swap."""
 from pathlib import Path
 directory=Path(directory).resolve();root=Path(hierarchy_root or '/sys/fs/cgroup').resolve()
 if directory!=root and root not in directory.parents:raise RuntimeError('publication_memory_scope_invalid')
 limits=[];swaps=[];group_kill=False
 for path in [directory,*directory.parents]:
  for name,target in [('memory.max',limits),('memory.swap.max',swaps)]:
   file=path/name
   if file.exists():
    value=file.read_text().strip()
    if value!='max':target.append(int(value))
  if (path/'memory.oom.group').exists() and (path/'memory.oom.group').read_text().strip()=='1':group_kill=True
  if path==root:break
 if not limits or min(limits)>limit or not swaps or min(swaps)!=0 or not group_kill:raise RuntimeError('publication_512m_scope_required')
 return dict(maxBytes=min(limits),swapMaxBytes=0,oomGroupKill=True)

def require_memory_ceiling():
 from pathlib import Path
 rows=Path('/proc/self/cgroup').read_text().splitlines();relative=next((r.split(':',2)[2] for r in rows if r.startswith('0::')),None)
 if relative is None:raise RuntimeError('publication_cgroup_v2_required')
 return memory_ceiling(Path('/sys/fs/cgroup')/relative.lstrip('/'))
