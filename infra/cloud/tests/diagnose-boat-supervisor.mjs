import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {diagnose,validatePrepared} from '../diagnose-boat-supervisor.mjs';
const prepared={provider:'boat',status:'prepared-uncertified',cleanupVerified:true,artifactRetained:true,artifactId:'zuse-base-build-'+'a'.repeat(24),runtimeSha256:'27a922d084d65c500ab8afecb0eb42c144b73f20de78e41cef857074479c4756',manifest:{managedBase:true,credentialsRemoved:true,repositories:[],rustRuntimeSupervisorVersion:1,rustGithubCliVersion:1,githubCli:{sha256:'b'.repeat(64),version:'gh version 2.100.0'}},githubCliProof:{status:'passed'}};
for(const failure of ['none','cold-service','second-ping','lost-create','unobserved-create'])test('existing Boat27 diagnostic '+failure,async()=>{
 const root=await mkdtemp(join(tmpdir(),'boat-diag-'));let creates=0,deletes=0,name,id,pings=0;
 const adapter={snapshotState:async()=>({status:'ready',name:prepared.artifactId}),createFromCleanSnapshot:async(n,artifact,options)=>{creates++;name=n;assert.equal(artifact,prepared.artifactId);assert.deepEqual(options,{ttlSeconds:600});id='original-restored';if(failure.includes('create'))throw new Error('LostResponse');return id;},identity:async target=>({id:target,name}),ready:async()=>true,find:async()=>failure==='unobserved-create'?undefined:id,remove:async target=>{assert.equal(target,id);deletes++;id=undefined;},absent:async()=>id===undefined,close:()=>{},exec:async(_id,command)=>{
  if(command.includes('--receipt'))return JSON.stringify({version:1,required:true,status:'passed',accountRequired:false,modelCalled:false,systemReceipt:prepared.manifest.githubCli,globalShim:{contentMatches:true},loginShellMatchesGlobal:true,globalVersionMatchesReceipt:true,canonicalPath:'/opt/zuse/bin/gh',ancestorsControlled:true,profileMatches:true});
  if(command.includes('runtime-supervisor.py ping')){pings++;if(failure==='cold-service'&&pings===1)throw new Error('BuilderCommandFailed');if(failure==='second-ping'&&pings===2)throw new Error('BuilderCommandFailed');return JSON.stringify({version:1,bound:false,childAlive:false,recoveryAlive:false,residentPid:42,bootNonce:'test-only-nonce'});}
  return command==='true'?'':JSON.stringify({version:1,path:{inode:100},sockets:[]});
 }};
 try{
  const output=join(root,'original.json');const result=await diagnose({prepared,output,probeSource:'READONLY'},adapter,{sleep:async()=>{}});
  assert.equal(creates,1);assert.equal(deletes,failure==='unobserved-create'?0:1);assert.equal(result.cleanupVerified,failure!=='unobserved-create');
  if(failure==='second-ping'){assert.equal(result.errorStage,'second-supervisor-ping');assert.equal(result.githubCliProof.status,'passed');assert.ok(result.observations.failure);assert.ok(result.stages.some(s=>s.name==='separate-exec-true'&&s.status==='completed'));}
  if(failure==='cold-service'){assert.equal(result.status,'diagnosed');assert.equal(result.firstPingFailedObservations,1);assert.deepEqual(result.firstPingObservations,[{attempt:1,code:'BuilderCommandFailed'}]);assert.equal(pings,3);}
  if(failure==='none'){assert.equal(result.status,'diagnosed');assert.equal(result.sameResident,true);assert.equal(pings,2);}
  const persisted=JSON.parse(await readFile(output,'utf8'));assert.equal(persisted.restoreName,name);assert.equal(persisted.runtimeReplaced,false);assert.equal(persisted.credentialsDelivered,false);
  await assert.rejects(()=>diagnose({prepared,output,probeSource:'READONLY'},adapter));assert.equal(creates,1,'existing intent is never replayed');
 }finally{await rm(root,{recursive:true,force:true});}
});
test('diagnostic refuses other/new/nonclean artifacts before provider effects',()=>{assert.throws(()=>validatePrepared({...prepared,runtimeSha256:'0'.repeat(64)}));assert.throws(()=>validatePrepared({...prepared,manifest:{...prepared.manifest,credentialsRemoved:false}}));});
