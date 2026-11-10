import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,writeFile,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
import {observeBoatStop,probeBoat,inspectOriginalBoat} from '../probe-boat-supervisor.mjs';

test('archived resource never completes a pending or failing original Boat stop',async()=>{
 let state='pending';let reads=0;
 const adapter={stopStatus:async(id,operation)=>{assert.equal(id,'original');assert.equal(operation,'stop-original');reads++;return{id:operation,status:state};},identity:async()=>({state:'archived'})};
 assert.equal(await observeBoatStop(adapter,'original',{id:'stop-original'}),false);
 state='failing';assert.equal(await observeBoatStop(adapter,'original',{id:'stop-original'}),false);
 state='completed';assert.equal(await observeBoatStop(adapter,'original',{id:'stop-original'}),true);
 state='superseded';await assert.rejects(observeBoatStop(adapter,'original',{id:'stop-original'}),/Superseded/);
 assert.equal(reads,4);
 adapter.stopStatus=async()=>({id:'another-stop',status:'completed'});
 await assert.rejects(observeBoatStop(adapter,'original',{id:'stop-original'}),/IdentityMismatch/);
});
async function fixture(){
 const root=await mkdtemp(join(tmpdir(),'zuse-boat-proof-'));
 const paths=['runtime','claude','codex','grok','harness-manifest.json','harness-release.py','launch-runtime.sh','launch-recovery.sh','managed-base-install.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py','prepare-plan.json'];
 const files=[];for(const path of paths){const value=Buffer.from('fixture '+path);await writeFile(join(root,path),value);files.push({path,bytes:value.length,sha256:createHash('sha256').update(value).digest('hex')});}
 await writeFile(join(root,'bundle.json'),JSON.stringify({schema:1,status:'candidate-only',releaseId:'zuse-base-'+'a'.repeat(32),harnessRelease:'b'.repeat(64),runtimeSha256:files[0].sha256,files}));return root;
}
for(const mode of ['success','resident-retained','lost-restore','lost-stop','dirty-lost','software-lost','superseded','invisible-restore','gh-missing'])test('Boat mechanical proof reconciles '+mode+' without reallocating or restopping',async()=>{
 const root=await fixture();let machine=false,snapshot=true,cold=false,marker='',creates=0,stops=0,deletes=0,snapshotDeletes=0;let name;
 const runId='zuse-base-build-'+'a'.repeat(24);
 const prepared={provider:'boat',runId,status:'prepared-uncertified',cleanupVerified:true,artifactRetained:true,artifactId:'saved-original',snapshotName:'saved-original',runtimeSha256:'c'.repeat(64),manifest:{rustRuntimeSupervisorVersion:1,rustRuntimeRepairVersion:1,rustRecreationVersion:1}};
 if(mode==='gh-missing')Object.assign(prepared.manifest,{rustGithubCliVersion:1,githubCli:{version:'gh version 2.45.0 (2025-01-01)',sha256:'a'.repeat(64)}});
 const adapter={restore:async(n,artifact)=>{creates++;name=n;assert.equal(artifact,'saved-original');machine=mode!=='invisible-restore';if(mode==='lost-restore'||mode==='invisible-restore')throw new Error('LostReply');return{id:'proof-original',name};},
  find:async n=>{assert.equal(n,name);return machine?'proof-original':undefined;},identity:async id=>{assert.equal(id,'proof-original');if(!machine)throw Object.assign(new Error('absent'),{status:404});return{id,name,hydrated:true,state:'archived',stop:{id:'stop-original',status:'pending'}};},ready:async()=>true,
  exec:async(_id,command)=>{if(mode==='gh-missing'&&command.includes('--receipt'))return JSON.stringify({version:1,required:true,status:'rejected',code:'managed_github_global_shim_invalid',globalShim:{lexists:false},accountRequired:false,modelCalled:false});if(command.includes(' ping'))return JSON.stringify({version:1,bound:false,childAlive:false,recoveryAlive:false,residentPid:100,bootNonce:cold&&mode!=='resident-retained'?'cold':'first',repairVersion:1,recreationVersion:1});if(command.includes('is-enabled'))return'enabled';if(command.includes('base64 -d'))return'';if(command.includes('zuse-boat-retention-canary'))return mode==='software-lost'?'changed':marker;if(command.includes('printf %s')){marker='retained-work-'+runId;return'';}if(command.includes('cat /home/zuse/'))return mode==='dirty-lost'?'changed':marker;return'';},
  stop:async()=>{stops++;if(mode==='lost-stop')throw new Error('LostStopReply');return{id:'stop-original',status:'pending'};},stopStatus:async()=>({id:'stop-original',status:mode==='superseded'?'superseded':'completed'}),resume:async()=>{cold=true;},
  remove:async()=>{deletes++;machine=false;},absent:async()=>!machine,removeSnapshot:async()=>{snapshotDeletes++;snapshot=false;},snapshotAbsent:async()=>!snapshot,close:()=>{}};
 try{
  const report=await probeBoat({bundle:root,output:join(root,'report.json'),retainCandidate:true,candidateOwner:'owner'},adapter,{buildBase:async()=>prepared,ghReadinessTimeoutMs:1});
  assert.equal(creates,1);assert.ok(stops<=1);assert.equal(machine,false);
  if(['success','resident-retained','lost-restore','lost-stop'].includes(mode)){assert.equal(report.status,'mechanism-verified-uncertified');assert.equal(report.cleanupVerified,true);assert.equal(report.artifactRetained,true);assert.equal(snapshot,true);assert.equal(deletes,1);assert.equal(snapshotDeletes,0);assert.equal(stops,1);assert.equal(report.snapshotBootOwnerStarts,true);assert.equal(report.originalStopCompleted,true);assert.equal(report.installedSoftwarePreserved,true);assert.equal(report.resumeMode,mode==='resident-retained'?'resident-retained':'resident-restarted');assert.equal(report.capturedBootOwnerRestarts,mode!=='resident-retained');}
  else{if(mode==='gh-missing'){assert.equal(report.errorCode,'ManagedGithubCliProofFailed');assert.equal(stops,0);}assert.equal(snapshot,false);assert.equal(report.artifactRetained,false);assert.equal(report.status,mode==='invisible-restore'?'cleanup-required':'failed');assert.equal(report.cleanupVerified,mode!=='invisible-restore');}
 }finally{await rm(root,{recursive:true});}
});

test('existing Boat proof or builder receipt cannot allocate a replacement',async()=>{
 for(const suffix of ['', '.builder.json']){
  const root=await fixture(),output=join(root,'existing.json'),original='{"status":"incomplete","restoreMachineId":"original"}';
  let builds=0;await writeFile(output+suffix,original);
  try{await assert.rejects(probeBoat({bundle:root,output},{},{buildBase:async()=>{builds++;}}),/ExistingPublicationOperation/);
   assert.equal(builds,0);assert.equal(await readFile(output+suffix,'utf8'),original);
  }finally{await rm(root,{recursive:true});}
 }
});
test('a deferred original builder cannot be replaced or implicitly deleted by outer proof',async()=>{
 const root=await fixture();let restores=0,deletes=0;
 const prepared={runId:'zuse-base-build-'+'d'.repeat(24),machineId:'original-builder',status:'observation-deferred',inspectionRequired:true,cleanupVerified:false};
 const adapter={restore:async()=>{restores++;assert.fail('replacement forbidden');},remove:async()=>{deletes++;assert.fail('original requires inspection');},close:()=>{}};
 try{
  const report=await probeBoat({bundle:root,output:join(root,'deferred-builder.json')},adapter,{buildBase:async()=>prepared});
  assert.equal(report.status,'observation-deferred');assert.equal(report.originalBuilderId,'original-builder');assert.equal(report.cleanupVerified,false);assert.equal(restores,0);assert.equal(deletes,0);
 }finally{await rm(root,{recursive:true});}
});
test('deferred builder observation and explicit cleanup retain exact identity and never delete parent',async()=>{
 const root=await fixture(),output=join(root,'builder-pending.json'),runId='zuse-base-build-'+'f'.repeat(24);let alive=true,deletes=0,reads=0;
 const outer={provider:'boat',inspectionRequired:true};const builder={provider:'boat',runId,machineId:'original-builder',inspectionRequired:true,parentQuiescenceProcess:{machineId:'original-builder',processId:77}};
 await writeFile(output,JSON.stringify(outer));await writeFile(output+'.builder.json',JSON.stringify(builder));
 const adapter={identity:async id=>{if(!alive)throw Object.assign(new Error('gone'),{status:404});return{id,name:runId,state:'ready',hydrated:true};},exec:async()=>JSON.stringify({schema:1,paths:{}}),commandState:async(id,pid)=>{reads++;assert.equal(id,'original-builder');assert.equal(pid,77);return{processId:77,running:true,status:'running'};},remove:async id=>{assert.equal(id,'original-builder');deletes++;alive=false;},absent:async()=>!alive,removeSnapshot:async()=>assert.fail('parent must never be deleted'),close:()=>{}};
 try{
  const observed=await inspectOriginalBoat(output,adapter);assert.equal(observed.originalCommand.processId,77);assert.equal(reads,1);assert.equal(deletes,0);
  const cleaned=await inspectOriginalBoat(output,adapter,{cleanup:true});assert.equal(cleaned.cleanupVerified,true);assert.equal(deletes,1);
  assert.deepEqual(JSON.parse(await readFile(output,'utf8')),outer);
 }finally{await rm(root,{recursive:true});}
});


test('observer timeout preserves original restore then read-only recheck and explicit cleanup never allocate again',async()=>{
 const root=await fixture(),output=join(root,'deferred.json'),runId='zuse-base-build-'+'b'.repeat(24);let cold=false,machine=true,snapshot=true,name,restores=0,stops=0,deletes=0;
 const prepared={provider:'boat',runId,machineId:'clean-builder',status:'prepared-uncertified',cleanupVerified:true,artifactRetained:true,artifactId:'snapshot-original',snapshotName:'snapshot-original',runtimeSha256:'c'.repeat(64),manifest:{rustRuntimeSupervisorVersion:1,rustRuntimeRepairVersion:1,rustRecreationVersion:1}};
 const adapter={restore:async n=>{restores++;name=n;return{id:'restore-original',name};},identity:async id=>{assert.equal(id,'restore-original');if(!machine)throw Object.assign(new Error('absent'),{status:404});return{id,name,hydrated:true,state:'archived'};},ready:async()=>true,
  exec:async(_id,command)=>{if(command.includes(' ping')){if(cold)throw new Error('BuilderCommandFailed');return JSON.stringify({version:1,bound:false,childAlive:false,recoveryAlive:false,residentPid:100,bootNonce:'original',repairVersion:1,recreationVersion:1});}if(command.includes('is-enabled'))return'enabled';if(command.endsWith('sudo -n python3'))return JSON.stringify({schema:1,paths:{socket:{error:'ENOENT'}},dirtyData:{matchesExpected:true},software:{matchesExpected:true}});return'';},
  stop:async()=>{stops++;return{id:'original-stop',status:'pending'};},stopStatus:async()=>({id:'original-stop',status:'completed'}),resume:async()=>{cold=true;},remove:async()=>{deletes++;machine=false;},absent:async id=>id==='clean-builder'||!machine,removeSnapshot:async()=>{snapshot=false;},snapshotAbsent:async()=>!snapshot,close:()=>{}};
 try{
  const result=await probeBoat({bundle:root,output,retainCandidate:true,candidateOwner:'owner'},adapter,{observationTimeoutMs:1,buildBase:async()=>{await writeFile(output+'.builder.json',JSON.stringify(prepared));return prepared;}});
  assert.equal(result.status,'observation-deferred');assert.equal(result.inspectionRequired,true);assert.equal(result.cleanupVerified,false);assert.equal(result.failureObservation.dirtyData.matchesExpected,true);assert.equal(machine,true);assert.equal(snapshot,true);assert.equal(restores,1);assert.equal(stops,1);assert.equal(deletes,0);
  const original=await readFile(output,'utf8');
  const observation=await inspectOriginalBoat(output,adapter);assert.equal(observation.readOnly,true);assert.equal(observation.observation.paths.socket.error,'ENOENT');assert.equal(restores,1);assert.equal(stops,1);assert.equal(deletes,0);assert.equal(await readFile(output,'utf8'),original);
  const cleanup=await inspectOriginalBoat(output,adapter,{cleanup:true});assert.equal(cleanup.cleanupVerified,true);assert.equal(machine,false);assert.equal(snapshot,false);assert.equal(restores,1);assert.equal(stops,1);assert.equal(deletes,1);assert.equal(await readFile(output,'utf8'),original);
 }finally{await rm(root,{recursive:true});}
});
for(const mode of ['success','first-lost','first-pending','cold-lost','cold-pending','first-malformed','ttl-budget'])test('new Boat clean start retains original on '+mode+' without unsafe replay',async()=>{
 const root=await fixture();const bundle=JSON.parse(await readFile(join(root,'bundle.json')));const body=Buffer.from('fixed helper fixture');await writeFile(join(root,'managed-base-fresh-boot.py'),body);bundle.files.push({path:'managed-base-fresh-boot.py',bytes:body.length,sha256:createHash('sha256').update(body).digest('hex')});Object.assign(bundle,{managedBaseFreshBootVersion:1,managedBaseFreshBootMode:'post-hydration-first-start-v1'});await writeFile(join(root,'bundle.json'),JSON.stringify(bundle));
 const runId='zuse-base-build-'+'c'.repeat(24),spec={version:1,mode:bundle.managedBaseFreshBootMode,...Object.fromEntries(['runtimeSha256','supervisorSha256','serviceSha256','manifestSha256','helperSha256'].map(k=>[k,'a'.repeat(64)]))};
 const prepared={provider:'boat',runId,status:'prepared-uncertified',cleanupVerified:true,artifactRetained:true,artifactId:'saved',snapshotName:'saved',runtimeSha256:'a'.repeat(64),initialRuntimeBootstrap:spec,manifest:{rustRuntimeSupervisorVersion:1,rustRuntimeRepairVersion:1,rustRecreationVersion:1}};
 let live=false,cold=false,starts=0,hydrated=false,now=Date.now(),stops=0,deletes=0,snapshotDeletes=0;const order=[];const name=runId+'-restore';
 const adapter={restore:async()=>{live=true;return{id:'original',name};},find:async()=>live?'original':undefined,identity:async()=>{if(!live)throw Object.assign(Error('absent'),{status:404});hydrated=true;order.push('hydrate');return{id:'original',name,hydrated:true,state:'archived'};},ready:async()=>true,
  exec:async(_id,command,budget)=>{
   assert.ok(!command.includes('supervisor-dirty-work')&&!command.includes('tee /usr/local/bin/zuse-boat-retention-canary'));
   if(command.includes('managed-base-fresh-boot.py')&&command.includes('sys.argv=')){assert.equal(hydrated,true);assert.deepEqual(budget,{commandTimeoutSeconds:60,fetchTimeoutMs:65000});const encoded=command.match(/'([^']+)'$/)[1],request=JSON.parse(Buffer.from(encoded,'base64'));starts++;order.push('start');
    const phase=cold?'cold':'first';if(mode===phase+'-lost')throw new TypeError('private lost response detail');if(mode===phase+'-malformed')return'{';
    if(mode===phase+'-pending'){now+=90000;return JSON.stringify({...request,status:'starting',requestSha256:createHash('sha256').update(Buffer.from(encoded,'base64')).digest('hex')});}
    return JSON.stringify({...request,status:'ready',requestSha256:createHash('sha256').update(Buffer.from(encoded,'base64')).digest('hex'),resident:{residentPid:123,residentStartTicks:456,bootNonceSha256:'d'.repeat(64),ipcVerified:true,unbound:true,childAlive:false,recoveryAlive:false}});}
   if(command.includes('is-enabled'))return'disabled';if(command.includes(' ping'))return JSON.stringify({version:1,residentPid:123,bootNonce:cold?'cold':'first',bound:false,childAlive:false,recoveryAlive:false,repairVersion:1,recreationVersion:1});return'';
  },cleanSupervisorForSnapshot:async(_id,r)=>{order.push('atomic-stop');assert.equal(r.disabledBootService,true);return{version:1,...r,originalResidentExited:true,supervisorLockReleased:true,runtimeDataAbsent:true,enabledBootServiceRetained:false,disabledBootServiceRetained:true,bootServiceSha256:'e'.repeat(64),atomicStopAcknowledged:true,removed:['supervisor.lock']};},
  stop:async()=>{stops++;return{id:'stop'};},stopStatus:async()=>({id:'stop',status:'completed'}),resume:async()=>{cold=true;hydrated=false;},remove:async()=>{deletes++;live=false;},absent:async()=>!live,removeSnapshot:async()=>{snapshotDeletes++;},snapshotAbsent:async()=>true,close:()=>{}};
 try{const output=join(root,'new-mode.json');const report=await probeBoat({bundle:root,output,retainCandidate:true,candidateOwner:'owner'},adapter,{freshBootClock:()=>mode==='ttl-budget'?Date.now()+1200000:now,buildBase:async()=>{await writeFile(output+'.builder.json',JSON.stringify(prepared));return prepared;}});
  if(mode==='success'){assert.equal(report.status,'mechanism-verified-uncertified');assert.equal(starts,2);assert.equal(report.unitRemainsDisabled,true);assert.equal(report.cleanColdRestartVerified,true);assert.equal(report.taskRetentionVerified,false);assert.equal(report.dirtyWorkPreserved,undefined);assert.equal(report.installedSoftwarePreserved,undefined);assert.ok(order.indexOf('hydrate')<order.indexOf('start'));assert.ok(order.indexOf('atomic-stop')<order.lastIndexOf('start'));assert.equal(live,false);}
  else{assert.equal(report.status,'observation-deferred');assert.equal(report.inspectionRequired,true);assert.equal(report.cleanupVerified,false);assert.equal(report.artifactRetained,true);assert.equal(report.deferredObservation.sourceMachineId,'original');assert.equal(report.deferredObservation.resubmissionAllowed,false);assert.equal(live,true);assert.equal(deletes,0);assert.equal(snapshotDeletes,0);assert.equal(starts,mode==='ttl-budget'?0:mode.startsWith('cold')?2:1);assert.equal(stops,mode.startsWith('cold')?1:0);assert.equal(report.errorCode,mode.includes('pending')||mode==='ttl-budget'?'FreshBootObservationPending':'FreshBootExecutionUncertain');
   const original=await readFile(output,'utf8');assert.equal(original.includes('private lost response detail'),false);const count=starts;const observed=await inspectOriginalBoat(output,adapter);assert.equal(observed.readOnly,true);assert.equal(starts,count);assert.equal(deletes,0);assert.equal(snapshotDeletes,0);assert.equal(await readFile(output,'utf8'),original);
  }
 }
 finally{await rm(root,{recursive:true});}
});
