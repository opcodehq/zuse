#!/usr/bin/env python3
"""One-shot operator launcher; default plan is credential-free and has no effects."""
import argparse,ctypes,hashlib,json,os,signal,subprocess,time,uuid
from pathlib import Path

SOURCE_FILES=('run-e2b-publication.py','build-e2b-managed-base.mjs','build-managed-base.mjs','github-cli-proof.mjs','github-cli-proof.py','managed-base-preparation.py','e2b-base-boot.py','e2b-streaming-hash.mjs','parent-preflight.mjs','clean-base-lineage.mjs')
def digest(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  while data:=f.read(1024*1024):h.update(data)
 return h.hexdigest()
def save_once(path,value):
 with path.open('x') as f:
  os.fchmod(f.fileno(),0o600);json.dump(value,f,sort_keys=True);f.write('\n');f.flush();os.fsync(f.fileno())
 fd=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY)
 try:os.fsync(fd)
 finally:os.close(fd)
def bounded(command,env,log,seconds):
 # Linux operator only. Adopt SDK grandchildren so a failed observer cannot keep
 # mutating after its parent exits. WNOWAIT retains the original leader PID until
 # group termination, so killpg cannot be redirected by leader PID reuse.
 if not hasattr(os,'WNOWAIT') or not hasattr(os,'pidfd_open') or ctypes.CDLL(None,use_errno=True).prctl(36,1,0,0,0)!=0:
  raise ValueError('e2b_launcher_process_ownership_unsupported')
 with log.open('x') as output:
  os.fchmod(output.fileno(),0o600)
  process=subprocess.Popen(command,env=env,stdin=subprocess.DEVNULL,stdout=output,stderr=subprocess.STDOUT,start_new_session=True)
  try:pidfd=os.pidfd_open(process.pid)
  except OSError:
   # The unreaped Popen child still reserves this group leader identity.
   try:os.killpg(process.pid,signal.SIGKILL)
   except ProcessLookupError:pass
   process.wait(timeout=5)
   until=time.monotonic()+3
   while True:
    try:child,_=os.waitpid(-process.pid,os.WNOHANG)
    except ChildProcessError:break
    if child:continue
    if time.monotonic()>=until:raise ValueError('e2b_launcher_observer_reap_uncertain')
    time.sleep(.01)
   raise
  timed_out=False
  try:
   deadline=time.monotonic()+seconds
   while os.waitid(os.P_PID,process.pid,os.WEXITED|os.WNOHANG|os.WNOWAIT) is None:
    if time.monotonic()>=deadline:timed_out=True;break
    time.sleep(min(.02,max(0,deadline-time.monotonic())))
   # Also retire observers on a normal/nonzero leader exit. No reconciliation
   # starts until this exact original group is killed and all adopted children
   # have been reaped. There is never a retry of the publisher here.
   try:os.killpg(process.pid,signal.SIGTERM)
   except ProcessLookupError:pass
   if timed_out:time.sleep(.1)
   try:os.killpg(process.pid,signal.SIGKILL)
   except ProcessLookupError:pass
   exit_code=process.wait(timeout=5)
   until=time.monotonic()+3
   while True:
    try:child,_=os.waitpid(-process.pid,os.WNOHANG)
    except ChildProcessError:break
    if child:continue
    if time.monotonic()>=until:raise ValueError('e2b_launcher_observer_reap_uncertain')
    time.sleep(.01)
   return dict(exitCode=exit_code,timedOut=timed_out)
  finally:os.close(pidfd)
def main():
 p=argparse.ArgumentParser(description=__doc__)
 for name in ('node','node-sha256','publisher','bundle','base-image','output','tmpdir'):p.add_argument('--'+name,required=True)
 p.add_argument('--sdk',required=True);p.add_argument('--live',action='store_true');p.add_argument('--plan-sha256');p.add_argument('--retain-artifact',action='store_true');a=p.parse_args()
 os.umask(0o077);node=Path(a.node).resolve();publisher=Path(a.publisher).resolve();output=Path(a.output).resolve();output.parent.mkdir(parents=True,exist_ok=True)
 if digest(node)!=a.node_sha256:raise ValueError('e2b_launcher_node_pin_mismatch')
 command=[str(node),'--max-old-space-size=256',str(publisher),'--bundle',str(Path(a.bundle).resolve()),'--base-image',a.base_image,'--output',str(output)]
 if a.retain_artifact:command+=['--retain-artifact']
 sources={name:digest(publisher.parent/name) for name in SOURCE_FILES}
 # The SDK pin is validated by the publisher before import; bind local dependency lock too.
 sdk=Path(a.sdk).resolve();sdk_root=sdk.parent.parent
 sdk_pins={str(path):digest(path) for path in (sdk,sdk_root/'package.json',sdk_root.parent.parent/'package-lock.json')}
 tmpdir=Path(a.tmpdir).resolve();info=tmpdir.lstat()
 if not tmpdir.is_dir() or tmpdir.is_symlink() or info.st_uid!=os.geteuid() or info.st_mode&0o077:raise ValueError('e2b_launcher_tmpdir_unsafe')
 env={'PATH':'/usr/bin:/bin','TMPDIR':str(tmpdir)}
 validation=subprocess.run(command,env=env,capture_output=True,text=True,timeout=45)
 if validation.returncode!=0:raise ValueError('e2b_launcher_plan_validation_failed')
 provider_plan=json.loads(validation.stdout)
 plan=dict(version=1,command=command,sdk=str(sdk),sdkPins=sdk_pins,sourceFiles=sources,nodeSha256=a.node_sha256,tmpdir=str(tmpdir),providerPlan=provider_plan,outerDeadlineSeconds=1200,reconciliationDeadlineSeconds=90,maxBuildAttempts=1,maxProofSandboxes=1,certification=False,registration=False)
 plan_path=Path(str(output)+'.plan.json')
 if not a.live:
  save_once(plan_path,plan);print(json.dumps(dict(status='plan-only',planSha256=digest(plan_path),plan=plan,providerEffectsPerformed=False)));return
 if not a.plan_sha256 or digest(plan_path)!=a.plan_sha256 or json.loads(plan_path.read_text())!=plan:raise ValueError('e2b_launcher_plan_changed')
 if output.exists() or Path(str(output)+'.intent.json').exists():raise ValueError('e2b_launcher_already_attempted')
 if not os.environ.get('E2B_API_KEY'):raise ValueError('e2b_launcher_key_missing')
 operation=str(uuid.uuid4());name='zuse-e2b-stage-'+operation.replace('-','')
 intent=dict(version=1,status='original-publication-started',operationId=operation,name=name,planSha256=a.plan_sha256,startedAtMs=time.time_ns()//1000000)
 save_once(Path(str(output)+'.intent.json'),intent)
 env['E2B_API_KEY']=os.environ['E2B_API_KEY']
 outcome=bounded(command+['--live','--sdk',str(sdk),'--operation-id',operation],env,Path(str(output)+'.private.log'),1200)
 receipt=json.loads(output.read_text()) if output.exists() else None
 if outcome['timedOut'] or outcome['exitCode']!=0 or not receipt or not receipt.get('cleanupVerified'):
  # Read-only reconciliation runs in a separate bounded process. It never creates,
  # retries, resumes or deletes a resource, even if a listing observes absence.
  inp=Path(str(output)+'.reconcile-input.json');save_once(inp,receipt or intent)
  reconcile=bounded([str(node),str(publisher),'--reconcile','--sdk',str(sdk),'--output',str(inp)],env,Path(str(output)+'.reconcile.json'),90)
  outcome['reconciliation']=reconcile
 outcome.update(version=1,operationId=operation,name=name,report=str(output),status=receipt.get('status') if receipt else 'outcome-unknown',cleanupVerified=bool(receipt and receipt.get('cleanupVerified') and not outcome['timedOut']))
 save_once(Path(str(output)+'.outcome.json'),outcome);print(json.dumps(outcome))
 if not outcome['cleanupVerified'] or outcome['status']!='prepared-uncertified':raise SystemExit(1)
if __name__=='__main__':
 try:main()
 except Exception:
  print(json.dumps(dict(status='rejected',code='e2b_launcher_rejected')));raise SystemExit(1)
