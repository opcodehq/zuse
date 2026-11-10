#!/usr/bin/env python3
"""Print a pinned no-effects resource plan; only explicit approval starts a unit."""
import argparse,hashlib,json,os,shlex,stat,subprocess
from pathlib import Path
PROPERTIES={'MemoryMax':'536870912','MemorySwapMax':'0','CPUQuota':'50%','TasksMax':'128','OOMPolicy':'kill','KillMode':'control-group','RuntimeMaxSec':'1700','CPUWeight':'10'}
def sha(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  while data:=f.read(1024*1024):h.update(data)
 return h.hexdigest()
def canonical(value):return json.dumps(value,sort_keys=True,separators=(',',':')).encode()
def plan_for(path):
 saved=json.loads(path.read_text());command=saved['command'];publisher=Path(command[2]);source=publisher.parent
 for name,digest in saved['sourceFiles'].items():
  if sha(source/name)!=digest:raise ValueError('daytona_unit_source_changed')
 if sha(Path(command[0]))!=saved['nodeSha256']:raise ValueError('daytona_unit_node_changed')
 for name,digest in saved['sdkPins'].items():
  if sha(Path(name))!=digest:raise ValueError('daytona_unit_sdk_changed')
 def get(name):return command[command.index(name)+1]
 launch=['/usr/bin/python3',str(source/'run-daytona-publication.py'),'--node',command[0],'--node-sha256',saved['nodeSha256'],'--publisher',str(publisher),'--bundle',get('--bundle'),'--base-image',get('--base-image'),'--sdk',saved['sdk'],'--output',get('--output'),'--live','--plan-sha256',sha(path)]
 if '--retain-artifact' in command:launch+=['--retain-artifact']
 if '--context-mode' in command:
  if get('--context-mode')!='verified-bundle-v1':raise ValueError('daytona_unit_context_mode_invalid')
  launch+=['--context-mode','verified-bundle-v1']
 value=dict(version=1,launcherPlan=str(path),launcherPlanSha256=sha(path),wrapperSha256=sha(Path(__file__)),properties=PROPERTIES,command=launch,providerEffectsPerformed=False,privateKeySource='/etc/zuse/cloud/staging/worker.env',keyNames=['DAYTONA_API_KEY'],keyInArgv=False)
 digest=hashlib.sha256(canonical(value)).hexdigest();value['planSha256']=digest;value['unitName']='zuse-daytona-publish-'+digest[:16]
 return value
def check_cgroup(value,*,membership=Path('/proc/self/cgroup'),hierarchy=Path('/sys/fs/cgroup')):
 rows=membership.read_text().splitlines();relative=next((v.split('::',1)[1] for v in rows if v.startswith('0::')),None)
 if not relative or Path(relative).name!=value['unitName']+'.service':raise ValueError('daytona_unit_wrong_cgroup')
 root=hierarchy/relative.lstrip('/')
 if (root/'memory.oom.group').read_text().strip()!='1':raise ValueError('daytona_unit_oom_group_missing')
 if (root/'memory.max').read_text().strip()!='536870912' or (root/'memory.swap.max').read_text().strip()!='0' or (root/'pids.max').read_text().strip()!='128':raise ValueError('daytona_unit_limits_missing')
 quota,period=(root/'cpu.max').read_text().split()
 if quota=='max' or int(quota)*2>int(period):raise ValueError('daytona_unit_cpu_limit_missing')
def selected_key(path):
 fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
 with os.fdopen(fd) as file:
  info=os.fstat(file.fileno())
  if not stat.S_ISREG(info.st_mode) or info.st_uid!=0 or info.st_mode&0o077 or info.st_nlink!=1:raise ValueError('daytona_unit_key_source_unsafe')
  selected=[]
  for line in file:
   if not line.strip() or line.lstrip().startswith('#') or '=' not in line:continue
   key,raw=line.split('=',1)
   if key.strip()=='DAYTONA_API_KEY':
    tokens=shlex.split(raw.strip());selected.append(tokens[0] if len(tokens)==1 else '')
 if len(selected)!=1 or not selected[0] or any(c in selected[0] for c in '\r\n\0'):raise ValueError('daytona_unit_key_missing')
 return selected[0]
def inside_environment(value):
 check_cgroup(value)
 return {'PATH':'/usr/bin:/bin','DAYTONA_API_KEY':selected_key(Path(value['privateKeySource']))}
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--launcher-plan',type=Path,required=True);p.add_argument('--approve-plan-sha256');p.add_argument('--inside',action='store_true');a=p.parse_args();os.umask(0o077)
 value=plan_for(a.launcher_plan.resolve())
 if not a.approve_plan_sha256:
  print(json.dumps(value,sort_keys=True));return
 if a.approve_plan_sha256!=value['planSha256'] or os.geteuid()!=0:raise ValueError('daytona_unit_approval_changed')
 if a.inside:
  check_cgroup(value)
  env=inside_environment(value)
  os.execve(value['command'][0],value['command'],env)
 command=['/usr/bin/systemd-run','--quiet','--collect','--unit',value['unitName']]
 for key,val in PROPERTIES.items():command+=['--property',key+'='+val]
 command+=['/usr/bin/python3',str(Path(__file__).resolve()),'--launcher-plan',value['launcherPlan'],'--approve-plan-sha256',value['planSha256'],'--inside']
 subprocess.run(command,env={'PATH':'/usr/bin:/bin'},check=True,timeout=15)
 print(json.dumps(dict(version=1,status='original-unit-submitted',unitName=value['unitName'],planSha256=value['planSha256'])))
if __name__=='__main__':
 try:main()
 except Exception:
  print(json.dumps(dict(status='rejected',code='daytona_unit_rejected')));raise SystemExit(1)
