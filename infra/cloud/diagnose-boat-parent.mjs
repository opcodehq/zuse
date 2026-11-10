#!/usr/bin/env node
// One clean-parent diagnostic. No runtime/native overlay, snapshot, auth or model.
import{readFile,open,mkdir,rename}from'node:fs/promises';
import{dirname,join,resolve}from'node:path';
import{pathToFileURL}from'node:url';
import{randomUUID,createHash}from'node:crypto';
import{createBoatBuilderAdapter,requireFreshPublicationReport}from'./build-managed-base.mjs';
import{loadCleanLineage}from'./clean-base-lineage.mjs';
import{uploadParentPreflight,inspectParentPreflight,repairableMetadata,VERIFIED_PROVIDER_ANCESTORS}from'./parent-preflight.mjs';
import{observeBoatOriginal}from'./boat-observation.mjs';
const sha=v=>createHash('sha256').update(v).digest('hex');
const q=v=>`'${v.replaceAll("'","'\\''")}'`;
const pause=ms=>new Promise(r=>setTimeout(r,ms));
const safe=e=>['BoatRequestFailed','BuilderCommandFailed','ParentPreflightReceiptInvalid','OriginalParentMismatch','OriginalResourceMismatch','UnexpectedParentMetadata','OriginalObservationDeadline','MetadataUploadUncertain'].includes(e?.message)?e.message:'ParentDiagnosticIncomplete';
export async function diagnosticInputs(bundle,lineage,lineageSha){
 const bytes=await readFile(join(bundle,'bundle.json'));const manifest=JSON.parse(bytes);
 const parent=await loadCleanLineage(lineage,lineageSha,manifest);
 if(parent.artifactId!=='zuse-base-build-2dd7fb58465b4c7999313fd8'||parent.providerArtifactVersion!=='c72c4d51-353f-4315-90d4-2b5dc69ded0c')throw new Error('OriginalParentMismatch');
 let total=parent.bytes.length+bytes.length;
 for(const name of['harness-release.py','harness-manifest.json']){
  const raw=await readFile(join(bundle,name)),pin=manifest.files.find(f=>f.path===name);
  if(!pin||sha(raw)!==pin.sha256||raw.length!==pin.bytes)throw new Error('OriginalParentMismatch');total+=raw.length;
 }
 if(total>256*1024)throw new Error('MetadataUploadUncertain');
 return{parent,manifest,payloadBytes:total};
}
export async function diagnoseParent({bundle,output,parent,manifest,payloadBytes,observeExisting=false},adapter,{sleep=pause,maxObservations=3,clock=Date.now}={}){
 let report;
 if(observeExisting){report=JSON.parse(await readFile(output,'utf8'));if(report.scope!=='clean-parent-preflight-only'||report.parentArtifactId!==parent.artifactId||report.parentVersion!==parent.providerArtifactVersion||report.lineageSha256!==parent.sha256)throw new Error('OriginalParentMismatch');}
 else{
  await requireFreshPublicationReport(output);await mkdir(dirname(output),{recursive:true});
  report={version:1,scope:'clean-parent-preflight-only',status:'intent',runId:'zuse-base-build-'+randomUUID().replaceAll('-','').slice(0,24),parentArtifactId:parent.artifactId,parentVersion:parent.providerArtifactVersion,lineageSha256:parent.sha256,targetRuntimeSha256:manifest.runtimeSha256,createdAtMs:clock(),ttlSeconds:600,providerNoEnv:true,stages:[],observations:[],preflightAttempts:[],uploadReceipts:[],uploadedBytes:0,cleanupVerified:false,overlayInstalled:false,snapshotSubmitted:false,credentialsDelivered:false};
 }
 let claimed=observeExisting,rootShell=report.rootShell;
 const save=async()=>{
  const path=claimed?output+'.next':output;
  const f=await open(path,'wx',0o600);try{await f.writeFile(JSON.stringify(report,null,2)+'\n');await f.sync();}finally{await f.close();}
  if(claimed)await rename(path,output);claimed=true;
  const dir=await open(dirname(output),'r');try{await dir.sync();}finally{await dir.close();}
 };
 const stage=async(name,task)=>{report.activeStage=name;await save();const start=performance.now();try{const value=await task();report.stages.push({name,status:'completed',elapsedMs:Math.round((performance.now()-start)*100)/100});await save();return value;}catch(error){report.stages.push({name,status:'failed',elapsedMs:Math.round((performance.now()-start)*100)/100,code:safe(error)});await save();throw error;}};
 const owned=async()=>{const v=await adapter.identity(report.resourceId);if(v.id!==report.resourceId||v.name!==report.runId)throw new Error('OriginalResourceMismatch');return v;};
 const facts=async label=>{
  await owned();const value=await observeBoatOriginal(adapter,report.resourceId,report.runId);
  const source=await readFile(new URL('./boat-socket-diagnostic.py',import.meta.url));report.socketDiagnosticSha256=sha(source);await save();
  const kernel=JSON.parse(await adapter.exec(report.resourceId,`${rootShell} ${q('/usr/bin/python3 -I -B -c '+q(source.toString('utf8')))}`));
  report.observations.push({label,value,kernel});await save();return value;
 };
 const classify=async()=>{
  for(let n=0;n<maxObservations;n++){
   if(clock()>=report.createdAtMs+600000-30000)throw new Error('OriginalObservationDeadline');
   await owned();
   const result=await stage('strict-parent-observation',()=>inspectParentPreflight({adapter,id:report.resourceId,parent,rootShell,bundle,record:report,persist:save}));
   report.preflightAttempts.push(result);await save();await facts('after-preflight-'+report.preflightAttempts.length);
   if(result.status==='passed'){report.status='diagnosed';return;}
   if(result.code==='parent_preflight_failed'&&!result.inspection){report.status='deferred-observation';return;}
   if(result.inspection?.errorClass!=='TimeoutExpired'){report.status='classified-rejection';return;}
   // Repeat only this read-only inspection of the exact original resource.
   // Do not rerun uploads, chown, create, install, stop or TTL operations.
   report.retainedForObservation=true;await save();await sleep(1000);
  }
  report.status='deferred-observation';
 };
 await save();
 try{
  if(!observeExisting){
   await stage('verify-original-parent',async()=>{const v=await adapter.snapshotState(parent.artifactId);if(v.name!==parent.artifactId||v.status!=='ready'||(v.version??v.snapshotId??null)!==parent.providerArtifactVersion)throw new Error('OriginalParentMismatch');});
   report.allocationSubmitted=true;await save();
   try{report.resourceId=await stage('restore-original-parent',()=>adapter.createFromCleanSnapshot(report.runId,parent.artifactId,{ttlSeconds:600}));await save();}
   catch{report.resourceId=await adapter.find(report.runId);await save();if(!report.resourceId){report.status='deferred-observation';return report;}}
  }else if(!report.resourceId){report.resourceId=await adapter.find(report.runId);await save();if(!report.resourceId){report.status='deferred-observation';return report;}}
  await owned();
  const until=Math.min(clock()+60000,report.createdAtMs+570000);
  while(!await adapter.ready(report.resourceId)){if(clock()>=until)throw new Error('OriginalObservationDeadline');await sleep(500);}
  const uid=(await adapter.exec(report.resourceId,'id -u')).trim();if(!/^\d+$/.test(uid))throw new Error('OriginalResourceMismatch');
  rootShell=uid==='0'?'sh -c':'sudo -n sh -c';report.rootShell=rootShell;await save();
  await stage('capture-original-resident-and-ipc',()=>facts(observeExisting?'reobserved-original':'before-preflight'));
  if(!observeExisting){
   // Exactly one metadata upload attempt. A lost reply is observed, never PUT again.
   await stage('upload-fixed-metadata',()=>uploadParentPreflight({adapter,id:report.resourceId,runId:report.runId,bundle,parent,rootShell,record:report,persist:save,uploadExactChunk:async(a,id,_name,path,bytes)=>{const n=await a.upload(id,path,bytes);if(n!==bytes.length)throw new Error('MetadataUploadUncertain');return{bytes:n,sha256:sha(bytes)};}}));
   if(report.uploadedBytes!==payloadBytes)throw new Error('MetadataUploadUncertain');
   report.metadataUploaded=true;await save();
   const first=await stage('initial-strict-parent-preflight',()=>inspectParentPreflight({adapter,id:report.resourceId,parent,rootShell,bundle,record:report,persist:save}));report.initialPreflight=first;await save();
   if(first.status==='rejected'&&first.code==='parent_metadata_not_protected'){
    if(!repairableMetadata(first))throw new Error('UnexpectedParentMetadata');
    const source=await readFile(new URL('./managed-base-ownership.py',import.meta.url));report.ancestorSourceSha256=sha(source);await save();
    report.ancestorHardening=await stage('approved-clone-directory-metadata',async()=>{
     const v=JSON.parse(await adapter.exec(report.resourceId,`${rootShell} ${q('/usr/bin/python3 -I -B -c '+q(source.toString('utf8')))}`));
     if(v.version!==2||v.recursive!==false||v.paths?.length!==4||v.paths.some((p,i)=>p.path!==VERIFIED_PROVIDER_ANCESTORS[i]||p.recursive!==false||p.after?.uid!==0||p.after?.gid!==0||p.after?.mode!=='0o755'))throw new Error('UnexpectedParentMetadata');return v;
    });await save();
   }else if(first.status==='rejected'&&first.inspection?.errorClass!=='TimeoutExpired'){report.preflightAttempts.push(first);report.status=first.code==='parent_preflight_failed'&&!first.inspection?'deferred-observation':'classified-rejection';return report;}
  }
  if(!report.metadataUploaded){report.status='deferred-observation';report.preflightUnavailableReason='original-metadata-upload-unconfirmed';return report;}
  await classify();
 }catch(error){report.status='deferred-observation';report.errorStage=report.activeStage;report.errorCode=safe(error);if(report.resourceId)try{await facts('failure');}catch{report.failureFactsUnavailable=true;}}
 finally{
  await save();
  // Keep a timed-out/unobserved original for its remaining600s TTL. No replacement.
  if(['diagnosed','classified-rejection'].includes(report.status)&&report.resourceId){
   try{await owned();await stage('capture-before-delete',()=>facts('before-delete'));try{await adapter.remove(report.resourceId);}catch{if(!await adapter.absent(report.resourceId))throw new Error('OriginalObservationDeadline');}report.cleanupVerified=await adapter.absent(report.resourceId);}catch{report.cleanupInspectionRequired=true;}
  }else if(!report.allocationSubmitted)report.cleanupVerified=true;
  await save();adapter.close();
 }
 return report;
}
async function main(){
 const a=process.argv.slice(2),get=k=>a[a.indexOf(k)+1];
 if(!['--bundle','--lineage','--lineage-sha256','--output'].every(k=>a.includes(k)))throw new Error('ArgumentsRequired');
 const bundle=resolve(get('--bundle')),output=resolve(get('--output'));const inputs=await diagnosticInputs(bundle,resolve(get('--lineage')),get('--lineage-sha256'));
 if(!a.includes('--live')){console.log(JSON.stringify({status:'plan-only',parentArtifactId:inputs.parent.artifactId,parentVersion:inputs.parent.providerArtifactVersion,payloadBytes:inputs.payloadBytes,ttlSeconds:600,maxMachines:1,overlayInstalled:false,snapshotSubmitted:false,providerEffects:false}));return;}
 const report=await diagnoseParent({bundle,output,...inputs,observeExisting:a.includes('--observe-existing')},await createBoatBuilderAdapter());
 console.log(JSON.stringify({status:report.status,resourceId:report.resourceId,runId:report.runId,cleanupVerified:report.cleanupVerified}));
 if(!report.cleanupVerified||!['diagnosed','classified-rejection'].includes(report.status))process.exitCode=2;
}
if(process.argv[1]&&pathToFileURL(resolve(process.argv[1])).href===import.meta.url)main().catch(()=>{console.error('Original parent diagnostic requires private report inspection.');process.exitCode=1;});
