import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {freshBootCommand,validFreshBootSpec,verifyFreshBootReceipt,proveFreshBoot} from '../managed-base-fresh-boot.mjs';
const fields=['runtimeSha256','supervisorSha256','serviceSha256','manifestSha256','helperSha256'];
const spec={version:1,mode:'stopped-memory-snapshot-v1',...Object.fromEntries(fields.map((k,i)=>[k,String(i+1).repeat(64)]))};
const binding={purpose:'publication-proof',operationId:'op',workspaceId:'image-proof',generation:1,storageIncarnationId:'disk',sourceMachineId:'machine'};
function ready(p){return {...p.request,status:'ready',requestSha256:p.requestSha256,resident:{residentPid:42,residentStartTicks:123,bootNonceSha256:'a'.repeat(64),ipcVerified:true,unbound:true,childAlive:false,recoveryAlive:false}};}
test('secure transport is bound to exact metadata and installed helper digest',async()=>{
 const p=await freshBootCommand(spec,binding);assert.match(p.command,/sudo -n \/usr\/bin\/python3 -I -B/);assert.ok(p.command.includes(spec.helperSha256));
 const encoded=p.command.match(/'([^']+)'$/)[1];assert.deepEqual(JSON.parse(Buffer.from(encoded,'base64')),p.request);
 assert.equal(p.request.provider,'boxd');assert.equal(validFreshBootSpec({...spec,unexpected:true}),false);
 for(const changed of [{...binding,token:'secret'},{...binding,generation:0},{...binding,sourceMachineId:'../machine'},{...binding,purpose:'retained-resume'}])await assert.rejects(freshBootCommand(spec,changed));
});
test('ready receipt requires exact current source and all idle peer evidence',async()=>{
 const p=await freshBootCommand(spec,binding);assert.equal(verifyFreshBootReceipt(p.request,ready(p)).status,'ready');
 for(const mutate of [r=>r.sourceMachineId='other',r=>r.operationId='other',r=>r.imagePins.runtimeSha256='f'.repeat(64),r=>r.requestSha256='f'.repeat(64),r=>r.resident.childAlive=true,r=>r.resident.ipcVerified=false,r=>r.resident.residentStartTicks=0]){const r=structuredClone(ready(p));mutate(r);assert.throws(()=>verifyFreshBootReceipt(p.request,r));}
});
test('pending observes one original command and persists intent before execution',async()=>{
 const p=await freshBootCommand(spec,binding);const order=[];const commands=[];let calls=0;
 const adapter={exec:async(id,cmd)=>{order.push('exec');assert.equal(id,'machine');commands.push(cmd);return JSON.stringify(++calls===1?{...p.request,status:'starting',requestSha256:p.requestSha256}:ready(p));}};
 const result=await proveFreshBoot(adapter,'machine',spec,binding,{onIntent:async()=>order.push('intent'),onObservation:async()=>order.push('observe'),sleep:async()=>{}});
 assert.equal(result.receipt.status,'ready');assert.deepEqual(order,['intent','exec','observe','exec','observe']);assert.equal(new Set(commands).size,1);
});
test('uncertain execution is propagated without another dispatch',async()=>{
 let calls=0;await assert.rejects(proveFreshBoot({exec:async()=>{calls++;throw Error('lost observation');}},'machine',spec,binding));assert.equal(calls,1);
});
test('expired observation and wrong original never become a ready proof',async()=>{
 const p=await freshBootCommand(spec,binding);let calls=0;
 await assert.rejects(proveFreshBoot({exec:async()=>{calls++;return JSON.stringify({...p.request,status:'starting',requestSha256:p.requestSha256});}},'machine',spec,binding,{deadlineMs:0}),/ObservationPending/);assert.equal(calls,1);
 await assert.rejects(proveFreshBoot({exec:async()=>JSON.stringify({...p.request,sourceMachineId:'wrong',status:'starting'})},'machine',spec,binding),/ReceiptInvalid/);
});
test('Boat mode scopes unchanged request to disabled post-hydration owner and bound retained peer',async()=>{
 const boat={...spec,mode:'post-hydration-first-start-v1'};const initial=await freshBootCommand(boat,binding);assert.equal(initial.request.provider,'boat');verifyFreshBootReceipt(initial.request,ready(initial));
 const p=await freshBootCommand(boat,{...binding,purpose:'retained-resume'});const r=ready(p);r.resident={...r.resident,unbound:false,identityVerified:true,childAlive:true};verifyFreshBootReceipt(p.request,r);
 for(const patch of[{unbound:true},{identityVerified:false},{childAlive:'true'},{recoveryAlive:true}])assert.throws(()=>verifyFreshBootReceipt(p.request,{...r,resident:{...r.resident,...patch}}));
});
