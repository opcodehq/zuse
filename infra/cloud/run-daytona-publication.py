#!/usr/bin/env python3
"""One-shot operator launcher; default plan is credential-free and has no effects."""
import argparse,hashlib,json,os,subprocess,time,uuid,importlib.util
from pathlib import Path

SOURCE_FILES=('publication-process.py','run-daytona-publication.py','build-daytona-managed-base.mjs','daytona-publication-context.mjs','base-supervisor-hygiene.mjs','clean-parent-quiescence.mjs','boat-hydration.mjs','managed-base-fresh-boot.mjs','daytona-snapshot-submit.mjs','build-managed-base.mjs','parent-preflight.mjs','clean-base-lineage.mjs','github-cli-proof.mjs','github-cli-proof.py','managed-base-preparation.py')
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
spec=importlib.util.spec_from_file_location('publication_process',Path(__file__).with_name('publication-process.py'))
process_owner=importlib.util.module_from_spec(spec);spec.loader.exec_module(process_owner)
bounded=process_owner.bounded
def main():
 p=argparse.ArgumentParser(description=__doc__)
 for name in ('node','node-sha256','publisher','bundle','base-image','output'):p.add_argument('--'+name,required=True)
 p.add_argument('--context-mode',choices=['verified-bundle-v1']);p.add_argument('--sdk',required=True);p.add_argument('--live',action='store_true');p.add_argument('--plan-sha256');p.add_argument('--retain-artifact',action='store_true');a=p.parse_args()
 os.umask(0o077);node=Path(a.node).resolve();publisher=Path(a.publisher).resolve();output=Path(a.output).resolve();output.parent.mkdir(parents=True,exist_ok=True)
 if digest(node)!=a.node_sha256:raise ValueError('daytona_launcher_node_pin_mismatch')
 command=[str(node),'--max-old-space-size=192',str(publisher),'--bundle',str(Path(a.bundle).resolve()),'--base-image',a.base_image,'--output',str(output)]
 if a.retain_artifact:command+=['--retain-artifact']
 if a.context_mode:command+=['--context-mode',a.context_mode]
 sources={name:digest(publisher.parent/name) for name in SOURCE_FILES}
 # The SDK pin is validated by the publisher before import; bind local dependency lock too.
 sdk=Path(a.sdk).resolve();sdk_pins={str(path):digest(path) for path in (sdk/'package.json',sdk/'esm/index.js',sdk.parents[2]/'package-lock.json')}
 env={'PATH':'/usr/bin:/bin'}
 validation=subprocess.run(command,env=env,capture_output=True,text=True,timeout=45)
 if validation.returncode!=0:raise ValueError('daytona_launcher_plan_validation_failed')
 provider_plan=json.loads(validation.stdout)
 plan=dict(version=1,command=command,sdk=str(sdk),sdkPins=sdk_pins,sourceFiles=sources,nodeSha256=a.node_sha256,providerPlan=provider_plan,outerDeadlineSeconds=1500,reconciliationDeadlineSeconds=90,maxBuildAttempts=1,maxProofSandboxes=1,persistentProof=True,automaticDeletionGuaranteed=False,aggregateMemoryMaxBytes=536870912,swapMaxBytes=0,processScopeRequired=True,certification=False,registration=False)
 plan_path=Path(str(output)+'.plan.json')
 if not a.live:
  save_once(plan_path,plan);print(json.dumps(dict(status='plan-only',planSha256=digest(plan_path),plan=plan,providerEffectsPerformed=False)));return
 if not a.plan_sha256 or digest(plan_path)!=a.plan_sha256 or json.loads(plan_path.read_text())!=plan:raise ValueError('daytona_launcher_plan_changed')
 if output.exists() or Path(str(output)+'.intent.json').exists():raise ValueError('daytona_launcher_already_attempted')
 if not os.environ.get('DAYTONA_API_KEY'):raise ValueError('daytona_launcher_key_missing')
 memory=process_owner.require_memory_ceiling()
 operation=str(uuid.uuid4());name='zuse-daytona-stage-'+operation.replace('-','')
 intent=dict(version=1,status='original-publication-started',operationId=operation,name=name,planSha256=a.plan_sha256,startedAtMs=time.time_ns()//1000000,memoryScope=memory)
 save_once(Path(str(output)+'.intent.json'),intent)
 provider_plan_path=Path(str(output)+'.provider-plan.json');save_once(provider_plan_path,provider_plan)
 env['DAYTONA_API_KEY']=os.environ['DAYTONA_API_KEY']
 outcome=bounded(command+['--live','--sdk',str(sdk),'--operation-id',operation,'--approved-plan',str(provider_plan_path)],env,Path(str(output)+'.private.log'),1500)
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
  print(json.dumps(dict(status='rejected',code='daytona_launcher_rejected')));raise SystemExit(1)
