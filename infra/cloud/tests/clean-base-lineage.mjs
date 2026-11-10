import{cleanSupervisorForSnapshot,quiesceCleanParent}from'./hygiene-fixture.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,rm,readFile} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {build,verifyBundle} from '../build-managed-base.mjs';
import {loadCleanLineage} from '../clean-base-lineage.mjs';
const run=promisify(execFile);
async function fixture(){const root=await mkdtemp(resolve('.context/clean-lineage-node-'));const {stdout}=await run('python3',['infra/cloud/tests/managed-base-derive.py','--emit-fixture',root]);return{root,...JSON.parse(stdout)};}
for(const lost of [false,true])test('derived builder preserves original snapshot and exact allocation identity '+lost,async()=>{
 const f=await fixture();try{
  const bundle=await verifyBundle(f.input),parent=await loadCleanLineage(join(f.input,'derivation.json'),f.expected,bundle);let active=false,creates=0,deletes=0,capture,deleted=[];const uploads=[];let factsCommand,originalName,parentPings=0;
  const adapter={cleanSupervisorForSnapshot,quiesceCleanParent,create:async()=>{throw new Error('MustNotColdAllocate');},
   createFromCleanSnapshot:async(name,artifact)=>{creates++;originalName=name;assert.equal(artifact,parent.artifactId);active=true;if(lost)throw new Error('UncertainOriginalCreate');return'original-derived-builder';},
   identity:async id=>({id,name:originalName,hydrated:true,state:'ready'}),
   find:async()=>active?'original-derived-builder':undefined,
   snapshotState:async name=>name===parent.artifactId?{name,status:'ready',snapshotId:'immutable-snapshot-id'}:{name,status:'ready',snapshotId:'new-child-id'},
   ready:async()=>true,
   exec:async(id,cmd)=>cmd.includes('--parent-preflight')?JSON.stringify({version:1,status:cmd.endsWith(" --quiescence-image'")?'quiescence-image-verified':'passed',parentArtifactId:parent.artifactId,parentLineageSha256:f.expected,runtimeSha256:bundle.runtimeSha256,harnessRelease:bundle.harnessRelease,parentRuntimeSha256:parent.parentRuntimeSha256,completeCodexAssetsVerified:true,sourceClean:true}):cmd.includes('managed_base_ancestor_hardening_rejected')?JSON.stringify({version:2,recursive:false,paths:['/opt','/usr/local','/usr/local/bin','/usr/local/lib'].map(path=>({path,recursive:false,after:{uid:0,gid:0,mode:'0o755'}}))}):cmd.includes('os.lstat')?(factsCommand=cmd,'{}'):cmd==='id -u'?'1000':cmd==='sudo -n id -u'?'0':cmd.endsWith('runtime-supervisor.py ping')?(parentPings++,JSON.stringify({version:1,bound:false,childAlive:false,recoveryAlive:false})):cmd.includes('cat /opt/zuse/preparation/status/')?JSON.stringify({managedBase:true,credentialsRemoved:true,repositories:[]}):cmd.includes('sha256sum /usr/local/bin/')?bundle.runtimeSha256+'  /usr/local/bin/zuse-cloud-runtime\n':'',
   upload:async(id,path,bytes)=>{assert.equal(id,'original-derived-builder');uploads.push(path);return bytes.length;},
   prepare:async(id,command,save)=>{assert.ok(command.includes('ZUSE_CLEAN_DERIVATION_SHA256=')&&command.includes(f.expected));await save({mode:'synthetic-tracked-operation',machineId:id,processId:11});},
   snapshot:async(id,name)=>{capture=name;},remove:async id=>{deletes++;active=false;},absent:async()=>!active,
   removeSnapshot:async name=>{deleted.push(name);},snapshotAbsent:async()=>true,close:()=>{},
  };
  const report=await build({provider:'boat',bundle:f.input,output:join(f.root,'report.json'),retainArtifact:false,deriveLineage:join(f.input,'derivation.json'),deriveLineageSha256:f.expected},adapter);
  assert.equal(creates,1);assert.equal(deletes,1);assert.equal(report.cleanupVerified,true);assert.equal(parentPings,0,'a copied stale pathname socket is not an admission dependency');
  assert.equal(report.cleanParent.artifactId,parent.artifactId);assert.ok(!deleted.includes(parent.artifactId),'parent artifact never deleted');
  if(lost){assert.equal(report.status,'failed');assert.equal(uploads.length,0);assert.equal(capture,undefined);}
  else{assert.equal(report.status,'prepared-uncertified',JSON.stringify({failedStage:report.failedStage,errorCode:report.errorCode,parent:report.parentSupervisorHygiene}));assert.ok(uploads.some(p=>p.includes('runtime.part-')));assert.ok(uploads.some(p=>p.endsWith('derivation.json')));for(const name of ['claude','codex','grok','codex-assets.tar'])assert.ok(!uploads.some(p=>p.includes('/'+name+'.part-')));assert.equal(report.uploadedBytes,bundle.files.filter(f=>!['claude','codex','grok','codex-assets.tar','harness-release.py','harness-manifest.json'].includes(f.path)).reduce((n,f)=>n+f.bytes,0)+report.preflightUploadedBytes);assert.deepEqual(deleted,[capture]);const parsed=await run('python3',['-c','import shlex,json,sys;print(json.dumps(shlex.split(sys.argv[1])))',factsCommand]);const argv=JSON.parse(parsed.stdout);const facts=JSON.parse((await run('sh',['-c',argv.at(-1)])).stdout);assert.ok(facts['/opt']);}
 }finally{await rm(f.root,{recursive:true});}
});
test('source artifact replacement or missing derivation installer fails before allocation',async()=>{
 const f=await fixture();try{
  let effects=0;const args={provider:'boat',bundle:f.input,output:join(f.root,'report.json'),deriveLineage:join(f.input,'derivation.json'),deriveLineageSha256:f.expected};
  await assert.rejects(build(args,{createFromCleanSnapshot:()=>{effects++;},snapshotState:async()=>({status:'ready',name:'different',snapshotId:'replacement'})}),/CleanParentArtifactMismatch/);assert.equal(effects,0);
  const bundle=await verifyBundle(f.input);delete bundle.managedBaseDerivationVersion;
  await assert.rejects(loadCleanLineage(args.deriveLineage,f.expected,bundle),/DerivationInstallerRequired/);
  await assert.rejects(loadCleanLineage(args.deriveLineage,'0'.repeat(64),await verifyBundle(f.input)),/DigestMismatch/);
 }finally{await rm(f.root,{recursive:true});}
});
test('local derivation plan preserves captured bundle and hashes every overlay',async()=>{
 const f=await fixture();try{
  const {prepareCleanDerivation}=await import('../prepare-clean-derivation.mjs');
  const before=await readFile(join(f.input,'bundle.json'));
  const plan=await prepareCleanDerivation({source:f.input,output:join(f.root,'derived'),proof:join(f.root,'proof.json'),builder:join(f.root,'builder.json'),parentBundle:join(f.root,'parentBundle.json'),cleanup:join(f.root,'cleanup.json')});
  assert.deepEqual(await readFile(join(f.input,'bundle.json')),before);
  assert.equal(plan.providerAllocationAuthorized,false);assert.equal(plan.agentAcceptanceVerified,false);
  assert.equal(plan.runtimeSha256,JSON.parse(before).runtimeSha256);assert.ok(plan.reusedHarnessBytes>0);assert.ok(plan.plannedUploadBytes<plan.fullBundleBytes+120000);
  assert.equal((await verifyBundle(join(f.root,'derived'))).files.length,15);
 }finally{await rm(f.root,{recursive:true});}
});
for(const mode of ['unknown-ancestor','known-repair','preflight-only','quiescence-only'])test('strict small parent gate prevents premature payload '+mode,async()=>{
 const f=await fixture();try{
  const bundle=await verifyBundle(f.input),parent=await loadCleanLineage(join(f.input,'derivation.json'),f.expected,bundle);let active=false,checks=0,repairs=0,captures=0,deletes=0,originalName;const uploaded=[];
  const passed={version:1,status:'passed',parentArtifactId:parent.artifactId,parentLineageSha256:f.expected,runtimeSha256:bundle.runtimeSha256,harnessRelease:bundle.harnessRelease,parentRuntimeSha256:parent.parentRuntimeSha256,completeCodexAssetsVerified:true,sourceClean:true};
  const failures=['/opt','/usr/local','/usr/local/bin',...(mode==='unknown-ancestor'?['/usr/local/share']:[])].map(path=>({path,uid:1000,gid:1000,mode:'0o755',symlink:false}));
  const adapter={cleanSupervisorForSnapshot,quiesceCleanParent,createFromCleanSnapshot:async(n,a,o)=>{originalName=n;assert.equal(a,parent.artifactId);assert.equal(o.ttlSeconds,['preflight-only','quiescence-only'].includes(mode)?600:1800);active=true;return'original';},ready:async()=>true,
   identity:async id=>({id,name:originalName,hydrated:true,state:'ready'}),
   snapshotState:async name=>({name,status:'ready',snapshotId:name===parent.artifactId?'immutable-snapshot-id':'child'}),
   exec:async(id,cmd)=>{
    if(cmd==='id -u')return'1000';if(cmd==='sudo -n id -u')return'0';
    if(cmd.endsWith('runtime-supervisor.py ping'))return JSON.stringify({version:1,bound:false,childAlive:false,recoveryAlive:false});
    if(cmd.includes('--parent-preflight')){checks++;return JSON.stringify(repairs?{...passed,status:cmd.endsWith(" --quiescence-image'")?'quiescence-image-verified':'passed'}:{version:1,status:'rejected',code:'parent_metadata_not_protected',failures});}
    if(cmd.includes('managed_base_ancestor_hardening_rejected')){repairs++;return JSON.stringify({version:2,recursive:false,paths:['/opt','/usr/local','/usr/local/bin','/usr/local/lib'].map(path=>({path,recursive:false,after:{uid:0,gid:0,mode:'0o755'}}))});}
    if(cmd.includes('os.lstat'))return'{}';
    if(cmd.includes('cat /opt/zuse/preparation/status/'))return JSON.stringify({managedBase:true,credentialsRemoved:true,repositories:[]});
    if(cmd.includes('sha256sum /usr/local/bin/'))return bundle.runtimeSha256+'  runtime\n';return'';
   },upload:async(id,path,data)=>{if(path.startsWith('/tmp/zuse-base-input/'))assert.equal(checks,3,'no heavy payload before confirmed strict parent admission');uploaded.push(path);return data.length;},
   prepare:async()=>{},snapshot:async()=>{captures++;},remove:async()=>{deletes++;active=false;},absent:async()=>!active,removeSnapshot:async()=>{},snapshotAbsent:async()=>true,close:()=>{},
  };
  const report=await build({provider:'boat',bundle:f.input,output:join(f.root,'strict-report.json'),retainArtifact:false,deriveLineage:join(f.input,'derivation.json'),deriveLineageSha256:f.expected,parentPreflightOnly:mode==='preflight-only',parentQuiescenceOnly:mode==='quiescence-only'},adapter);
  assert.equal(deletes,1);assert.equal(report.cleanupVerified,true);assert.ok(report.preflightUploadedBytes>0);
  if(mode==='known-repair'){assert.equal(report.status,'prepared-uncertified',JSON.stringify({failedStage:report.failedStage,errorCode:report.errorCode,parent:report.parentSupervisorHygiene}));assert.equal(repairs,1);assert.equal(checks,3);assert.equal(captures,1);assert.ok(uploaded.some(p=>p.includes('/runtime.part-')));}
  else if(mode==='quiescence-only'){assert.equal(report.status,'parent-quiescence-verified',JSON.stringify(report));assert.equal(repairs,1);assert.equal(checks,3);assert.equal(captures,0);assert.equal(report.artifactRetained,false);assert.equal(report.parentQuiescenceOnly,true);assert.equal(report.uploadedBytes,report.preflightUploadedBytes);assert.ok(uploaded.every(p=>p.startsWith('/tmp/zuse-parent-preflight/')));assert.ok(!report.stages.some(s=>['upload','install-and-sanitize','capture'].includes(s.name)));}
  else{assert.equal(repairs,0);assert.equal(captures,0);assert.equal(report.uploadedBytes,report.preflightUploadedBytes);assert.ok(uploaded.every(p=>p.startsWith('/tmp/zuse-parent-preflight/')));assert.equal(report.status,mode==='preflight-only'?'parent-preflight-rejected':'failed');if(mode==='unknown-ancestor')assert.equal(report.errorCode,'ParentPreflightRejected');}
 }finally{await rm(f.root,{recursive:true});}
});
