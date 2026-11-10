import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {mkdtemp,writeFile,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
import {publicationPlan,publish,composeTemplate,buildWithDurableIdentity} from '../build-e2b-managed-base.mjs';
const sha=value=>createHash('sha256').update(value).digest('hex');
async function fixture(){
 const root=await mkdtemp(join(tmpdir(),'zuse-e2b-publisher-'));const paths=['runtime','claude','codex','grok','harness-manifest.json','harness-release.py','launch-runtime.sh','launch-recovery.sh','managed-base-install.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py','prepare-plan.json','codex-assets.tar'];
 const files=[];for(const path of paths){const value=path==='harness-release.py'?'# Synthetic archive validation fixture only.\npass\n':path==='managed-base-install.sh'?'# all|layout|prepare layout-receipt.json':'synthetic '+path;const bytes=Buffer.from(value);await writeFile(join(root,path),bytes);files.push({path,bytes:bytes.length,sha256:sha(bytes)});}
 await writeFile(join(root,'bundle.json'),JSON.stringify({schema:1,status:'candidate-only',releaseId:'zuse-base-'+'a'.repeat(32),harnessRelease:'b'.repeat(64),runtimeSha256:files[0].sha256,files}));return{bundle:root,baseImage:'ubuntu@sha256:'+'a'.repeat(64),output:join(root,'receipt.json')};
}
function mock(mode,args){
 const calls=[];let name,operation,removed=false,templateRemoved=false;
 const runtime=sha('synthetic runtime'),cli={version:'gh version 2.80.0',sha256:'a'.repeat(64),wrapperSha256:'b'.repeat(64),loginShellResolvedManaged:true};
 const manifest={managedBase:true,credentialsRemoved:true,repositories:[],rustRuntimeSupervisorVersion:1,rustCommandAdmissionVersion:2,commandAdmission:{runtimeSha256:runtime},rustGithubCliVersion:1,githubCli:cli};
 const info=()=>({templateID:'template1',buildID:mode==='failed-build'?'':'build1',builds:[{buildID:'build1'}],buildCount:mode==='retagged'?2:1,buildStatus:'ready',public:false,names:[name]});
 return{calls,build:async(plan,context,n)=>{calls.push('build');name=n;const receipt=JSON.parse(await readFile(args.output));assert.equal(receipt.templateSubmitted,true);assert.equal(receipt.templateId,undefined);if(mode==='lost-build')throw Error('socket lost');return{templateId:'template1',buildId:'build1'};},buildStatus:async()=>({templateID:'template1',buildID:'build1',status:mode==='failed-build'?'error':'ready'}),templateInfo:async()=>templateRemoved?null:info(),
 create:async(id,op)=>{calls.push('create');operation=op;assert.equal(JSON.parse(await readFile(args.output)).sandboxSubmitted,true);if(mode==='lost-create')throw Error('socket lost');return{id:'sandbox1'};},inspect:async()=>({sandboxID:'sandbox1',templateID:'template1',metadata:{'zuse-operation-id':mode==='foreign'?'foreign':operation}}),
 exec:async(id,cmd)=>{calls.push('exec');if(cmd.includes(' ready '))return JSON.stringify({status:mode==='not-ready'?'invalid':'resident-ready',certified:false,bootSealVerified:true,bootSha256:sha(await readFile(new URL('../e2b-base-boot.py',import.meta.url)))});if(cmd.startsWith('sha256sum'))return runtime+' file';if(cmd.startsWith('cat '))return JSON.stringify(manifest);return JSON.stringify({version:1,required:true,status:'passed',accountRequired:false,modelCalled:false,systemReceipt:cli,globalShim:{contentMatches:true,sha256:cli.wrapperSha256},loginShellMatchesGlobal:true,globalVersionMatchesReceipt:true,canonicalPath:'/opt/zuse/bin/gh',ancestorsControlled:true,profileMatches:true});},
 remove:async()=>{calls.push('remove');removed=true;},absent:async()=>removed,removeTemplate:async()=>{calls.push('removeTemplate');templateRemoved=true;},templateAbsent:async()=>templateRemoved};
}
for(const mode of ['success','retain','lost-build','lost-create','foreign','failed-build','retagged','not-ready'])test('E2B bounded publication '+mode,async()=>{
 const args=await fixture();args.retainArtifact=mode==='retain';const adapter=mock(mode,args);
 try{const result=await publish(args,adapter);assert.equal(adapter.calls.filter(x=>x==='build').length,1);
 assert.ok(adapter.calls.filter(x=>x==='create').length<=1);assert.equal(result.certification,false);
 if(['success','retain'].includes(mode)){assert.equal(result.status,'prepared-uncertified');assert.equal(result.cleanupVerified,true);assert.equal(result.artifactRetained,mode==='retain');assert.equal(result.providerArtifactBinding.artifactVersion,'build1');assert.equal(result.providerArtifactBinding.mode,'private-single-build');assert.match(result.providerArtifactBinding.manifestSha256,/^[a-f0-9]{64}$/);assert.equal(adapter.calls.includes('remove'),true);}
 if(['lost-build','lost-create','foreign','retagged'].includes(mode))assert.equal(result.status,'cleanup-required');
 if(['lost-create','foreign'].includes(mode))assert.equal(adapter.calls.includes('remove'),false,'never delete unverified sandbox');
 if(['failed-build','not-ready'].includes(mode)){assert.equal(result.status,'failed');assert.equal(result.cleanupVerified,true);}
 assert.equal((await readFile(args.output,'utf8')).includes('socket lost'),false);
 await assert.rejects(publish(args,adapter));assert.equal(adapter.calls.filter(x=>x==='build').length,1,'persisted one-shot intent prevents retries');
 }finally{await rm(args.bundle,{recursive:true});}
});
test('plan rejects mutable images and never invokes provider',async()=>{const args=await fixture();try{await assert.rejects(publicationPlan({...args,baseImage:'ubuntu:latest'}),/Immutable/);const plan=await publicationPlan(args);assert.equal(plan.maxTemplateBuilds,1);assert.equal(plan.modelCredentials,false);assert.equal(plan.registration,false);}finally{await rm(args.bundle,{recursive:true});}});
test('template stages layout and preparation before foreground read-only readiness',()=>{
 const calls=[];const t={};for(const method of ['fromImage','setUser','runCmd','copy','setStartCmd'])t[method]=(...args)=>{calls.push([method,...args]);return t;};
 composeTemplate({baseImage:'pinned',files:[{path:'runtime'}],bundleSha256:'a'.repeat(64),runtimeSha256:'b'.repeat(64),supervisorSha256:'c'.repeat(64),harnessRelease:'d'.repeat(64),startCmd:'exec foreground',helperBase64:Buffer.from('helper').toString('base64'),helperSha256:sha('helper'),bootBase64:Buffer.from('boot').toString('base64'),bootSha256:sha('boot')}, {Template:()=>t},'/private');
 const start=calls.find(c=>c[0]==='setStartCmd');assert.equal(start[1],'exec foreground');assert.match(start[2],/ ready --provider e2b/);assert.doesNotMatch(start[2],/prepare-image| layout| prepare /);
 assert.ok(calls.some(c=>c[0]==='runCmd'&&c[1].endsWith(' layout')));assert.ok(calls.some(c=>c[0]==='runCmd'&&c[1].includes(' prepare --provider e2b')));assert.deepEqual(calls.filter(c=>c[0]==='setUser').map(c=>c[1]),['root']);
});
test('explicit pinned SDK derivation composes supported instructions offline',{skip:!process.env.ZUSE_E2B_SDK_TEST_MODULE},async()=>{
 const {pathToFileURL}=await import('node:url');const {E2B_SDK_MODULE_SHA256}=await import('../build-e2b-managed-base.mjs');
 const modulePath=process.env.ZUSE_E2B_SDK_TEST_MODULE;assert.equal(sha(await readFile(modulePath)),E2B_SDK_MODULE_SHA256);
 const sdk=await import(pathToFileURL(modulePath));const original=globalThis.fetch;globalThis.fetch=()=>{throw Error('NetworkForbiddenInOfflineSdkTest');};
 try{const template=composeTemplate({baseImage:'ubuntu@sha256:'+'a'.repeat(64),files:[{path:'runtime'}],bundleSha256:'b'.repeat(64),runtimeSha256:'c'.repeat(64),supervisorSha256:'d'.repeat(64),harnessRelease:'e'.repeat(64),startCmd:'exec foreground',helperBase64:Buffer.from('helper').toString('base64'),helperSha256:sha('helper'),bootBase64:Buffer.from('boot').toString('base64'),bootSha256:sha('boot')},sdk,'/private');
 assert.equal(template.startCmd,'exec foreground');assert.match(template.readyCmd,/ ready --provider e2b/);assert.equal(template.instructions.filter(i=>i.type==='COPY').length,2);assert.equal(template.instructions.filter(i=>i.type==='RUN').length,6);assert.equal(template.instructions.filter(i=>i.type==='USER').at(-1).args[0],'root');
 }finally{globalThis.fetch=original;}
});
test('read-only uncertainty reconciliation uses original name and operation, never creates',async()=>{
 const {reconcilePublication}=await import('../build-e2b-managed-base.mjs');const operationId='12345678-1234-4234-8234-123456789abc',name='zuse-e2b-stage-'+operationId.replaceAll('-','');
 const record={operationId,name};let reads=0;
 const adapter={findTemplates:async requested=>{reads++;assert.equal(requested,name);return[{templateID:'owned',buildID:'build',public:false,names:['team/'+name]}];},findSandboxes:async requested=>{reads++;assert.equal(requested,operationId);return[{sandboxID:'original',metadata:{'zuse-operation-id':operationId}}];}};
 const result=await reconcilePublication(record,adapter);assert.equal(reads,2);assert.equal(result.mutationsPerformed,false);assert.equal(result.cleanupVerified,false);assert.equal(result.absenceIsPointInTime,true);assert.equal(result.templateId,'owned');
 await assert.rejects(reconcilePublication({...record,templateId:'different'},adapter),/IdentityChanged/);
 await assert.rejects(reconcilePublication({...record,name:'other'},adapter),/IntentInvalid/);
 adapter.findTemplates=async()=>[{templateID:'owned',public:false,names:[name]},{templateID:'other',public:false,names:[name]}];await assert.rejects(reconcilePublication(record,adapter),/Ambiguous/);
});

test('build failure diagnostics are private and durable before original cleanup',async()=>{
 const args=await fixture();const adapter=mock('failed-build',args);const remove=adapter.removeTemplate;
 adapter.buildDiagnostics=async()=>[{message:'PRIVATE_DIAGNOSTIC_SENTINEL'}];
 adapter.removeTemplate=async id=>{const publicReceipt=JSON.parse(await readFile(args.output));assert.equal(publicReceipt.buildDiagnosticPersisted,true);const raw=await readFile(publicReceipt.buildFailure.diagnosticPath);assert.equal(sha(raw),publicReceipt.buildFailure.diagnosticSha256);assert.match(raw.toString(),/PRIVATE_DIAGNOSTIC_SENTINEL/);return remove(id);};
 try{const result=await publish(args,adapter);assert.equal(result.cleanupVerified,true);assert.equal(result.errorCode,'E2bTemplateBuildFailed');const publicReceipt=await readFile(args.output,'utf8');assert.equal(publicReceipt.includes('PRIVATE_DIAGNOSTIC_SENTINEL'),false);const {stat}=await import('node:fs/promises');assert.equal((await stat(result.buildFailure.diagnosticPath)).mode&0o777,0o600);}finally{await rm(args.bundle,{recursive:true});}
});

for(const phase of ['upload-files','trigger-build'])test('pinned SDK persists exact build before '+phase+' failure without replay',async()=>{
 const args=await fixture(),adapter=mock('retain',args),buildId='34567890-1234-4234-8234-123456789abc';let builds=0,actualName;
 const sdk={Template:{buildInBackground:async(template,name,options)=>{
  builds++;actualName=name;assert.equal(options.retries,0);
  options.onBuildLogs({message:`Template created with ID: template1, Build ID: ${buildId}`});
  const saved=JSON.parse(readFileSync(args.output,'utf8'));
  assert.equal(saved.templateId,'template1');assert.equal(saved.buildId,buildId);assert.equal(saved.buildIdentityDurable,true);assert.equal(saved.buildSubmissionStage,'upload-files');
  options.onBuildLogs({message:"Uploaded 'runtime'"});
  if(phase==='trigger-build'){options.onBuildLogs({message:'All file uploads completed'});options.onBuildLogs({message:'Starting building...'});assert.equal(JSON.parse(readFileSync(args.output,'utf8')).buildSubmissionStage,'trigger-build');}
  const error=new Error('PRIVATE_PRESIGNED_URL_AND_TOKEN_SENTINEL');error.statusCode=phase==='upload-files'?503:504;if(phase==='upload-files')error.name='FileUploadError';else error.code='ETIMEDOUT';throw error;
 }}};
 adapter.build=(plan,context,name,hooks)=>buildWithDurableIdentity(sdk,{instructionsWithHashes:async()=>[]},name,{retries:0},hooks);
 adapter.templateInfo=async()=>({templateID:'template1',buildID:buildId,public:false,buildCount:1,names:[actualName]});
 try{
  const record=await publish(args,adapter);
  assert.equal(record.templateId,'template1');assert.equal(record.buildId,buildId);assert.equal(record.errorStage,phase);
  assert.equal(record.errorCode,phase==='upload-files'?'E2bFileUploadFailed':'E2bBuildTriggerUncertain');assert.equal(record.providerHttpStatus,phase==='upload-files'?503:504);
  assert.equal(phase==='upload-files'?record.sdkErrorType:record.systemErrorCode,phase==='upload-files'?'FileUploadError':'ETIMEDOUT');
  assert.equal(record.status,'cleanup-required');assert.equal(record.inspectionRequired,true);assert.equal(record.cleanupVerified,false);
  assert.deepEqual(record.uploadReceipts,[{path:'runtime',sha256:record.runtimeSha256,cached:false}]);
  assert.equal(adapter.calls.includes('create'),false);assert.equal(adapter.calls.includes('removeTemplate'),false,'uncertain original remains available for inspection');
  assert.equal((await readFile(args.output,'utf8')).includes('PRIVATE_PRESIGNED_URL_AND_TOKEN_SENTINEL'),false);
  await assert.rejects(publish(args,adapter));assert.equal(builds,1,'lost response never replays SDK build request');
 }finally{await rm(args.bundle,{recursive:true});}
});
test('SDK journal must be synchronous before upload can proceed',async()=>{
 let calls=0;const sdk={Template:{buildInBackground:async(_,__,options)=>{calls++;options.onBuildLogs({message:'Template created with ID: template1, Build ID: 34567890-1234-4234-8234-123456789abc'});throw Error('upload must not be reached');}}};
 await assert.rejects(buildWithDurableIdentity(sdk,{instructionsWithHashes:async()=>[]},'name',{}, {identity:async()=>{},stage:()=>{}}),/JournalMustBeSynchronous/);assert.equal(calls,1);
});

for(const reason of ['missing-log','journal-throws'])test('SDK identity gate blocks '+reason+' before hashes uploads or trigger',async()=>{
 let hashes=0,uploads=0,triggers=0;const template={instructionsWithHashes:async()=>{hashes++;return[];}};const original=template.instructionsWithHashes;
 const sdk={Template:{buildInBackground:async(t,name,options)=>{
  if(reason==='journal-throws')options.onBuildLogs({message:'Template created with ID: template1, Build ID: 34567890-1234-4234-8234-123456789abc'});
  await t.instructionsWithHashes();uploads++;triggers++;return{templateId:'template1',buildId:'34567890-1234-4234-8234-123456789abc'};
 }}};
 await assert.rejects(buildWithDurableIdentity(sdk,template,'name',{}, {identity:()=>{throw Error('fsync failure; not a durable receipt');},stage:()=>{}}),reason==='missing-log'?/IdentityRequired/:/BuildRequestUncertain/);
 assert.equal(hashes,0);assert.equal(uploads,0);assert.equal(triggers,0);assert.equal(template.instructionsWithHashes,original,'instance wrapper is restored');
});

test('cross-filesystem output uses verified original bundle without linking or deleting it',async()=>{
 const {cp,link,stat}=await import('node:fs/promises');const original=await fixture();const root=await mkdtemp('/dev/shm/zuse-e2b-context-');const bundle=join(root,'bundle');
 try{
  await cp(original.bundle,bundle,{recursive:true});assert.notEqual((await stat(bundle)).dev,(await stat(original.bundle)).dev,'fixture must use actual different filesystems');
  await assert.rejects(link(join(bundle,'runtime'),join(original.bundle,'cross-device-probe')),error=>error.code==='EXDEV');
  const args={...original,bundle};const adapter=mock('retain',args),build=adapter.build;adapter.build=async(plan,context,...rest)=>{assert.equal(context,bundle,'SDK reads verified original bundle directly');return build(plan,context,...rest);};
  const result=await publish({...args,retainArtifact:true},adapter);assert.equal(result.status,'prepared-uncertified');assert.equal(result.cleanupVerified,true);assert.equal((await readFile(join(bundle,'runtime'),'utf8')),'synthetic runtime');assert.equal(adapter.calls.filter(x=>x==='build').length,1);
 }finally{await rm(root,{recursive:true});await rm(original.bundle,{recursive:true});}
});

test('embedded pinned helpers install exact bytes and COPY excludes unrelated context files',async()=>{
 const {execFile}=await import('node:child_process');const {promisify}=await import('node:util');const {stat}=await import('node:fs/promises');const root=await mkdtemp(join(tmpdir(),'zuse-e2b-inline-helper-'));
 try{
  const calls=[],t={};for(const method of ['fromImage','setUser','runCmd','copy','setStartCmd'])t[method]=(...args)=>{calls.push([method,...args]);return t;};
  const helperBytes=Buffer.from('# quoted \' value\nprint("λ")\n'),bootBytes=Buffer.from('# boot\n');
  const plan={baseImage:'pinned',files:[{path:'runtime'}],bundleSha256:'a'.repeat(64),runtimeSha256:'b'.repeat(64),supervisorSha256:'c'.repeat(64),harnessRelease:'d'.repeat(64),startCmd:'exec foreground',helperBase64:helperBytes.toString('base64'),helperSha256:sha(helperBytes),bootBase64:bootBytes.toString('base64'),bootSha256:sha(bootBytes)};
  composeTemplate(plan,{Template:()=>t},root);assert.deepEqual(calls.filter(c=>c[0]==='copy').map(c=>c[1]),['runtime','bundle.json']);
  for(const [source,name,expected]of [['/opt/zuse/base/managed-base-preparation.py','helper',helperBytes],['/opt/zuse/base/e2b-base-boot.py','boot',bootBytes]]){
   const command=calls.find(c=>c[0]==='runCmd'&&c[1].includes('base64 --decode')&&c[1].includes(source))[1];const target=join(root,name);await promisify(execFile)('/bin/sh',['-c',command.replaceAll(source,target)]);assert.deepEqual(await readFile(target),expected);assert.equal((await stat(target)).mode&0o777,0o555);
  }
  assert.throws(()=>composeTemplate({...plan,helperSha256:'f'.repeat(64)},{Template:()=>t},root),/EmbeddedHelperInvalid/);
 }finally{await rm(root,{recursive:true});}
});

test('direct bundle context rejects dockerignore before any provider operation',async()=>{
 const args=await fixture();try{await writeFile(join(args.bundle,'.dockerignore'),'runtime\n');const adapter=mock('retain',args);const result=await publish(args,adapter);assert.equal(result.errorCode,'E2bBundleContextIgnoreUnsupported');assert.equal(result.templateSubmitted,false);assert.equal(result.cleanupVerified,true);assert.equal(adapter.calls.includes('build'),false);}finally{await rm(args.bundle,{recursive:true});}
});
