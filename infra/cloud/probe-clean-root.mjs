#!/usr/bin/env node
// Tiny ownership-only diagnostic; never uploads a runtime, launches a guest,
// creates a snapshot, extends TTL, stops/wakes a machine or calls a model.
import {readFile,writeFile} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {rootPathFactsCommand,requireFreshPublicationReport,createBoatBuilderAdapter} from './build-managed-base.mjs';
const q=v=>`'${v.replaceAll("'", "'\\''")}'`;
export async function probeCleanRoot({output,parentArtifactId,parentProviderArtifactVersion,readyTimeoutMs=120000},adapter){
 await requireFreshPublicationReport(output);
 const parent=await adapter.snapshotState(parentArtifactId);
 if(parent.status!=='ready'||parent.name!==parentArtifactId||(parent.version??parent.snapshotId??null)!==parentProviderArtifactVersion)throw new Error('CleanParentArtifactMismatch');
 const record={schema:1,provider:'boat',status:'pending',name:'zuse-clean-root-diag-'+randomUUID().replaceAll('-','').slice(0,24),parentArtifactId,parentProviderArtifactVersion,ttlSeconds:600,cleanupVerified:false,metadataOnly:true};
 await writeFile(output,JSON.stringify(record,null,2)+'\n',{flag:'wx',mode:0o600});
 const save=()=>writeFile(output,JSON.stringify(record,null,2)+'\n',{mode:0o600});let submitted=false,deferred=false;
 try{
  submitted=true;record.machineId=await adapter.createFromCleanSnapshot(record.name,parentArtifactId,{ttlSeconds:600});await save();
  const deadline=Date.now()+readyTimeoutMs;let ready=false;
  while(Date.now()<deadline){if(await adapter.ready(record.machineId)){ready=true;break;}await new Promise(r=>setTimeout(r,200));}
  if(!ready){record.status='observation-deferred';deferred=true;return record;}
  const uid=(await adapter.exec(record.machineId,'id -u')).trim();if(!/^[0-9]{1,10}$/.test(uid))throw new Error('InvalidBuilderExecutorIdentity');record.executorUid=Number(uid);
  if(uid!=='0'&&(await adapter.exec(record.machineId,'sudo -n id -u')).trim()!=='0')throw new Error('BuilderRootPrivilegeRequired');
  const shell=uid==='0'?'sh -c':'sudo -n sh -c';
  const facts=async()=>JSON.parse(await adapter.exec(record.machineId,`${shell} ${q(rootPathFactsCommand())}`));
  record.before=await facts();await save();
  // Reproduce exactly the approved publisher's empty staging-directory
  // ownership sequence, without touching installed code or the clean parent.
  await adapter.exec(record.machineId,'test ! -e /tmp/zuse-base-input && mkdir -p /tmp/zuse-base-input');
  let sequenceError;
  try{await adapter.exec(record.machineId,`${shell} ${q('test ! -e /opt/zuse-base-input && mv -- /tmp/zuse-base-input /opt/zuse-base-input && chown -hR 0:0 -- /opt/zuse-base-input && chmod -R go-w -- /opt/zuse-base-input')}`);record.sequenceCompleted=true;}catch(error){sequenceError=error;record.sequenceCompleted=false;}
  record.after=await facts();
  record.guardFailureComponent=['/opt/zuse-base-input','/opt','/'].find(path=>{const f=record.after[path];return!f||f.error||f.symlink||f.uid!==0||(parseInt(f.mode.slice(2),8)&0o022)!==0;})??null;
  if(sequenceError)throw sequenceError;
  record.status='observed';
 }catch(error){record.status='failed';record.errorCode=['BuilderCommandFailed','BuilderRootPrivilegeRequired','InvalidBuilderExecutorIdentity'].includes(error.message)?error.message:'DiagnosticIncomplete';}
 finally{
  if(!deferred){try{
   let id=record.machineId;if(!id&&submitted)id=await adapter.find(record.name);
   if(!id)throw new Error('OriginalDiagnosticIdentityUnknown');
   let current;try{current=await adapter.identity(id);}catch(e){if(e.status!==404)throw e;}
   if(current){if(current.id!==id||current.name!==record.name)throw new Error('OriginalDiagnosticIdentityMismatch');try{await adapter.remove(id);}catch{if(!await adapter.absent(id))throw new Error('OriginalDeleteUncertain');}}
   record.machineId=id;record.cleanupVerified=await adapter.absent(id);if(!record.cleanupVerified)record.status='cleanup-required';
  }catch{record.status='cleanup-required';}}
  await save();adapter.close();
 }
 return record;
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){
 const a=process.argv.slice(2),get=k=>a[a.indexOf(k)+1];
 if(!a.includes('--live'))console.log(JSON.stringify({status:'plan-only',maxMachines:1,ttlSeconds:600,metadataOnly:true,snapshotsCreated:0,runtimeUploaded:false}));
 else if(!['--output','--parent','--parent-version'].every(k=>a.includes(k)))throw new Error('ExactDiagnosticInputsRequired');
 else probeCleanRoot({output:resolve(get('--output')),parentArtifactId:get('--parent'),parentProviderArtifactVersion:get('--parent-version')},await createBoatBuilderAdapter()).then(r=>{console.log(JSON.stringify({status:r.status,cleanupVerified:r.cleanupVerified,machineId:r.machineId}));if(r.status!=='observed'||!r.cleanupVerified)process.exitCode=1;}).catch(()=>{console.error('Clean root diagnostic failed; inspect its original private receipt.');process.exitCode=1;});
}
