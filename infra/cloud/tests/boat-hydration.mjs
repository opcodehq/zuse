import test from'node:test';
import assert from'node:assert/strict';
import{waitBoatHydration}from'../boat-hydration.mjs';
test('command ready cannot substitute for same-ID hydrated true',async()=>{
 let reads=0,sleeps=0;const facts=[];
 const result=await waitBoatHydration({identity:async id=>{reads++;return{id,name:'original-name',state:'ready',hydrated:reads===3,hydratedAt:reads===3?'2026-10-30T00:00:00Z':null};}},'original-id','original-name',{sleep:async()=>{sleeps++;},onObservation:async f=>facts.push(f)});
 assert.equal(reads,3);assert.equal(sleeps,2);assert.equal(result.hydrated,true);assert.equal(facts[0].hydrated,false);
});
test('missing hydration contract and wrong identity fail closed without mutation',async()=>{
 for(const value of[{id:'original-id',name:'original-name',state:'ready'},{id:'different',name:'original-name',hydrated:true},{id:'original-id',name:'different',hydrated:true}]){
  let reads=0;await assert.rejects(waitBoatHydration({identity:async()=>{reads++;return value;}},'original-id','original-name',{sleep:async()=>assert.fail('must not guess readiness')}));assert.equal(reads,1);
 }
});
test('pending hydration exhausts bounded original observation and never extends TTL',async()=>{
 let now=0,reads=0;const adapter={identity:async()=>{reads++;return{id:'original-id',name:'original-name',state:'ready',hydrated:false};},resume:()=>assert.fail('resume forbidden'),create:()=>assert.fail('create forbidden')};
 await assert.rejects(waitBoatHydration(adapter,'original-id','original-name',{deadlineMs:2000,clock:()=>now,sleep:async ms=>{now+=ms;}}),/OriginalHydrationPending/);
 assert.equal(reads,3);assert.equal(now,2000);
});
