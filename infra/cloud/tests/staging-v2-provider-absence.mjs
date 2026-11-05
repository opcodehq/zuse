import test from 'node:test';
import assert from 'node:assert/strict';
import {inspectAbsence} from '../staging-v2-provider-absence.mjs';

for(const provider of ['e2b','daytona']){
 test(provider+' inspects exact original IDs with only GET and fails closed',async()=>{
  const calls=[];const env={E2B_API_KEY:'fixture-e2b',DAYTONA_API_KEY:'fixture-daytona'};
  const run=status=>inspectAbsence({provider,targets:['original-123']},{env,fetchFn:async(url,options)=>{calls.push({url,options});return {status};}});
  assert.equal((await run(404)).allAbsent,true);
  assert.equal((await run(200)).allAbsent,false);
  for(const status of [301,401,403,409,429,500,503])await assert.rejects(run(status));
  for(const {url,options} of calls){
   assert.equal(options.method,'GET');assert.equal(options.redirect,'error');
   assert.equal(url,(provider==='e2b'?'https://api.e2b.app/sandboxes/':'https://app.daytona.io/api/sandbox/')+'original-123');
   assert.deepEqual(options.headers,provider==='e2b'?{'X-API-Key':'fixture-e2b'}:{Authorization:'Bearer fixture-daytona'});
   assert.equal(options.body,undefined);
  }
 });
}
test('invalid inventory cannot initiate any provider inspection',async()=>{
 let reads=0;const fetchFn=async()=>{reads++;throw Error('unexpected');};
 for(const targets of [[],['../other'],['a','a'],['a/b'],['a','b','c','d','e']])await assert.rejects(inspectAbsence({provider:'e2b',targets},{env:{E2B_API_KEY:'fixture'},fetchFn}));
 await assert.rejects(inspectAbsence({provider:'e2b',targets:['a']},{env:{},fetchFn}));
 assert.equal(reads,0);
});
test('all original targets must be absent and native adapter closes',async()=>{
 let closed=0;const reads=[];
 const result=await inspectAbsence({provider:'boat',targets:['first','second']},{boat:async()=>({absent:async id=>{reads.push(id);return id==='first';},close:()=>closed++})});
 assert.deepEqual(reads,['first','second']);assert.equal(closed,1);
 assert.equal(result.checkedCount,2);assert.equal(result.absentCount,1);assert.equal(result.allAbsent,false);
});
