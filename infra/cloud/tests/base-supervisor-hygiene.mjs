import test from 'node:test';
import assert from 'node:assert/strict';
import{cleanBuilderSupervisor,verifyHygiene}from'../base-supervisor-hygiene.mjs';
import{cleanSupervisorForSnapshot}from'./hygiene-fixture.mjs';
test('only the original complete idle-stop receipt permits capture',async()=>{
 const request={operationId:'original-publication',runtimeSha256:'a'.repeat(64),legacyBuilder:false};
 const valid=await cleanSupervisorForSnapshot('original',request);assert.equal(verifyHygiene(valid,request),valid);
 for(const patch of [{operationId:'different'},{runtimeSha256:'b'.repeat(64)},{atomicStopAcknowledged:false},{originalResidentExited:false},{supervisorLockReleased:false},{runtimeDataAbsent:false},{enabledBootServiceRetained:false},{bootServiceSha256:null},{removed:['supervisor.lock','user-data']},{legacyBuilder:true}])assert.throws(()=>verifyHygiene({...valid,...patch},request),/CleanSupervisorHygieneRejected/);
});
test('legacy parent stop is explicit and cannot certify final snapshot',async()=>{
 const request={operationId:'same-original',runtimeSha256:'a'.repeat(64),legacyBuilder:true};const receipt=await cleanSupervisorForSnapshot('original',request);
 verifyHygiene(receipt,request);assert.throws(()=>verifyHygiene(receipt,{...request,legacyBuilder:false}),/CleanSupervisorHygieneRejected/);
});
test('hygiene provenance and rejected receipt persist before refusing capture',async()=>{
 const record={};const events=[];const adapter={cleanSupervisorForSnapshot:async(id,request)=>{assert.equal(id,'original');events.push('original-idle-check');return{version:1,status:'rejected',code:'publication_runtime_bound'};}};
 await assert.rejects(()=>cleanBuilderSupervisor({adapter,id:'original',rootShell:'sudo -n sh -c',operationId:'same-original',runtimeSha256:'a'.repeat(64),record,persist:async()=>events.push('durable-receipt')}),/CleanSupervisorHygieneRejected/);
 assert.match(record.baseHygieneSourceSha256,/^[a-f0-9]{64}$/);assert.equal(record.supervisorHygiene.code,'publication_runtime_bound');assert.deepEqual(events,['durable-receipt','original-idle-check','durable-receipt']);
});
test('disabled factory stop is opt-in and cannot waive atomic or legacy rules',async()=>{
 const request={operationId:'cold-factory',runtimeSha256:'a'.repeat(64),legacyBuilder:false,disabledBootService:true};
 const receipt={...await cleanSupervisorForSnapshot('original',request),enabledBootServiceRetained:false,disabledBootServiceRetained:true};verifyHygiene(receipt,request);
 assert.throws(()=>verifyHygiene(receipt,{...request,disabledBootService:false}));assert.throws(()=>verifyHygiene({...receipt,atomicStopAcknowledged:false},request));
 await assert.rejects(cleanBuilderSupervisor({adapter:{},id:'original',rootShell:'sudo -n sh -c',...request,legacyBuilder:true,record:{},persist:async()=>{}}));
});
