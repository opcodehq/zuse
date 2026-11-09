import test from'node:test';
import assert from'node:assert/strict';
import{mkdtemp,writeFile,rm}from'node:fs/promises';
import{tmpdir}from'node:os';
import{join}from'node:path';
import{quiesceCleanParent}from'../clean-parent-quiescence.mjs';
import{quiesceCleanParent as receipt}from'./hygiene-fixture.mjs';
for(const lost of[false,true])test('one tracked original clean-parent command '+lost,async()=>{
 const root=await mkdtemp(join(tmpdir(),'parent-quiescence-'));let submitted=0,readback=0;const record={};const events=[];
 const parent={sha256:'a'.repeat(64),parentRuntimeSha256:'b'.repeat(64),artifactId:'parent-artifact'};
 await writeFile(join(root,'bundle.json'),JSON.stringify({runtimeSha256:'c'.repeat(64),harnessRelease:'d'.repeat(64)}));
 const request={operationId:'zuse-base-build-'+'e'.repeat(24),runtimeSha256:parent.parentRuntimeSha256,legacyBuilder:true,parentLineageSha256:parent.sha256,targetRuntimeSha256:'c'.repeat(64),parentArtifactId:parent.artifactId,harnessRelease:'d'.repeat(64)};
 const adapter={prepare:async(id,command,save)=>{submitted++;assert.equal(id,'original');assert.ok(command.includes('parent-quiescence-result.json.tmp'));await save({machineId:id,processId:77,mode:'boat-detached-command'});events.push('original-receipt-durable');if(lost)throw new Error('BuilderCommandUncertain');},exec:async(id,command)=>{readback++;assert.equal(id,'original');assert.ok(command.includes('cat --'));return JSON.stringify(await receipt(id,request));}};
 try{
  const promise=quiesceCleanParent({adapter,id:'original',rootShell:'sudo -n sh -c',operationId:request.operationId,parent,bundle:root,record,persist:async()=>events.push('persist')});
  if(lost){await assert.rejects(promise,/BuilderCommandUncertain/);assert.equal(record.inspectionRequired,true);assert.equal(readback,0);}
  else{const result=await promise;assert.equal(result.frozenCleanVerified,true);assert.equal(readback,1);}
  assert.equal(submitted,1);assert.equal(record.parentQuiescenceProcess.processId,77);assert.ok(events.includes('original-receipt-durable'));
 }finally{await rm(root,{recursive:true});}
});
test('already disabled clean parent uses a distinct receipt without retiring or starting a resident',async()=>{
 const root=await mkdtemp(join(tmpdir(),'disabled-parent-quiescence-'));
 const operationId='zuse-base-build-'+'e'.repeat(24),serviceSha256='f'.repeat(64);
 const parent={sha256:'a'.repeat(64),parentRuntimeSha256:'b'.repeat(64),artifactId:'clean-disabled-parent',docs:{parentBundle:{managedBaseFreshBootMode:'post-hydration-first-start-v1'},builder:{initialRuntimeBootstrap:{serviceSha256}}}};
 const manifest={runtimeSha256:'c'.repeat(64),harnessRelease:'d'.repeat(64)};await writeFile(join(root,'bundle.json'),JSON.stringify(manifest));
 const image={status:'quiescence-image-verified',parentMode:'post-hydration-first-start-v1',noResidentPresent:true,noAuthorityPresent:true,parentArtifactId:parent.artifactId,parentLineageSha256:parent.sha256,parentRuntimeSha256:parent.parentRuntimeSha256,harnessRelease:manifest.harnessRelease,runtimeSha256:manifest.runtimeSha256,completeCodexAssetsVerified:true,sourceClean:true};
 const good={version:1,operationId,runtimeSha256:parent.parentRuntimeSha256,parentMode:'post-hydration-first-start-v1',alreadyDisabledParent:true,noResidentPresent:true,noAuthorityPresent:true,startedOrStoppedProcess:false,legacyBuilder:false,atomicStopAcknowledged:false,enabledBootServiceRetained:false,disabledBootServiceRetained:true,bootServiceSha256:serviceSha256,parentImageProof:image,parentLineageSha256:parent.sha256};
 let calls=0;const record={},adapter={quiesceCleanParent:async(id,request)=>{calls++;assert.equal(id,'one-original');assert.equal(request.legacyBuilder,false);return structuredClone(good);},prepare:()=>assert.fail('No alternate operation'),exec:()=>assert.fail('No runtime start')};
 try{
  const result=await quiesceCleanParent({adapter,id:'one-original',rootShell:'sudo -n sh -c',operationId,parent,bundle:root,record,persist:async()=>{}});
  assert.equal(result.startedOrStoppedProcess,false);assert.equal(calls,1);assert.equal(record.parentSupervisorHygiene.alreadyDisabledParent,true);
  for(const change of[{startedOrStoppedProcess:true},{legacyBuilder:true},{alreadyDisabledParent:false},{noResidentPresent:false},{noAuthorityPresent:false},{parentMode:'unknown'},{bootServiceSha256:'0'.repeat(64)},{operationId:'different-operation'},{parentImageProof:{...image,noResidentPresent:false}}]){
   adapter.quiesceCleanParent=async()=>({...good,...change});
   await assert.rejects(quiesceCleanParent({adapter,id:'one-original',rootShell:'sudo -n sh -c',operationId,parent,bundle:root,record:{},persist:async()=>{}}),/CleanDisabledParentRejected/);
  }
 }finally{await rm(root,{recursive:true});}
});
