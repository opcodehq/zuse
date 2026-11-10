#!/usr/bin/env node
// A prepared base is not an agent certification. Live mode requires explicit intent.
import{githubCliProof,githubCliReadiness,requireGithubCliProof}from'./github-cli-proof.mjs';
import {createHash} from 'node:crypto';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {resolve,dirname} from 'node:path';
import {pathToFileURL} from 'node:url';
import {build,verifyBundle,createBoatBuilderAdapter,requireFreshPublicationReport} from './build-managed-base.mjs';
import {residentExtensions} from './probe-boxd-supervisor.mjs';
import {observeBoatOriginal} from './boat-observation.mjs';
import{waitBoatHydration}from'./boat-hydration.mjs';
import{proveFreshBoot}from'./managed-base-fresh-boot.mjs';
import{cleanBuilderSupervisor}from'./base-supervisor-hygiene.mjs';
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const proofErrors=new Set(['BuilderNotReady','InvalidOriginalBuilderIdentity','OriginalProofIdentityMismatch','IdleResidentProofInvalid','CapturedSupervisorServiceMissing','ResidentDidNotSurviveExec','DirtyWorkLost','InstalledSoftwareLost','OriginalStopUncertain','OriginalStopIdentityMismatch','OriginalStopSuperseded','OriginalStopInvalid','OriginalResumeUncertain','OriginalRestoreUncertain','ProofDeadline','BuilderCommandFailed','BoatRequestFailed','RequiredResidentExtensionMissing','ResidentExtensionMismatch','UnknownResidentExtension']);
for(const code of ['ManagedGithubCliReceiptInvalid','ManagedGithubCliProofInvalid','ManagedGithubCliProofFailed'])proofErrors.add(code);
for(const code of['OriginalHydrationPending','ProviderHydrationUnavailable','FreshBootObservationPending','FreshBootExecutionUncertain'])proofErrors.add(code);
const safeError=error=>proofErrors.has(error?.message)?error.message:'SupervisorProofIncomplete';
const residentFact=value=>({residentPid:value.residentPid,bootNonceSha256:createHash('sha256').update(value.bootNonce).digest('hex'),bound:value.bound,childAlive:value.childAlive,recoveryAlive:value.recoveryAlive});
export async function observeBoatStop(adapter,id,receipt) {
 const stop=await adapter.stopStatus(id,receipt.id);
 if(stop.id!==receipt.id)throw new Error('OriginalStopIdentityMismatch');
 if(stop.status==='superseded')throw new Error('OriginalStopSuperseded');
 if(stop.status==='completed')return (await adapter.identity(id)).state==='archived';
 if(!['pending','failing'].includes(stop.status))throw new Error('OriginalStopInvalid');
 return false;
}
async function wait(task,accept,timeout=120000){const deadline=Date.now()+timeout;while(Date.now()<deadline){const value=await task();if(accept(value))return value;await sleep(500);}throw new Error('ProofDeadline');}
export async function probeBoat({bundle,output,retainCandidate=false,candidateOwner,requireGithubCli=false,deriveLineage,deriveLineageSha256},adapter,{buildBase=build,observationTimeoutMs=120000,ghReadinessTimeoutMs=30000,freshBootClock=Date.now}={}) {
 await requireFreshPublicationReport(output);
 await requireFreshPublicationReport(output+'.builder.json');
 const verified=await verifyBundle(bundle);
 const deferredBoat=verified.managedBaseFreshBootMode==='post-hydration-first-start-v1';
 const requireExtensions=verified.files.some(file=>file.path==='runtime-repair.py');
 const report={schema:1,status:'incomplete',scope:'provider boot supervision only',provider:'boat',agentAcceptanceVerified:false,stages:[],cleanupVerified:false};
 await mkdir(dirname(output),{recursive:true});
 let reportClaimed=false;
 const save=async()=>{try{await writeFile(output,JSON.stringify(report,null,2)+'\n',{mode:0o600,flag:reportClaimed?'w':'wx'});reportClaimed=true;}catch(error){if(error.code==='EEXIST')throw new Error('ExistingPublicationOperation');throw error;}};
 const stage=async(name,operation)=>{report.activeStage=name;await save();const start=performance.now();const value=await operation();report.stages.push({name,elapsedMs:Math.round((performance.now()-start)*100)/100});await save();return value;};
 let prepared,restored,restoreSubmitted=false,buildStarted=false;
 const owned=async id=>{const machine=await adapter.identity(id);if(machine.id!==id||machine.name!==report.restoreName)throw new Error('OriginalProofIdentityMismatch');return machine;};
 const remove=async(id,name)=>{try{const machine=await adapter.identity(id);if(machine.id!==id||machine.name!==name)throw new Error('OriginalProofIdentityMismatch');}catch(error){if(error.status===404)return;throw error;}try{await adapter.remove(id);}catch{if(!await adapter.absent(id))throw new Error('OriginalDeleteUncertain');}await wait(()=>adapter.absent(id),Boolean,90000);};
 const ping=async()=>{
  const value=JSON.parse(await adapter.exec(restored.id,'sudo -n -u zuse python3 /usr/local/lib/zuse/runtime-supervisor.py ping'));
  if(value.version!==1||value.bound!==false||value.childAlive!==false||value.recoveryAlive!==false||typeof value.bootNonce!=='string'||!value.bootNonce||!Number.isSafeInteger(value.residentPid)||value.residentPid<1)throw new Error('IdleResidentProofInvalid');
  residentExtensions(prepared.manifest,value,requireExtensions);return value;
 };
 const resident=()=>wait(async()=>{try{return await ping();}catch(error){report.lastResidentError=safeError(error);report.residentFailedObservations=(report.residentFailedObservations??0)+1;return false;}},Boolean,observationTimeoutMs);
 await save();
 try{
  buildStarted=true;
  prepared=await stage('prepare-clean-builder',()=>buildBase({provider:'boat',bundle,requireGithubCli,deriveLineage,deriveLineageSha256,output:output+'.builder.json',retainArtifact:true},adapter));
  if(prepared.status!=='prepared-uncertified'||!prepared.cleanupVerified||!prepared.artifactRetained||prepared.manifest?.rustRuntimeSupervisorVersion!==1)throw new Error('BuilderNotReady');
  if(!/^zuse-base-build-[a-f0-9]{24}$/.test(prepared.runId))throw new Error('InvalidOriginalBuilderIdentity');
  report.runtimeSha256=prepared.runtimeSha256;report.artifactId=prepared.artifactId;report.snapshotName=prepared.snapshotName;
  report.sourceMachineAbsent=true;report.restoreName=prepared.runId+'-restore';report.restoreRequestedAt=Date.now();report.restoreTtlSeconds=1200;restoreSubmitted=true;await save();
  try{restored=await stage('restore-captured-boot-owner',()=>adapter.restore(report.restoreName,prepared.artifactId));}
  catch{
   const original=await adapter.find(report.restoreName);
   if(!original)throw new Error('OriginalRestoreUncertain');
   restored=await adapter.identity(original);
  }
  report.restoreMachineId=restored.id;await owned(restored.id);await save();
  await stage('restored-command-ready',()=>wait(()=>adapter.ready(restored.id),Boolean));
  report.restoreHydration=await stage('filesystem-hydrated-after-restore',()=>waitBoatHydration(adapter,restored.id,report.restoreName,{onObservation:async value=>{report.restoreHydration=value;await save();}}));
  const unitDisabled=async()=>{const state=await adapter.exec(restored.id,'state=$(sudo -n systemctl is-enabled zuse-runtime-supervisor.service); test "$state" = disabled && printf %s "$state"');if(state.trim()!=='disabled')throw new Error('CapturedSupervisorServiceMissing');};
  const freshStart=async suffix=>{
   report.initialRuntimeBootstrap=prepared.initialRuntimeBootstrap;
   const binding={purpose:'publication-proof',operationId:prepared.runId+suffix,workspaceId:'publication-'+prepared.runId,generation:1,storageIncarnationId:'publication-disk-'+prepared.runId,sourceMachineId:restored.id};
   const field=suffix?'coldInitialRuntimeBootstrapReceipt':'initialRuntimeBootstrapReceipt';
   // A helper may spend15s starting the unit and13s connecting/reading IPC,
   // plus bounded systemd state/SRI work. Its60s command has a65s fetch budget.
   // All observations share ONE90s window and the ORIGINAL restore TTL; no
   // deadline refresh, TTL extension, or uncertain-execution resubmission.
   const until=Math.min(freshBootClock()+90000,report.restoreRequestedAt+report.restoreTtlSeconds*1000-30000);
   report[field+'Budget']={commandTimeoutSeconds:60,fetchTimeoutMs:65000,observationDeadlineAt:until,originalRestoreExpiresAt:report.restoreRequestedAt+report.restoreTtlSeconds*1000};await save();
   const bounded={exec:async(id,command)=>{
    if(until-freshBootClock()<65000)throw new Error('FreshBootObservationPending');
    try{return await adapter.exec(id,command,{commandTimeoutSeconds:60,fetchTimeoutMs:65000});}
    catch(error){report[field+'ExecutionUncertain']={sourceMachineId:id,errorClass:/^[A-Za-z][A-Za-z0-9]{0,48}$/.test(error?.name??'')?error.name:'Error'};await save();throw new Error('FreshBootExecutionUncertain');}
   }};
   let receipt;
   try{receipt=await proveFreshBoot(bounded,restored.id,prepared.initialRuntimeBootstrap,binding,{deadlineMs:Math.max(0,until-freshBootClock()),onIntent:async v=>{report[field+'Intent']=v;await save();},onObservation:async v=>{report[field+'Observation']=v;await save();}});}
   catch(error){if(error instanceof SyntaxError)throw new Error('FreshBootExecutionUncertain');throw error;}
   report[field]=receipt;await unitDisabled();await save();
  };
  if(deferredBoat){await unitDisabled();await stage('post-hydration-protected-first-start',()=>freshStart(''));}
  else if((await adapter.exec(restored.id,'sudo -n systemctl is-enabled zuse-runtime-supervisor.service')).trim()!=='enabled')throw new Error('CapturedSupervisorServiceMissing');
  const first=await stage('resident-after-restore',resident);
  report.snapshotBootOwnerStarts=true;report.residentAfterRestore=residentFact(first);
  report.githubCliProof=await stage('managed-github-cli-after-restore',()=>githubCliReadiness(adapter,restored.id,prepared.manifest,{missingTimeoutMs:ghReadinessTimeoutMs,onObservation:async value=>{report.githubCliProof=value;await save();}}));await save();requireGithubCliProof(report.githubCliProof);
  await adapter.exec(restored.id,'true');const second=await ping();
  if(first.bootNonce!==second.bootNonce||first.residentPid!==second.residentPid)throw new Error('ResidentDidNotSurviveExec');
  report.residentSurvivesSeparateExec=true;report.installedExtensions=residentExtensions(prepared.manifest,first,requireExtensions);
  const marker='retained-work-'+prepared.runId;
  if(!deferredBoat)await adapter.exec(restored.id,`sudo -n -u zuse sh -c 'umask 077; printf %s ${marker} > /home/zuse/workspace/supervisor-dirty-work'`);
  const software=Buffer.from('#!/bin/sh\nprintf \"%s\" '+marker+'\n').toString('base64');
  if(!deferredBoat)await adapter.exec(restored.id,`printf %s ${software} | base64 -d | sudo -n tee /usr/local/bin/zuse-boat-retention-canary >/dev/null && sudo -n chmod 0555 /usr/local/bin/zuse-boat-retention-canary`);
  try{report.beforeStopObservation=await observeBoatOriginal(adapter,restored.id,prepared.runId);}catch(error){report.beforeStopObservationUnavailable=safeError(error);}
  if(deferredBoat){
   await stage('clean-disabled-owner-stop-before-cold-proof',()=>cleanBuilderSupervisor({adapter,id:restored.id,rootShell:'sudo -n sh -c',operationId:prepared.runId+':cold',runtimeSha256:prepared.runtimeSha256,disabledBootService:true,record:report,persist:save}));
   report.taskRetentionVerified=false;report.proofScope='clean first-start and cold restart mechanics only';
  }
  // Follow one original stop operation; an archived resource never proves a
  // pending/failed stop completed. Never force, re-stop, or wake it on timeout.
  await stage('stop-retained-disk',async()=>{
   let receipt;
   try{receipt=await adapter.stop(restored.id);}catch{receipt=(await owned(restored.id)).stop;}
   if(typeof receipt?.id!=='string')throw new Error('OriginalStopUncertain');
   report.stopReceipt={id:receipt.id,resourceId:restored.id};await save();
   await wait(()=>observeBoatStop(adapter,restored.id,receipt),Boolean,90000);
   report.originalStopCompleted=true;
  });
  await stage('resume-command-ready',async()=>{
   try{await adapter.resume(restored.id);}catch{
    if(!['resuming','provisioning','provisioned','cloning','ready','idle','running'].includes((await owned(restored.id)).state))throw new Error('OriginalResumeUncertain');
   }
   await wait(()=>adapter.ready(restored.id),Boolean);
  });
  report.resumeHydration=await stage('filesystem-hydrated-after-resume',()=>waitBoatHydration(adapter,restored.id,report.restoreName,{onObservation:async value=>{report.resumeHydration=value;await save();}}));
  if(deferredBoat)await stage('post-hydration-protected-cold-start',()=>freshStart(':cold'));
  const cold=await stage('resident-after-resume',resident);
  // A completed Boat stop can still resume the retained resident. Fresh
  // snapshot boot was proved above on a separate resource after builder deletion.
  // Record observed behavior; never manufacture guaranteed RAM or a cold reboot.
  report.residentAfterResume=residentFact(cold);
  report.resumeMode=cold.bootNonce===first.bootNonce?'resident-retained':'resident-restarted';
  report.capturedBootOwnerRestarts=cold.bootNonce!==first.bootNonce;report.coldBootExtensions=residentExtensions(prepared.manifest,cold,requireExtensions);
  if(!deferredBoat){
  const bytes=await stage('retained-work-readback',()=>adapter.exec(restored.id,'sudo -n -u zuse cat /home/zuse/workspace/supervisor-dirty-work'));
  if(bytes.trim()!==marker)throw new Error('DirtyWorkLost');
  report.dirtyWorkPreserved=true;
  const installed=await stage('installed-software-readback',()=>adapter.exec(restored.id,'sudo -n -u zuse /usr/local/bin/zuse-boat-retention-canary'));
  if(installed.trim()!==marker)throw new Error('InstalledSoftwareLost');
  report.installedSoftwarePreserved=true;
  }else{await unitDisabled();report.unitRemainsDisabled=true;report.cleanColdRestartVerified=true;}
  report.status='mechanism-verified-uncertified';
 }catch(error){report.status='failed';report.errorCode=safeError(error);report.errorStage=report.activeStage;if(Number.isInteger(error?.status))report.providerHttpStatus=error.status;}
 finally{
  let clean=true;
  if(buildStarted&&!prepared){
   try{const partial=JSON.parse(await readFile(output+'.builder.json','utf8'));if(partial.provider!=='boat'||!/^zuse-base-build-[a-f0-9]{24}$/.test(partial.runId))throw new Error('PartialBuilderInvalid');prepared=partial;}catch{clean=false;}
  }
  if(restoreSubmitted&&!restored){try{const id=await adapter.find(report.restoreName);if(id)restored=await adapter.identity(id);else clean=false;}catch{clean=false;}}
  if(prepared?.inspectionRequired){report.status='observation-deferred';report.inspectionRequired=true;report.cleanupVerified=false;report.originalBuilderId=prepared.machineId;report.cleanupScope='original-builder-under-original-ttl';await save();adapter.close();return report;}
  if(restored?.id){
   if(report.status==='failed'){
    try{report.failureObservation=await observeBoatOriginal(adapter,restored.id,prepared.runId);}catch(error){report.failureObservationUnavailable=safeError(error);}
   }
   // A bounded observer timeout is not proof that a service, disk or provider
   // effect failed. Preserve the SAME original diagnostic VM under its existing
   // TTL; re-inspect and explicitly clean it instead of allocating a replacement.
   if((report.errorCode==='ProofDeadline'&&['resident-after-restore','resident-after-resume'].includes(report.errorStage))||['OriginalHydrationPending','ProviderHydrationUnavailable','FreshBootObservationPending','FreshBootExecutionUncertain'].includes(report.errorCode)){
    report.status='observation-deferred';report.cleanupVerified=false;
    report.artifactRetained=Boolean(prepared?.snapshotName);report.inspectionRequired=true;
    report.cleanupScope='builder-absent-original-restore-under-original-ttl';
    if(['FreshBootObservationPending','FreshBootExecutionUncertain'].includes(report.errorCode))report.deferredObservation={kind:'fresh-boot-original-only',sourceMachineId:restored.id,originalRestoreExpiresAt:report.restoreRequestedAt+report.restoreTtlSeconds*1000,resubmissionAllowed:false};
    await save();adapter.close();return report;
   }
   if(report.status==='failed'){
    try{
     const text=await adapter.exec(restored.id,'sudo -n systemctl show zuse-runtime-supervisor.service --property=LoadState,ActiveState,SubState,Result,ExecMainCode,ExecMainStatus,NRestarts');
     report.failureServiceFacts=Object.fromEntries(text.split('\n').map(line=>line.split('=',2)).filter(([key,value])=>['LoadState','ActiveState','SubState','Result','ExecMainCode','ExecMainStatus','NRestarts'].includes(key)&&typeof value==='string'&&/^[A-Za-z0-9_.-]{0,96}$/.test(value)));
    }catch(error){report.failureServiceFactsUnavailable=safeError(error);}
   }
   try{await remove(restored.id,report.restoreName);}catch{clean=false;}
  }
  if(prepared&&!prepared.cleanupVerified){try{const id=prepared.machineId??await adapter.find(prepared.runId);if(!id)clean=false;else await remove(id,prepared.runId);}catch{clean=false;}}
  const retain=Boolean(prepared?.snapshotName)&&retainCandidate&&clean&&report.status==='mechanism-verified-uncertified';
  report.artifactRetained=Boolean(prepared?.snapshotName);
  if(retain){report.candidateOwner=candidateOwner;report.candidateArtifactStatus='uncertified-staging';report.cleanupScope='all-test-machines-absent-one-intended-staging-candidate-snapshot-retained';}
  else if(prepared?.snapshotName){try{try{await adapter.removeSnapshot(prepared.snapshotName);}catch{if(!await adapter.snapshotAbsent(prepared.snapshotName))throw new Error('OriginalSnapshotDeleteUncertain');}await wait(()=>adapter.snapshotAbsent(prepared.snapshotName),Boolean,90000);report.artifactRetained=false;}catch{clean=false;}}
  report.cleanupVerified=clean;if(!clean)report.status='cleanup-required';await save();adapter.close();
 }
 return report;
}
// Re-observation never creates, wakes, stops, extends TTL or replays a command.
export async function inspectOriginalBoat(output,adapter,{cleanup=false}={}) {
 const report=JSON.parse(await readFile(output,'utf8'));
 const builder=JSON.parse(await readFile(output+'.builder.json','utf8'));
 if(report.provider==='boat'&&builder.provider==='boat'&&report.inspectionRequired===true&&builder.inspectionRequired===true&&report.restoreRequestedAt===undefined){
  if(!/^zuse-base-build-[a-f0-9]{24}$/.test(builder.runId)||builder.snapshotName!==undefined||builder.artifactId!==undefined)throw new Error('OriginalProofIdentityMismatch');
  const result={schema:1,provider:'boat',readOnly:!cleanup,originalReportSha256:createHash('sha256').update(await readFile(output)).digest('hex'),builderName:builder.runId,observedAt:Date.now()};
  const id=builder.machineId??await adapter.find(builder.runId);
  if(!id){result.originalAllocationUnconfirmed=true;result.cleanupVerified=false;}
  else{
   let machine;try{machine=await adapter.identity(id);}catch(error){if(error.status!==404)throw error;result.builderAbsent=true;}
   if(machine){
    if(machine.id!==id||machine.name!==builder.runId)throw new Error('OriginalProofIdentityMismatch');
    result.builderId=id;result.providerState=machine.state;result.hydrated=machine.hydrated===true;
    if(!cleanup){
     try{result.observation=await observeBoatOriginal(adapter,id,builder.runId);}catch(error){result.observationUnavailable=safeError(error);}
     const process=builder.parentQuiescenceProcess;
     if(process&&adapter.commandState){if(process.machineId!==id)throw new Error('OriginalProofIdentityMismatch');result.originalCommand=await adapter.commandState(id,process.processId);}
    }else{try{await adapter.remove(id);}catch{if(!await adapter.absent(id))throw new Error('OriginalDeleteUncertain');}await wait(()=>adapter.absent(id),Boolean,90000);result.builderAbsent=true;}
   }
   result.cleanupVerified=Boolean(result.builderAbsent);
  }
  await writeFile(output+(cleanup?'.cleanup.json':'.observation.json'),JSON.stringify(result,null,2)+'\n',{mode:0o600});adapter.close();return result;
 }
 if(report.provider!=='boat'||builder.provider!=='boat'||!/^zuse-base-build-[a-f0-9]{24}$/.test(builder.runId)||report.restoreName!==builder.runId+'-restore'||typeof report.restoreMachineId!=='string'||report.snapshotName!==builder.snapshotName||report.artifactId!==builder.artifactId)throw new Error('OriginalProofIdentityMismatch');
 const result={schema:1,provider:'boat',originalReportSha256:createHash('sha256').update(await readFile(output)).digest('hex'),restoreMachineId:report.restoreMachineId,restoreName:report.restoreName,snapshotName:report.snapshotName,readOnly:!cleanup,observedAt:Date.now()};
 let machine;
 try{machine=await adapter.identity(report.restoreMachineId);}catch(error){if(error.status!==404)throw error;result.restoreAbsent=true;}
 if(machine){
  if(machine.id!==report.restoreMachineId||machine.name!==report.restoreName)throw new Error('OriginalProofIdentityMismatch');
  result.providerState=machine.state;result.hydrated=machine.hydrated===true;
  if(!cleanup){try{result.observation=await observeBoatOriginal(adapter,machine.id,builder.runId);}catch(error){result.observationUnavailable=safeError(error);}}
  else{try{await adapter.remove(machine.id);}catch{if(!await adapter.absent(machine.id))throw new Error('OriginalDeleteUncertain');}await wait(()=>adapter.absent(machine.id),Boolean,90000);result.restoreAbsent=true;}
 }
 if(cleanup){
  try{await adapter.removeSnapshot(report.snapshotName);}catch{if(!await adapter.snapshotAbsent(report.snapshotName))throw new Error('OriginalSnapshotDeleteUncertain');}
  await wait(()=>adapter.snapshotAbsent(report.snapshotName),Boolean,90000);result.snapshotAbsent=true;
  result.builderAbsent=await adapter.absent(builder.machineId);result.cleanupVerified=result.builderAbsent&&result.restoreAbsent&&result.snapshotAbsent;
 }
 await writeFile(output+(cleanup?'.cleanup.json':'.observation.json'),JSON.stringify(result,null,2)+'\n',{mode:0o600});adapter.close();return result;
}
async function main(){
 const argv=process.argv.slice(2),get=name=>argv[argv.indexOf(name)+1];
 if(argv.includes('--observe-existing')||argv.includes('--cleanup-existing')){const cleanup=argv.includes('--cleanup-existing'),path=resolve(get(cleanup?'--cleanup-existing':'--observe-existing'));const result=await inspectOriginalBoat(path,await createBoatBuilderAdapter(),{cleanup});console.log(JSON.stringify({readOnly:result.readOnly,cleanupVerified:result.cleanupVerified??false,restoreAbsent:result.restoreAbsent??false}));return;}
 if(argv.includes('--help')){console.log('probe-boat-supervisor.mjs --bundle DIR --output REPORT [--live --retain-staging-candidate --candidate-owner ACCOUNT] | --observe-existing REPORT | --cleanup-existing REPORT');return;}
 if(!['--bundle','--output'].every(key=>argv.includes(key)))throw new Error('ArgumentsRequired');
 const bundle=resolve(get('--bundle'));await verifyBundle(bundle);
 if(!argv.includes('--live')){console.log(JSON.stringify({status:'plan-only',provider:'boat',maxActiveMachines:1,maxTemporarySnapshots:1,preservesMemory:false,scope:'tracked stop, systemd restore, dirty disk; no agent acceptance'}));return;}
 const retainCandidate=argv.includes('--retain-staging-candidate'),candidateOwner=argv.includes('--candidate-owner')?get('--candidate-owner'):undefined;
 if(retainCandidate&&(process.env.ZUSE_DEPLOYMENT_ENVIRONMENT!=='staging'||!candidateOwner||!/^[A-Za-z0-9_.-]{1,160}$/.test(candidateOwner)))throw new Error('ExplicitStagingCandidateIntentRequired');
 const report=await probeBoat({bundle,output:resolve(get('--output')),retainCandidate,candidateOwner,requireGithubCli:argv.includes('--require-github-cli'),deriveLineage:argv.includes('--derive-lineage')?resolve(get('--derive-lineage')):undefined,deriveLineageSha256:argv.includes('--derive-lineage-sha256')?get('--derive-lineage-sha256'):undefined},await createBoatBuilderAdapter());
 console.log(JSON.stringify({status:report.status,cleanupVerified:report.cleanupVerified,artifactRetained:report.artifactRetained,agentAcceptanceVerified:false}));
 if(report.status!=='mechanism-verified-uncertified'||!report.cleanupVerified)process.exitCode=1;
}
if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href)main().catch(()=>{console.error('Boat supervisor proof failed; inspect private report.');process.exitCode=1;});
