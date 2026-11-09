import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {mkdtemp,writeFile,symlink,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {inspectRehearsalProvider,verifiedSdk} from '../project-cache-rehearsal-provider.mjs';
const sha=b=>createHash('sha256').update(b).digest('hex');
const packed=v=>{const bytes=Buffer.from(JSON.stringify(v));return{bytes,sha256:sha(bytes)}};
const OP='cacheop_'+'a'.repeat(32), suffix='a'.repeat(32);
const packet={kind:'boxd-project-cache-serial-rehearsal-v1',environment:'staging',effectsAuthorized:false,runId:'b'.repeat(32),
 tasks:Object.fromEntries(['miss','hit','restore'].map(k=>[k,{idempotencyKey:'original-'+k}])),source:{ownerId:'owner',actorId:'owner',projectId:'project'},pilot:{ownerId:'owner',projectId:'project',baseArtifactId:'base',baseArtifactVersion:'1',protectedMachineId:'original'},
 bounds:{ownerId:'owner',projectId:'project',protectedMachineId:'original',protectedWorkspaceId:'protected-workspace',protectedGeneration:1}};
const machine=(id,name=id,status='running')=>({id,name,status});
const snapshot=(id,name=id,version=1)=>({id,name,version,status:'ready'});
const empty={operations:[],workspaces:[],allocations:[]};
const intent=label=>({action:'create-'+label,method:'POST',path:'/v1/cloud/workspaces',body:{projectId:'project',providerId:'boxd',idempotencyKey:'original-'+label}});
function operation(ids=true){return{id:OP,resources:{provider:ids?{builderId:'builder',verifierId:'verifier',artifactId:'prepared'}:{},effects:Object.fromEntries(['allocate','verify','capture'].map(k=>[k,{requested:true,operationId:OP+':'+k}]))}}}
function input(observation=empty,phase='inventory',baseline=null){
 const p=packed(packet),o=packed({packetSha256:p.sha256,readOnly:true,observedAtMs:100000,observation});
 return{packetBytes:p.bytes,packetSha256:p.sha256,observations:[o],phase,baseline:baseline?packed(baseline):null,
 tasks:Object.fromEntries(['miss','hit','restore'].map(k=>[k,{intentPresent:false,outcome:null}]))};
}
function sdkFixture({machines=[machine('original'),machine('unrelated')],snapshots=[snapshot('base'),snapshot('old-artifact')],getOverride,listOverride}={}){
 const calls=[];const notFound=()=>Object.assign(Error('safe fixture'),{name:'NotFoundError'});
 const get=(kind,rows)=>async value=>{calls.push([kind+'.get',value]);if(getOverride){const r=getOverride(kind,value);if(r!==undefined)return r;}const row=rows.find(r=>r.id===value||r.name===value);if(!row)throw notFound();return row;};
 return{calls,machines:{list:async params=>{calls.push(['machines.list',params]);return listOverride??machines;},get:get('machines',machines)},snapshots:{list:async()=>{calls.push(['snapshots.list']);return snapshots;},get:get('snapshots',snapshots)}};
}
const inspect=(i,sdk)=>inspectRehearsalProvider(i,{sdk,now:()=>100000});
const baseline=async options=>inspect(input(),sdkFixture(options));

test('complete pinned-SDK array inventory counts controlled original only and preserves unrelated machines/snapshots',async()=>{
 const sdk=sdkFixture(),p=await inspect(input(),sdk);assert.equal(p.completeProviderInventory,true);assert.equal(p.totalBoxdMachines,2);
 assert.deepEqual(p.controlledProviderCounts,{boxd:1});assert.deepEqual(p.unverifiedProviders,['boat','e2b','daytona']);
 assert.equal(p.protectedOriginalPresent,true);assert.equal(p.baseSnapshotPreserved,true);assert.equal(p.unrelatedInventory.machines.length,1);
 assert.equal(p.unrelatedPreserved,null);assert.equal(p.baselineCaptured,true);assert.deepEqual(sdk.calls[0],['machines.list',{allContexts:true}]);
 assert(sdk.calls.every(([m])=>/\.(list|get)$/.test(m)));
});
test('clean proves original builder/verifier/snapshot ID AND name404, retaining base and unrelated baseline',async()=>{
 const b=await baseline(),sdk=sdkFixture(),p=await inspect(input({...empty,operations:[operation()]},'clean',b),sdk);
 assert.equal(p.providerAbsenceVerified,true);assert.equal(p.preparationMachinesIndependentlyAbsent,true);assert.equal(p.unrelatedPreserved,true);
 assert.deepEqual(p.targets.map(t=>[t.kind,t.id404,t.name404,t.state]),[['builder',true,true,'absent-404'],['verifier',true,true,'absent-404'],['snapshot',true,true,'absent-404']]);
 for(const [kind,id,prefix]of[['machines','builder','pc-b-'],['machines','verifier','pc-v-'],['snapshots','prepared','pc-s-']]){
  assert(sdk.calls.some(c=>c[0]===kind+'.get'&&c[1]===id));assert(sdk.calls.some(c=>c[0]===kind+'.get'&&c[1]===prefix+suffix));
 }
});
test('present original builder remains present, consumes one controlled slot and cannot certify deletion',async()=>{
 const b=await baseline(),sdk=sdkFixture({machines:[machine('original'),machine('unrelated'),machine('builder','pc-b-'+suffix)]});
 const p=await inspect(input({...empty,operations:[operation()]},'preparation-absent',b),sdk);
 assert.equal(p.controlledProviderCounts.boxd,2);assert.equal(p.providerAbsenceVerified,false);assert.equal(p.preparationMachinesIndependentlyAbsent,false);
 assert.equal(p.targets[0].state,'present');assert.equal(p.unrelatedPreserved,true);
});
test('ID404 with still-present original name never establishes absence',async()=>{
 const b=await baseline(),sdk=sdkFixture({machines:[machine('original'),machine('unrelated'),machine('builder','pc-b-'+suffix)],getOverride:(kind,value)=>{
  if(kind==='machines'&&value==='builder')throw Object.assign(Error('absent id fixture'),{name:'NotFoundError'});
 }});
 const p=await inspect(input({...empty,operations:[operation()]},'clean',b),sdk);assert.equal(p.targets[0].id404,true);assert.equal(p.targets[0].name404,false);assert.equal(p.providerAbsenceVerified,false);
});
test('unknown requested effect remains unresolved even after exact original-name404, with no invented ID or retry',async()=>{
 const b=await baseline(),p=await inspect(input({...empty,operations:[operation(false)]},'clean',b),sdkFixture());
 assert.equal(p.providerAbsenceVerified,false);assert.equal(p.unresolved.length,3);
 assert(p.targets.every(t=>t.state==='requested-id-unrecorded'&&t.id404===null&&t.name404===true&&t.id===null));
});
test('original-name reconciliation can report a discovered ID but never invents its durable receipt',async()=>{
 const b=await baseline(),sdk=sdkFixture({machines:[machine('original'),machine('unrelated'),machine('discovered','pc-b-'+suffix)]});
 const p=await inspect(input({...empty,operations:[operation(false)]},'clean',b),sdk);
 assert.equal(p.targets[0].discoveredOriginalNameId,'discovered');assert.equal(p.targets[0].id,null);
 assert.equal(p.targets[0].state,'requested-id-unrecorded');assert.equal(p.providerAbsenceVerified,false);assert.equal(p.controlledProviderCounts.boxd,2);
});
test('never-requested phases are distinct from ID404',async()=>{
 const b=await baseline(),op=operation(false);op.resources.effects={};const p=await inspect(input({...empty,operations:[op]},'clean',b),sdkFixture());
 assert(p.targets.every(t=>t.state==='not-submitted'&&t.id404===null&&t.name404===true));assert.equal(p.providerAbsenceVerified,true);
});
test('task cleanup uses original persisted allocation name/ID across SQL tombstone; no task name is guessed',async()=>{
 const b=await baseline(),i=input({operations:[],workspaces:[{id:'task-ws',machineId:null,state:'deleted',generation:2}],allocations:[]},'clean',b);
 const previous=packed({packetSha256:i.packetSha256,readOnly:true,observedAtMs:99999,observation:{operations:[],workspaces:[{id:'task-ws',machineId:'task-vm',generation:1}],allocations:[{workspaceId:'task-ws',operationId:'original-allocation',machineName:'original-task-name',generation:1}]}});
 i.observations.unshift(previous);i.tasks.hit={intentPresent:true,intent:intent('hit'),outcome:{status:201,value:{workspace:{workspaceId:'task-ws',projectId:'project',providerId:'boxd',providerSandboxId:null}}}};
 const sdk=sdkFixture(),p=await inspect(i,sdk);assert.equal(p.providerAbsenceVerified,true);assert.equal(p.targets[0].name,'original-task-name');assert.equal(p.targets[0].id,'task-vm');assert.equal(p.targets[0].operationId,'original-allocation');
 assert(sdk.calls.some(c=>c[0]==='machines.get'&&c[1]==='original-task-name'));
});
test('uncertain task admission or missing original allocation cannot certify cleanup',async()=>{
 const b=await baseline(),i=input(empty,'clean',b);i.tasks.miss={intentPresent:true,intent:intent('miss'),outcome:{uncertain:true}};
 const p=await inspect(i,sdkFixture());assert.equal(p.providerAbsenceVerified,false);assert.equal(p.unresolved[0].code,'original_admission_unresolved');
});
test('scope, SRI, effect identity, base/protected target, stale observation and preservation-baseline failures precede provider reads',async()=>{
 const b=await baseline();const cases=[];
 const sri=input();sri.packetSha256='f'.repeat(64);cases.push(sri);
 const stale=input();stale.observations=[packed({packetSha256:stale.packetSha256,readOnly:true,observedAtMs:1,observation:empty})];cases.push(stale);
 const invalid=operation();invalid.resources.effects.allocate.operationId='foreign:allocate';cases.push(input({...empty,operations:[invalid]}));
 const original=operation();original.resources.provider.builderId='original';cases.push(input({...empty,operations:[original]}));
 const base=operation();base.resources.provider.artifactId='base';cases.push(input({...empty,operations:[base]}));
 const task=input();task.tasks.hit={intentPresent:true,intent:intent('hit'),outcome:{status:201,value:{workspace:{workspaceId:'foreign',projectId:'other',providerId:'boxd'}}}};cases.push(task);
 const badBase=input(empty,'clean',b);badBase.baseline.sha256='e'.repeat(64);cases.push(badBase);
 cases.push(input(empty,'clean'));
 for(const i of cases){const sdk=sdkFixture();await assert.rejects(inspect(i,sdk));assert.equal(sdk.calls.length,0);}
});
test('a new unrelated resource or altered snapshot/version is never hidden by controlled resource caps',async()=>{
 const b=await baseline(),i=input({...empty,operations:[operation()]},'clean',b);
 await assert.rejects(inspect(i,sdkFixture({snapshots:[snapshot('base'),snapshot('old-artifact'),snapshot('foreign-new')]})),/unrelated_inventory_changed/);
 await assert.rejects(inspect(i,sdkFixture({snapshots:[snapshot('base','base',2),snapshot('old-artifact')]})),/base_snapshot_changed/);
});
test('a later operation cannot claim a resource present in the earlier unrelated baseline',async()=>{
 const b=await baseline({snapshots:[snapshot('base'),snapshot('prepared','pc-s-'+suffix)]});const sdk=sdkFixture();
 await assert.rejects(inspect(input({...empty,operations:[operation()]},'clean',b),sdk),/claimed_preexisting_resource/);assert.equal(sdk.calls.length,0);
});
test('authentication, rate limit and transport errors never become provider404; unknown/incomplete inventory fails closed',async()=>{
 const b=await baseline(),i=input({...empty,operations:[operation()]},'clean',b);
 for(const name of['AuthenticationError','PermissionDeniedError','RateLimitError','APIConnectionError']){
  const sdk=sdkFixture({getOverride:(kind,value)=>{if(value==='builder')throw Object.assign(Error('do not print response'),{name});}});
  await assert.rejects(inspect(i,sdk),/read_unavailable/);
 }
 await assert.rejects(inspect(input(),sdkFixture({listOverride:{machines:[],nextCursor:'unknown'}})),/complete_inventory_required/);
 await assert.rejects(inspect(input(),sdkFixture({machines:[machine('original'),machine('original')]})),/ambiguous_inventory/);
 const inconsistent=sdkFixture({getOverride:(kind,value)=>kind==='machines'&&value==='builder'?machine('builder','pc-b-'+suffix):undefined});
 await assert.rejects(inspect(i,inconsistent),/inventory_inconsistent/);
});
test('an inconsistent NotFound class with authentication status cannot certify absence',async()=>{
 const b=await baseline(),sdk=sdkFixture({getOverride:(_,value)=>{if(value==='builder')throw Object.assign(Error('private'),{name:'NotFoundError',status:403});}});
 await assert.rejects(inspect(input({...empty,operations:[operation()]},'clean',b),sdk),/read_unavailable/);
});
test('unapproved SDK version/closure/symlink are rejected without importing or using a provider key',async()=>{
 await assert.rejects(verifiedSdk('/unused/index.js',{version:'next',files:{}}),/sdk_pin/);
 const d=await mkdtemp(join(tmpdir(),'cache-provider-sdk-'));
 try{
  const link=join(d,'linked');await symlink(d,link);
  const manifest={version:'0.2.14',files:{'index.js':'2cb65b3cc4c6c6bae7bc52e865227364ee186aeacc1bf999c3c1e16fceeb51aa'}};
  await assert.rejects(verifiedSdk(join(link,'index.js'),manifest),/sdk_root/);
  await rm(link);
  await writeFile(join(d,'index.js'),'throw Error("must never import")');await assert.rejects(verifiedSdk(join(d,'index.js'),manifest),/sdk_sri/);
 }finally{await rm(d,{recursive:true,force:true});}
});
