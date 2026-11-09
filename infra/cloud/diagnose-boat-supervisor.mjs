#!/usr/bin/env node
// Operator-only diagnosis: one original clean restore, no preparation or credentials.
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {dirname,resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {randomUUID,createHash} from 'node:crypto';
import {createBoatBuilderAdapter,requireFreshPublicationReport} from './build-managed-base.mjs';
import {githubCliProof,requireGithubCliProof} from './github-cli-proof.mjs';
const OLD_RUNTIME='27a922d084d65c500ab8afecb0eb42c144b73f20de78e41cef857074479c4756';
const quote=v=>`'${v.replaceAll("'","'\\''")}'`;
const pause=ms=>new Promise(r=>setTimeout(r,ms));
export function validatePrepared(value){
 if(value.provider!=='boat'||value.status!=='prepared-uncertified'||value.cleanupVerified!==true||value.artifactRetained!==true||value.runtimeSha256!==OLD_RUNTIME||typeof value.artifactId!=='string'||!/^zuse-base-build-[a-f0-9]{24}$/.test(value.artifactId)||value.manifest?.managedBase!==true||value.manifest?.credentialsRemoved!==true||value.manifest?.repositories?.length!==0||value.manifest?.rustRuntimeSupervisorVersion!==1||value.manifest?.rustGithubCliVersion!==1||value.githubCliProof?.status!=='passed')throw new Error('OriginalCleanBoat27Required');
 return value;
}
export async function diagnose({prepared,output,probeSource},adapter,{deadlineMs=120000,sleep=pause}={}){
 validatePrepared(prepared);await requireFreshPublicationReport(output);await mkdir(dirname(output),{recursive:true});
 const report={version:1,status:'incomplete',provider:'boat',scope:'existing-clean-supervisor-diagnosis',artifactId:prepared.artifactId,runtimeSha256:prepared.runtimeSha256,restoreName:'zuse-supervisor-diag-'+randomUUID().replaceAll('-','').slice(0,24),ttlSeconds:600,noEnv:true,credentialsDelivered:false,runtimeReplaced:false,stages:[],cleanupVerified:false,observations:{}};
 let claimed=false,id,submitted=false;
 const save=async()=>{await writeFile(output,JSON.stringify(report,null,2)+'\n',{mode:0o600,flag:claimed?'w':'wx'});claimed=true;};
 const stage=async(name,action)=>{report.activeStage=name;await save();const start=performance.now();try{const result=await action();report.stages.push({name,status:'completed',elapsedMs:Math.round((performance.now()-start)*100)/100});await save();return result;}catch(error){report.stages.push({name,status:'failed',elapsedMs:Math.round((performance.now()-start)*100)/100});throw error;}};
 const wait=async(action)=>{const end=Date.now()+deadlineMs;while(Date.now()<end){if(await action())return;await sleep(500);}throw new Error('OriginalObservationDeadline');};
 const owned=async()=>{const value=await adapter.identity(id);if(value.id!==id||value.name!==report.restoreName)throw new Error('OriginalRestoreIdentityMismatch');return value;};
 const metadata=async label=>{report.observations[label]=JSON.parse(await adapter.exec(id,'sudo -n /usr/bin/python3 -I -B -c '+quote(probeSource)));await save();};
 const ping=async()=>{const result=JSON.parse(await adapter.exec(id,'sudo -n -u zuse python3 /usr/local/lib/zuse/runtime-supervisor.py ping'));if(result.version!==1||result.bound!==false||result.childAlive!==false||result.recoveryAlive!==false||typeof result.bootNonce!=='string'||!Number.isSafeInteger(result.residentPid))throw new Error('IdleResidentProofInvalid');return{residentPid:result.residentPid,bootNonceSha256:createHash('sha256').update(result.bootNonce).digest('hex')};};
 const firstResident=async()=>{const end=Date.now()+deadlineMs;let attempts=0;report.firstPingObservations=[];
  while(Date.now()<end){attempts++;try{return await ping();}catch(error){if(error?.message==='IdleResidentProofInvalid')throw error;report.firstPingFailedObservations=attempts;if(report.firstPingObservations.length<64)report.firstPingObservations.push({attempt:attempts,code:['BuilderCommandFailed','BoatRequestFailed','IdleResidentProofInvalid'].includes(error?.message)?error.message:'ResidentNotReady'});await save();await sleep(500);}}
  throw new Error('OriginalObservationDeadline');
 };
 await save(); // Original name is durable before any provider mutation.
 try{
  await stage('verify-original-artifact',async()=>{const source=await adapter.snapshotState(prepared.artifactId);if(source.status!=='ready'||source.name!==prepared.artifactId||(prepared.providerArtifactVersion!=null&&(source.version??source.snapshotId??null)!==prepared.providerArtifactVersion))throw new Error('OriginalArtifactIdentityMismatch');});
  submitted=true;report.restoreSubmitted=true;await save();id=await stage('restore-original-artifact',()=>adapter.createFromCleanSnapshot(report.restoreName,prepared.artifactId,{ttlSeconds:600}));report.resourceId=id;await save();await owned();
  await stage('command-ready',()=>wait(()=>adapter.ready(id)));
  await stage('metadata-before-first-ping',()=>metadata('beforeFirstPing'));
  report.firstPing=await stage('first-supervisor-ping',firstResident);await save();
  await stage('metadata-after-first-ping',()=>metadata('afterFirstPing'));
  report.githubCliProof=await stage('actual-github-cli-proof',()=>githubCliProof(adapter,id,prepared.manifest));await save();
  await stage('metadata-after-github-proof',()=>metadata('afterGithubProof'));requireGithubCliProof(report.githubCliProof);
  await stage('separate-exec-true',()=>adapter.exec(id,'true'));
  await stage('metadata-after-exec-true',()=>metadata('afterExecTrue'));
  report.secondPing=await stage('second-supervisor-ping',ping);await save();
  await stage('metadata-after-second-ping',()=>metadata('afterSecondPing'));
  report.sameResident=report.firstPing.residentPid===report.secondPing.residentPid&&report.firstPing.bootNonceSha256===report.secondPing.bootNonceSha256;report.status='diagnosed';
 }catch(error){report.status='failed';report.errorStage=report.activeStage;report.errorCode=['OriginalObservationDeadline','OriginalRestoreIdentityMismatch','OriginalArtifactIdentityMismatch','IdleResidentProofInvalid','BuilderCommandFailed','BoatRequestFailed','ManagedGithubCliProofFailed','ManagedGithubCliProofInvalid'].includes(error?.message)?error.message:'DiagnosticIncomplete';await save();if(id){try{await metadata('failure');}catch{report.failureObservationUnavailable=true;}}}
 finally{
  await save(); // Preserve the exact failing boundary BEFORE cleanup.
  if(submitted&&!id){try{id=await adapter.find(report.restoreName);if(id){report.resourceId=id;await save();}}catch{report.restoreInspectionRequired=true;}}
  if(id){try{await owned();try{await adapter.remove(id);}catch{if(!await adapter.absent(id))throw new Error('OriginalDeleteUncertain');}await wait(()=>adapter.absent(id));report.cleanupVerified=true;}catch{report.cleanupInspectionRequired=true;}}
  else if(!submitted)report.cleanupVerified=true;else report.restoreInspectionRequired=true;
  await save();adapter.close();
 }
 return report;
}
async function main(){const args=process.argv.slice(2),get=k=>args[args.indexOf(k)+1];if(!args.includes('--prepared')||!args.includes('--output')||!args.includes('--probe'))throw new Error('InvalidArguments');const prepared=validatePrepared(JSON.parse(await readFile(resolve(get('--prepared')),'utf8')));const output=resolve(get('--output')),probeSource=await readFile(resolve(get('--probe')),'utf8');if(Buffer.byteLength(probeSource)>16384||!probeSource.includes('UNIX')||!probeSource.includes('def observe'))throw new Error('InvalidReadonlyProbe');if(!args.includes('--live')){console.log(JSON.stringify({status:'plan-only',artifactId:prepared.artifactId,runtimeSha256:prepared.runtimeSha256,maxMachines:1,ttlSeconds:600,noEnv:true,rebuild:false,runtimeReplacement:false,modelCalled:false,cleanup:'original-ID delete followed by independent absence',probeSha256:createHash('sha256').update(probeSource).digest('hex')}));return;}const adapter=await createBoatBuilderAdapter();const result=await diagnose({prepared,output,probeSource},adapter);console.log(JSON.stringify({status:result.status,errorStage:result.errorStage,cleanupVerified:result.cleanupVerified}));if(!result.cleanupVerified||result.status!=='diagnosed')process.exitCode=1;}
if(process.argv[1]&&pathToFileURL(resolve(process.argv[1])).href===import.meta.url)main().catch(()=>{console.error('Boat diagnosis incomplete; inspect the private original report.');process.exitCode=1;});
