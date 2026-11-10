import test from'node:test';import assert from'node:assert/strict';import{mkdtemp,rm,readFile}from'node:fs/promises';import{join}from'node:path';import{tmpdir}from'node:os';import{diagnoseQuiescence}from'../diagnose-boat-quiescence.mjs';
for(const mode of['success','known-rejection','uncertain'])test('metadata-only diagnostic fixes operation scope '+mode,async()=>{
 const root=await mkdtemp(join(tmpdir(),'parent-stop-diag-'));let calls=0;
 try{
  const args={bundle:'fixed-bundle',deriveLineage:'fixed-lineage',deriveLineageSha256:'a'.repeat(64),output:join(root,'report.json'),retainArtifact:true,parentPreflightOnly:true};
  const report=await diagnoseQuiescence(args,{}, {buildBase:async fixed=>{calls++;assert.equal(fixed.provider,'boat');assert.equal(fixed.parentQuiescenceOnly,true);assert.equal(fixed.parentPreflightOnly,false);assert.equal(fixed.retainArtifact,false);assert.equal(fixed.output,args.output+'.builder.json');assert.equal(fixed.deriveLineage,args.deriveLineage);return{runId:'original',machineId:'exact-id',status:mode==='success'?'parent-quiescence-verified':'failed',inspectionRequired:mode==='uncertain',cleanupVerified:mode!=='uncertain',parentSupervisorHygiene:{status:mode==='success'?'passed':'rejected',code:'parent_stop_job_unconfirmed',afterUnitFacts:{unit:{ActiveState:'failed'}}}};}});
  assert.equal(calls,1);assert.equal(report.status,mode==='success'?'diagnosed':mode==='uncertain'?'observation-deferred':'failed');assert.equal(report.inspectionRequired,mode==='uncertain');assert.equal(report.overlayInstalled,false);assert.equal(report.snapshotSubmitted,false);assert.equal(report.credentialsDelivered,false);assert.deepEqual(JSON.parse(await readFile(args.output)),JSON.parse(JSON.stringify(report)));
  await assert.rejects(diagnoseQuiescence(args,{}, {buildBase:()=>{throw Error('must-not-repeat')}}),/ExistingPublicationOperation/);
 }finally{await rm(root,{recursive:true});}
});
