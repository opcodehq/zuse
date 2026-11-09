import{cleanSupervisorForSnapshot}from'./hygiene-fixture.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';
import { build, verifyBundle, uploadExactChunk, createBoxdBuilderAdapter, createBoatBuilderAdapter, BOXD_BOOT_COMMAND } from '../build-managed-base.mjs';
const sha=value=>createHash('sha256').update(value).digest('hex');
async function fixture(){
 const root=await mkdtemp(join(tmpdir(),'zuse-base-builder-'));const paths=['runtime','claude','codex','grok','harness-manifest.json','harness-release.py','launch-runtime.sh','launch-recovery.sh','managed-base-install.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py','prepare-plan.json'];
 const files=[];for(const path of paths){const bytes=Buffer.from('synthetic '+path);await writeFile(join(root,path),bytes);files.push({path,bytes:bytes.length,sha256:sha(bytes)});}
 await writeFile(join(root,'bundle.json'),JSON.stringify({schema:1,status:'candidate-only',releaseId:'zuse-base-'+'a'.repeat(32),harnessRelease:'b'.repeat(64),runtimeSha256:files[0].sha256,files}));
 return root;
}
for(const mode of ['success','lost-create','failed-capture','lost-delete','install-failed','artifact-failed','sudo-root'])test('native builder reconciles and cleans '+mode,async()=>{
 const root=await fixture();const output=join(root,'report.json');let creates=0,deletes=0,snapDeletes=0,machine=false,snapshot=false;
 const adapter={cleanSupervisorForSnapshot,
  create:async()=>{creates++;machine=true;if(mode==='lost-create')throw new Error('OutcomeUnknown');return 'builder-id';},
  find:async()=>machine?'builder-id':undefined,
  exec:async(id,command)=>command==='id -u'?(mode==='sudo-root'?'1000':'0'):command==='sudo -n id -u'?'0':command.includes('cat /opt/zuse/preparation/status/')?JSON.stringify({managedBase:true,credentialsRemoved:true,repositories:[]}):command.includes('if test -f')?(mode==='install-failed'?'failed':'ready'):'',
  upload:async(id,path,data)=>{assert.ok(path.startsWith('/tmp/'),'Boat file API rejects paths outside /home/user or /tmp');return data.length;},snapshot:async()=>{snapshot=true;if(mode==='failed-capture')throw new Error('OutcomeUnknown');return{};},
  snapshotState:async()=>({status:mode==='artifact-failed'?'failed':'ready',id:'artifact-id'}),
  remove:async()=>{deletes++;machine=false;if(mode==='lost-delete')throw new Error('OutcomeUnknown');},absent:async()=>!machine,
  removeSnapshot:async()=>{snapDeletes++;snapshot=false;},snapshotAbsent:async()=>!snapshot,close:async()=>{},
 };
 try{const report=await build({bundle:root,output,provider:'boxd',retainArtifact:false},adapter);
  assert.equal(creates,1);assert.equal(deletes,1);assert.equal(machine,false);assert.equal(snapshot,false);
  assert.equal(report.cleanupVerified,true);assert.equal(report.artifactRetained,false);
  if(mode==='sudo-root'){assert.equal(report.executorUid,1000);assert.equal(report.installationPrivilege,'sudo-root');}
  assert.equal(report.status,['lost-create','failed-capture','install-failed','artifact-failed'].includes(mode)?'failed':'prepared-uncertified');
  assert.equal(snapDeletes,['lost-create','install-failed'].includes(mode)?0:1);
  if(mode==='install-failed'||mode==='artifact-failed')assert.ok(report.stages.reduce((sum,stage)=>sum+stage.elapsedMs,0)<1000,'explicit failure does not wait for readiness timeout');
 }finally{await rm(root,{recursive:true});}
});
test('immutable legacy bundles remain usable without advertising new helper files',async()=>{
 const root=await fixture();try{
  const path=join(root,'bundle.json');const value=JSON.parse(await readFile(path,'utf8'));
  value.files=value.files.filter(file=>!['runtime-repair.py','runtime-recreation.py'].includes(file.path));await writeFile(path,JSON.stringify(value));
  assert.equal((await verifyBundle(root)).files.length,11);
  value.files.push({path:'runtime-repair.py',bytes:1,sha256:'a'.repeat(64)});await writeFile(path,JSON.stringify(value));
  await assert.rejects(verifyBundle(root),/InvalidBundle/);
 }finally{await rm(root,{recursive:true});}
});
test('corrupt bundle fails before any allocation',async()=>{
 const root=await fixture();try{await writeFile(join(root,'runtime'),'modified');await assert.rejects(verifyBundle(root),/Bundle(Size|Integrity)Mismatch/);}finally{await rm(root,{recursive:true});}
});
test('an invisible uncertain allocation is never declared cleaned',async()=>{
 const root=await fixture();let creates=0;
 const adapter={cleanSupervisorForSnapshot,create:async()=>{creates++;throw new Error('OutcomeUnknown');},find:async()=>undefined,close:async()=>{}};
 try{const report=await build({bundle:root,output:join(root,'report.json'),provider:'boat',retainArtifact:false},adapter);
  assert.equal(creates,1);assert.equal(report.status,'cleanup-required');assert.equal(report.cleanupVerified,false);
 }finally{await rm(root,{recursive:true});}
});
test('missing hydration contract preserves the original Boat builder without install or replacement',async()=>{
 const root=await fixture();let creates=0,deletes=0,commands=0,name;
 const adapter={create:async n=>{name=n;creates++;return'original';},ready:async()=>true,identity:async id=>({id,name,state:'ready'}),exec:async()=>{commands++;},remove:async()=>{deletes++;},close:()=>{}};
 try{
  const report=await build({bundle:root,output:join(root,'hydration.json'),provider:'boat'},adapter);
  assert.equal(report.status,'observation-deferred');assert.equal(report.machineId,'original');assert.equal(report.errorCode,'ProviderHydrationUnavailable');assert.equal(report.cleanupVerified,false);assert.equal(report.inspectionRequired,true);assert.equal(creates,1);assert.equal(deletes,0);assert.equal(commands,0);
 }finally{await rm(root,{recursive:true});}
});
test('502 upload receipt reconciles exact bytes before retry and fences identity',async()=>{
 const data=Buffer.from('same exact public tool bytes');const error=Object.assign(new Error('gateway'),{status:502});let calls=0;
 const adapter={cleanSupervisorForSnapshot,upload:async()=>{calls++;throw error;},identity:async()=>({id:'original',name:'build-name'}),exec:async()=>`${data.length}\n${sha(data)}  /tmp/chunk\n`};
 const receipt=await uploadExactChunk(adapter,'original','build-name','/tmp/chunk',data);
 assert.equal(calls,1);assert.equal(receipt.sha256,sha(data));
 adapter.identity=async()=>({id:'replacement',name:'build-name'});
 await assert.rejects(uploadExactChunk(adapter,'original','build-name','/tmp/chunk',data),/IdentityMismatch/);
 assert.equal(calls,2);
});
test('missing chunk permits only bounded identical overwrite and validates byte receipt',async()=>{
 const data=Buffer.from('same bytes');let calls=0;const error=Object.assign(new Error('gateway'),{status:503});
 const adapter={cleanSupervisorForSnapshot,upload:async(id,path,bytes)=>{calls++;assert.equal(id,'original');assert.equal(path,'/tmp/chunk');assert.deepEqual(bytes,data);if(calls===1)throw error;return data.length;},identity:async()=>({id:'original',name:'build'}),exec:async()=>''};
 await uploadExactChunk(adapter,'original','build','/tmp/chunk',data);assert.equal(calls,2);
 adapter.upload=async()=>data.length-1;
 await assert.rejects(uploadExactChunk(adapter,'original','build','/tmp/chunk',data),/InvalidChunkReceipt/);
});

test('Boxd captures a permanent boot owner and foreground installer in one immutable machine',async()=>{
 const root=await mkdtemp(join(tmpdir(),'zuse-boxd-adapter-'));const old=process.env.BOXD_API_KEY;
 const modulePath=join(root,'sdk.mjs');
 await writeFile(modulePath,`export const VERSION='0.2.14';export const calls=[];export class Boxd {constructor(options){calls.push(['client',options]);this.machines={get:async id=>({id,imageRef:'provider-native-resolved-version',restartPolicy:null}),create:async input=>{calls.push(['create',input]);return{id:'original'};},exec:async(id,input)=>{calls.push(['exec',id,input]);return{success:true};}};}close(){calls.push(['close']);}}`);
 try{
  process.env.BOXD_API_KEY='synthetic-only';
  await assert.rejects(createBoxdBuilderAdapter(modulePath,'ubuntu:latest'),/ImmutableImageRequired/);
  const image='ubuntu@sha256:'+'a'.repeat(64),adapter=await createBoxdBuilderAdapter(modulePath,image);
  assert.equal(await adapter.create('isolated-builder'),'original');
  await adapter.prepare('original','exact foreground installation');adapter.close();
  const {calls}=await import(pathToFileURL(modulePath));const request=calls.find(call=>call[0]==='create')[1];
  assert.deepEqual(request.cmd,BOXD_BOOT_COMMAND);assert.equal(request.restartPolicy,'always');
  assert.equal(request.image,image);assert.equal(request.isolated,true);assert.equal(request.config.autoDestroyTimeout,1800);
  assert.equal(request.config.autoSuspendTimeout,0);assert.equal(request.fromSnapshot,undefined);
  assert.equal(calls.filter(call=>call[0]==='create').length,1);
  assert.deepEqual(calls.find(call=>call[0]==='exec'),['exec','original',{command:'exact foreground installation',timeout:840000}]);
  for(const client of calls.filter(call=>call[0]==='client'))assert.equal(client[1].maxRetries,0);
  await assert.rejects(createBoxdBuilderAdapter(modulePath,image,{nativeDefault:true}),/ImmutableImageRequired/);
  const native=await createBoxdBuilderAdapter(modulePath,undefined,{nativeDefault:true});
  await native.create('native-builder');const nativeRequest=calls.filter(call=>call[0]==='create').at(-1)[1];
  assert.equal(nativeRequest.image,undefined);assert.equal(nativeRequest.cmd,undefined);assert.equal(nativeRequest.restartPolicy,undefined);
  assert.deepEqual(await native.sourceInfo('original'),{mode:'boxd-native-systemd',imageRef:'provider-native-resolved-version',restartPolicy:null});
  await native.bootOwnerReady('original');
  assert.deepEqual(calls.filter(call=>call[0]==='exec').at(-1),['exec','original',{command:'sudo -n systemd-run --quiet --collect --wait --uid=root -- /bin/true',timeout:15000}]);
  native.close();
 }finally{if(old===undefined)delete process.env.BOXD_API_KEY;else process.env.BOXD_API_KEY=old;await rm(root,{recursive:true});}
});

test('Boat persists one tracked detached preparation receipt before observing completion',async()=>{
 const oldKey=process.env.BOAT_API_KEY,oldFetch=globalThis.fetch;let posted=0,receipt=false;
 try{
  process.env.BOAT_API_KEY='synthetic-only';
  globalThis.fetch=async(url,options)=>{
   assert.equal(options.headers.Authorization,'Bearer synthetic-only');
   if(options.method==='POST'){
    posted++;assert.equal(url,'https://boat.dev/api/v1/sandboxes/original/commands');
    assert.deepEqual(JSON.parse(options.body),{command:'install exact bundle',detached:true});
    return Response.json({processId:123,commandId:'operation-original'});
   }
   assert.ok(receipt,'persist receipt before status reads');
   assert.equal(url,'https://boat.dev/api/v1/sandboxes/original/commands/123');
   return Response.json({processId:123,status:'exited',running:false,exitCode:0});
  };
  const adapter=await createBoatBuilderAdapter();
  const operation=await adapter.prepare('original','install exact bundle',async value=>{assert.equal(value.processId,123);receipt=true;});
  assert.equal(operation.commandId,'operation-original');assert.equal(posted,1);
  globalThis.fetch=async(url,options)=>options.method==='POST'?Response.json({processId:456}):Response.json({processId:456,status:'lost',running:false,exitCode:0});
  await assert.rejects(adapter.prepare('original','one original command'),/BuilderCommandUncertain/);
 }finally{globalThis.fetch=oldFetch;if(oldKey===undefined)delete process.env.BOAT_API_KEY;else process.env.BOAT_API_KEY=oldKey;}
});

test('existing publication receipt is never overwritten or reallocated',async()=>{
 const root=await fixture();let creates=0;const output=join(root,'existing.json');
 const original=JSON.stringify({status:'cleanup-required',runId:'original-durable-operation',machineId:'original-machine'});
 await writeFile(output,original);
 try{
  await assert.rejects(build({bundle:root,output,provider:'boat'}, {create:async()=>{creates++;}}),/ExistingPublicationOperation/);
  assert.equal(creates,0);assert.equal(await readFile(output,'utf8'),original);
 }finally{await rm(root,{recursive:true});}
});

test('racing operators atomically reserve one original publication identity',async()=>{
 const root=await fixture(),output=join(root,'race.json');let allocations=0;
 const adapter={cleanSupervisorForSnapshot,create:async()=>{allocations++;throw new Error('OriginalAllocationUncertain');},find:async()=>undefined,close:()=>{}};
 try{const results=await Promise.allSettled([build({bundle:root,output,provider:'boat'},adapter),build({bundle:root,output,provider:'boat'},adapter)]);
  assert.equal(allocations,1);assert.equal(results.filter(r=>r.status==='fulfilled').length,1);
  assert.match(results.find(r=>r.status==='rejected').reason.message,/ExistingPublicationOperation/);
  const report=JSON.parse(await readFile(output,'utf8'));assert.equal(report.status,'cleanup-required');assert.equal(report.cleanupVerified,false);
 }finally{await rm(root,{recursive:true});}
});

test('Boat builder transport pins private bounded allocation and separates command readiness',async()=>{
 const oldFetch=globalThis.fetch,oldKey=process.env.BOAT_API_KEY,oldTimeout=AbortSignal.timeout;const calls=[],timeouts=[];let state='provisioning',commandStatus=503;
 try{
  process.env.BOAT_API_KEY='synthetic-only';AbortSignal.timeout=ms=>{timeouts.push(ms);return oldTimeout(ms);};
  globalThis.fetch=async(url,options)=>{
   assert.equal(options.headers.Authorization,'Bearer synthetic-only');calls.push({url,method:options.method,body:options.body?JSON.parse(options.body):null,headers:options.headers});
   if(url.endsWith('/sandboxes')&&options.method==='POST')return Response.json({sandbox:{id:'original',name:'run-original'}});
   if(url.endsWith('/commands'))return commandStatus===503?new Response('{}',{status:503}):Response.json({exitCode:0,timedOut:false});
   return Response.json({sandbox:{id:'original',state}});
  };
  const adapter=await createBoatBuilderAdapter();assert.equal(await adapter.create('run-original'),'original');
  assert.deepEqual(calls[0].body,{name:'run-original',type:'small',ttlSeconds:1800,noEnv:true,visibility:'private'});
  assert.equal(calls[0].headers['Idempotency-Key'],'run-original');
  assert.equal(await adapter.ready('original'),false);assert.equal(calls.filter(c=>c.url.endsWith('/commands')).length,0);
  state='ready';assert.equal(await adapter.ready('original'),false,'provider ready is not command ready');
  commandStatus=200;assert.equal(await adapter.ready('original'),true);
  await adapter.restore('run-original-restore','exact-snapshot');
  const restore=calls.at(-1);assert.deepEqual(restore.body,{name:'run-original-restore',from:'exact-snapshot',type:'small',ttlSeconds:1200,noEnv:true,visibility:'private'});
  assert.equal(restore.headers['Idempotency-Key'],'run-original-restore');
  await adapter.exec('original','default-bounded-command');assert.equal(calls.at(-1).body.timeoutSeconds,30);assert.equal(timeouts.at(-1),45000);
  await adapter.exec('original','fresh-helper-bounded-command',{commandTimeoutSeconds:60,fetchTimeoutMs:65000});assert.equal(calls.at(-1).body.timeoutSeconds,60);assert.equal(timeouts.at(-1),65000);
  const before=calls.length;for(const budget of[{commandTimeoutSeconds:61,fetchTimeoutMs:65000},{commandTimeoutSeconds:60,fetchTimeoutMs:60000},{commandTimeoutSeconds:60,fetchTimeoutMs:65001}])await assert.rejects(adapter.exec('original','invalid',budget),/InvalidCommandBudget/);assert.equal(calls.length,before);
  state='error';await assert.rejects(adapter.ready('original'),/ProviderMachineFailed/);
 }finally{globalThis.fetch=oldFetch;AbortSignal.timeout=oldTimeout;if(oldKey===undefined)delete process.env.BOAT_API_KEY;else process.env.BOAT_API_KEY=oldKey;}
});
test('Boat clean-source diagnostic uses exact 600-second private create contract',async()=>{
 const oldKey=process.env.BOAT_API_KEY,oldFetch=globalThis.fetch;const bodies=[];
 try{process.env.BOAT_API_KEY='synthetic-contract-key';globalThis.fetch=async(url,request)=>{assert.equal(url,'https://boat.dev/api/v1/sandboxes');assert.equal(request.method,'POST');bodies.push(JSON.parse(request.body));return{ok:true,status:200,json:async()=>({sandbox:{id:'exact-original'}})};};
  const adapter=await createBoatBuilderAdapter();
  assert.equal(await adapter.createFromCleanSnapshot('diagnostic-name','exact-parent',{ttlSeconds:600}),'exact-original');
  assert.deepEqual(bodies[0],{name:'diagnostic-name',from:'exact-parent',type:'small',ttlSeconds:600,noEnv:true,visibility:'private'});
  await adapter.createFromCleanSnapshot('builder-name','exact-parent');assert.equal(bodies[1].ttlSeconds,1800);
  await assert.rejects(adapter.createFromCleanSnapshot('forbidden','exact-parent',{ttlSeconds:601}),/InvalidBuilderTtl/);assert.equal(bodies.length,2);
 }finally{globalThis.fetch=oldFetch;if(oldKey===undefined)delete process.env.BOAT_API_KEY;else process.env.BOAT_API_KEY=oldKey;}
});
test('declared managed gh capability cannot snapshot a missing global shim',async()=>{
 const root=await fixture();let active=false,creates=0,deletes=0,captures=0,probes=0;
 const adapter={cleanSupervisorForSnapshot,create:async()=>{creates++;active=true;return'original';},
  exec:async(id,command)=>command==='id -u'?'0':command.includes('cat /opt/zuse/preparation/status/')?JSON.stringify({managedBase:true,credentialsRemoved:true,repositories:[],rustGithubCliVersion:1,githubCli:{version:'gh version 2.45.0 (2025-01-01)',sha256:'a'.repeat(64)}}):command.includes('--receipt')?(probes++,JSON.stringify({version:1,required:true,status:'rejected',code:'managed_github_global_shim_invalid',globalShim:{lexists:false},accountRequired:false,modelCalled:false})):''
  ,upload:async(id,path,data)=>data.length,prepare:async()=>{},snapshot:async()=>{captures++;},remove:async()=>{deletes++;active=false;},absent:async()=>!active,close:()=>{}};
 try{const report=await build({provider:'boxd',bundle:root,output:join(root,'missing-shim.json')},adapter);
  assert.equal(creates,1);assert.equal(deletes,1);assert.equal(probes,1);assert.equal(captures,0);assert.equal(report.cleanupVerified,true);assert.equal(report.status,'failed');assert.equal(report.errorCode,'ManagedGithubCliProofFailed');assert.equal(report.githubCliProof.globalShim.lexists,false);
 }finally{await rm(root,{recursive:true});}
});

test('fresh memory bootstrap requires explicit version and exact pinned helper',async()=>{
 const root=await fixture();try{
  const path=join(root,'bundle.json');const value=JSON.parse(await readFile(path,'utf8'));const body=Buffer.from('fixture bootstrap helper');
  await writeFile(join(root,'managed-base-fresh-boot.py'),body);
  value.files.push({path:'managed-base-fresh-boot.py',bytes:body.length,sha256:sha(body)});await writeFile(path,JSON.stringify(value));
  await assert.rejects(verifyBundle(root),/InvalidBundle/);
  value.managedBaseFreshBootVersion=1;await writeFile(path,JSON.stringify(value));assert.equal((await verifyBundle(root)).managedBaseFreshBootVersion,1);
  value.managedBaseFreshBootVersion=2;await writeFile(path,JSON.stringify(value));await assert.rejects(verifyBundle(root),/UnsupportedFreshBootVersion/);
  value.managedBaseFreshBootVersion=1;await writeFile(path,JSON.stringify(value));await writeFile(join(root,'managed-base-fresh-boot.py'),'tampered');await assert.rejects(verifyBundle(root),/Bundle(Size|Integrity)Mismatch/);
 }finally{await rm(root,{recursive:true});}
});
