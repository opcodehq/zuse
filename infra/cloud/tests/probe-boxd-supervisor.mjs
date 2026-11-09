import test from 'node:test';
import assert from 'node:assert/strict';
import {cleanupUncertainBuilder,residentExtensions} from '../probe-boxd-supervisor.mjs';
test('invisible uncertain original allocation never becomes a verified cleanup',async()=>{
 const sdk={machines:{get:async()=>{throw Object.assign(new Error('absent'),{name:'NotFoundError'});}}};
 assert.equal(await cleanupUncertainBuilder(sdk,{runId:'original',cleanupVerified:false},()=>assert.fail('must not delete another resource')),false);
});
test('uncertain original allocation cleans only its reconciled identity',async()=>{
 const calls=[];const sdk={machines:{get:async name=>{assert.equal(name,'original');return{id:'original-id',name};}}};
 assert.equal(await cleanupUncertainBuilder(sdk,{runId:'original',cleanupVerified:false},async(id,name)=>calls.push([id,name])),true);
 assert.deepEqual(calls,[['original-id','original']]);
 sdk.machines.get=async()=>({id:'other-id',name:'other'});
 assert.equal(await cleanupUncertainBuilder(sdk,{runId:'original',cleanupVerified:false},()=>assert.fail('identity mismatch')),false);
});

test('provider proof checks exact installed extension versions after image attestation',()=>{
 assert.deepEqual(residentExtensions({},{}),{repairVersion:null,recreationVersion:null});
 assert.throws(()=>residentExtensions({},{},true),/RequiredResidentExtensionMissing/);
 const manifest={rustRuntimeRepairVersion:1,rustRecreationVersion:1};
 assert.deepEqual(residentExtensions(manifest,{repairVersion:1,recreationVersion:1}),{repairVersion:1,recreationVersion:1});
 assert.throws(()=>residentExtensions(manifest,{repairVersion:1}),/ResidentExtensionMismatch/);
 assert.throws(()=>residentExtensions({rustRuntimeRepairVersion:2},{repairVersion:2}),/UnknownResidentExtension/);
 assert.throws(()=>residentExtensions(manifest,{repairVersion:2,recreationVersion:1}),/ResidentExtensionMismatch/);
});
