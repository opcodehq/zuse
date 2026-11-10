import test from'node:test';
import assert from'node:assert/strict';
import{mkdtemp,readFile,rm}from'node:fs/promises';
import{resolve,join}from'node:path';
import{execFile}from'node:child_process';
import{promisify}from'node:util';
import{diagnoseParent}from'../diagnose-boat-parent.mjs';
const run=promisify(execFile);
for(const mode of['passed','delayed-once','repeated-timeout','unclassified','fixed-error','lost-create','unknown-create'])test('one original parent diagnostic '+mode,async()=>{
 const root=await mkdtemp(resolve('.context/parent-diagnostic-test-'));
 try{
  const{stdout}=await run('python3',['-B','infra/cloud/tests/managed-base-derive.py','--emit-fixture',root]);const fixture=JSON.parse(stdout);
  const bundle=fixture.input,manifest=JSON.parse(await readFile(join(bundle,'bundle.json'),'utf8'));
  const lineage=await readFile(join(bundle,'derivation.json'));const parent={artifactId:'zuse-base-build-2dd7fb58465b4c7999313fd8',providerArtifactVersion:'c72c4d51-353f-4315-90d4-2b5dc69ded0c',sha256:fixture.expected,bytes:lineage,parentRuntimeSha256:'a'.repeat(64)};
  const payloadBytes=(await Promise.all(['bundle.json','harness-release.py','harness-manifest.json'].map(n=>readFile(join(bundle,n))))).reduce((n,b)=>n+b.length,lineage.length);
  let creates=0,uploads=0,hardens=0,removes=0,reads=0,id,name;
  const passed={version:1,status:'passed',parentArtifactId:parent.artifactId,parentLineageSha256:parent.sha256,parentRuntimeSha256:parent.parentRuntimeSha256,runtimeSha256:manifest.runtimeSha256,harnessRelease:manifest.harnessRelease,sourceClean:true,completeCodexAssetsVerified:true};
  const adapter={
   snapshotState:async()=>({name:parent.artifactId,status:'ready',snapshotId:parent.providerArtifactVersion}),
   createFromCleanSnapshot:async(n,a,o)=>{creates++;name=n;id='original-parent-clone';assert.equal(a,parent.artifactId);assert.deepEqual(o,{ttlSeconds:600});if(mode.includes('create'))throw new Error('LostReply');return id;},
   find:async()=>mode==='unknown-create'?undefined:id,
   identity:async target=>({id:target,name}),ready:async()=>true,close:()=>{},
   upload:async(_id,path,data)=>{uploads++;assert.ok(path.startsWith('/tmp/zuse-parent-preflight/'));return data.length;},
   remove:async target=>{assert.equal(target,id);removes++;id=undefined;},absent:async()=>id===undefined,
   snapshot:async()=>assert.fail('snapshot forbidden'),resume:async()=>assert.fail('resume forbidden'),stop:async()=>assert.fail('stop forbidden'),
   exec:async(_id,command)=>{
    if(command==='id -u')return'1000';
    if(command.includes('AF_NETLINK'))return JSON.stringify({version:1,residentPid:123,path:{inode:1},sockets:[{vfsInode:1,pathInodeMatchesBoundVfs:true}]});
    if(command.includes('--parent-preflight')){
     reads++;
     if(!hardens)return JSON.stringify({version:1,status:'rejected',code:'parent_metadata_not_protected',failures:['/usr/local','/usr/local/bin','/usr/local/lib'].map(path=>({path,uid:1000,gid:1000,mode:'0o755',symlink:false}))});
     if(mode==='repeated-timeout'||mode==='delayed-once'&&reads===2)return JSON.stringify({version:1,status:'rejected',code:'parent_preflight_failed',inspection:{stage:'resident-idle-ping',errorClass:'TimeoutExpired',elapsedMs:15000}});
     if(mode==='unclassified')return JSON.stringify({version:1,status:'rejected',code:'parent_preflight_failed'});
     if(mode==='fixed-error')return JSON.stringify({version:1,status:'rejected',code:'parent_preflight_failed',inspection:{stage:'native-distribution-full-integrity',errorClass:'PermissionError',errno:'EACCES'}});
     return JSON.stringify(passed);
    }
    if(command.includes('managed_base_ancestor_hardening_rejected')){hardens++;return JSON.stringify({version:2,recursive:false,paths:['/opt','/usr/local','/usr/local/bin','/usr/local/lib'].map(path=>({path,recursive:false,after:{uid:0,gid:0,mode:'0o755'}}))});}
    if(command.startsWith('printf %s'))return JSON.stringify({schema:1,paths:{},residentProcess:{pid:123,startTicks:456},service:{ActiveState:'active',MainPID:'123'}});
    return'';
   },
  };
  const output=join(root,'original-report.json');const options={bundle,output,parent,manifest,payloadBytes};
  const result=await diagnoseParent(options,adapter,{sleep:async()=>{}});
  assert.equal(creates,1);assert.equal(result.snapshotSubmitted,false);assert.equal(result.overlayInstalled,false);assert.equal(result.credentialsDelivered,false);
  const deferred=['repeated-timeout','unclassified','unknown-create'].includes(mode);
  assert.equal(removes,deferred?0:1);assert.equal(result.cleanupVerified,!deferred);
  if(mode!=='unknown-create'){assert.equal(uploads,4);assert.equal(hardens,1);assert.ok(result.observations.length>=2);}
  else{assert.equal(uploads,0);assert.equal(hardens,0);}
  if(mode==='delayed-once'){assert.equal(reads,3);assert.equal(result.retainedForObservation,true);assert.equal(result.status,'diagnosed');}
  if(mode==='fixed-error'){assert.equal(reads,2);assert.equal(result.status,'classified-rejection');}
  if(mode==='repeated-timeout'){
   assert.equal(reads,4);assert.equal(result.status,'deferred-observation');
   const original=result.resourceId;const resumed=await diagnoseParent({...options,observeExisting:true},adapter,{maxObservations:1,sleep:async()=>{}});
   assert.equal(resumed.resourceId,original);assert.equal(creates,1);assert.equal(uploads,4);assert.equal(hardens,1);assert.equal(removes,0);
  }
  await assert.rejects(diagnoseParent(options,adapter));assert.equal(creates,1,'existing original name/report cannot be resubmitted');
 }finally{await rm(root,{recursive:true});}
});
