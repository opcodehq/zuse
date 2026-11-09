#!/usr/bin/env node
// Operator-only E2B template publisher. Default invocation is a read-only plan.
import {createHash,randomUUID} from 'node:crypto';
import {readFile,mkdir,open,rename,lstat} from 'node:fs/promises';
import {openSync,writeFileSync,fsyncSync,closeSync,renameSync} from 'node:fs';
import {dirname,join,resolve} from 'node:path';
import {pathToFileURL,fileURLToPath} from 'node:url';
import {verifyBundle,requireFreshPublicationReport} from './build-managed-base.mjs';
import {bindStreamingHashes} from './e2b-streaming-hash.mjs';
import {githubCliProof,requireGithubCliProof} from './github-cli-proof.mjs';
export const E2B_SDK_VERSION='2.62.0';
export const E2B_SDK_UPSTREAM_MODULE_SHA256='1c96b58fb06dd23f45d869a20800356b8712e33ff6c7b3bd04034e2fac112eaa';
export const E2B_SDK_DERIVATION='zuse-sequential-upload-v1';
export const E2B_SDK_MODULE_SHA256='b69736a75bfdc0512683b55b38b971e4294f2382c2d2e820d819a8f3ad2242f9';
export const E2B_SDK_INTEGRITY='sha512-RKYzGLgFLJa9xCct6Bh255UbERo/yaYqG/3sUqau60sZ38TFZSAt2M+eXGfKWo3yD31iu9iNVC44H0x9ppaoJw==';
const sha=value=>createHash('sha256').update(value).digest('hex');
const digest=/^[a-f0-9]{64}$/;
const identity=/^[a-zA-Z0-9_-]{1,128}$/;
const helperPath=fileURLToPath(new URL('./managed-base-preparation.py',import.meta.url));
const pins=plan=>`--provider e2b --bundle-sha256 ${plan.bundleSha256} --runtime-sha256 ${plan.runtimeSha256} --supervisor-sha256 ${plan.supervisorSha256} --harness-release ${plan.harnessRelease}`;
const helper='/opt/zuse/base/managed-base-preparation.py';
const boot='/opt/zuse/base/e2b-base-boot.py';
const bootPath=fileURLToPath(new URL('./e2b-base-boot.py',import.meta.url));
export async function publicationPlan(args){
  if(!/^[a-z0-9./:_-]+@sha256:[a-f0-9]{64}$/.test(args.baseImage??''))throw new Error('E2bImmutableBaseImageRequired');
  const bundle=await verifyBundle(args.bundle);
  if(bundle.managedBaseDerivationVersion!==undefined)throw new Error('E2bDerivationNotCertified');
  if(!bundle.files.some(file=>file.path==='codex-assets.tar'))throw new Error('E2bCompleteNativeDistributionRequired');
  const supervisorSha256=bundle.files.find(file=>file.path==='runtime-supervisor.py')?.sha256;
  if(!digest.test(supervisorSha256??''))throw new Error('E2bSupervisorRequired');
  const installer=await readFile(join(args.bundle,'managed-base-install.sh'),'utf8');
  if(!installer.includes('all|layout|prepare')||!installer.includes('layout-receipt.json'))throw new Error('E2bInstallerStagesRequired');
  const helperSource=await readFile(helperPath),bootSource=await readFile(bootPath);
  const plan={schema:1,status:'plan-only',provider:'e2b',releaseId:bundle.releaseId,
    baseImage:args.baseImage,runtimeSha256:bundle.runtimeSha256,harnessRelease:bundle.harnessRelease,
    supervisorSha256,bundleSha256:sha(await readFile(join(args.bundle,'bundle.json'))),
    helperSha256:sha(helperSource),bootSha256:sha(bootSource),helperBase64:helperSource.toString('base64'),bootBase64:bootSource.toString('base64'),streamingHashSha256:sha(await readFile(new URL('./e2b-streaming-hash.mjs',import.meta.url))),builderSha256:sha(await readFile(new URL(import.meta.url))),sdkVersion:E2B_SDK_VERSION,sdkDerivation:E2B_SDK_DERIVATION,sdkUpstreamModuleSha256:E2B_SDK_UPSTREAM_MODULE_SHA256,sdkModuleSha256:E2B_SDK_MODULE_SHA256,sdkUpstreamIntegrity:E2B_SDK_INTEGRITY,
    maxTemplateBuilds:1,maxConcurrentProofSandboxes:1,proofSandboxes:1,
    buildDeadlineSeconds:900,proofSandboxTtlSeconds:600,retainArtifact:args.retainArtifact===true,
    modelCredentials:false,registration:false,certification:false,providerKeysInTemplate:false,
    artifactBindingPolicy:'private-single-build',atomicVersionSelection:false,
    uploadBytes:bundle.files.reduce((n,file)=>n+file.bytes,0)+(await readFile(join(args.bundle,'bundle.json'))).length,embeddedHelperBytes:helperSource.length+bootSource.length,
    files:bundle.files.map(file=>({...file}))};
  plan.startCmd=`exec /usr/bin/python3 -I -S -B ${boot} boot ${pins(plan)}`;
  return plan;
}
function embeddedScript(encoded,pin,path){
  if(typeof encoded!=='string'||!(/^[A-Za-z0-9+/]+={0,2}$/).test(encoded)||!digest.test(pin??''))throw new Error('E2bEmbeddedHelperInvalid');
  const bytes=Buffer.from(encoded,'base64');if(bytes.length>65536||bytes.toString('base64')!==encoded||sha(bytes)!==pin)throw new Error('E2bEmbeddedHelperInvalid');
  return `printf '%s' '${encoded}' | /usr/bin/base64 --decode > ${path} && /usr/bin/chmod 0555 ${path} && printf '%s\\n' '${pin}  ${path}' | /usr/bin/sha256sum --check --status`;
}
export function composeTemplate(plan,sdk,context){
  const template=sdk.Template({fileContextPath:context}).fromImage(plan.baseImage).setUser('root')
    .runCmd('/usr/bin/install -d -m 0755 /opt/zuse-base-input /opt/zuse/base /usr/local/lib/zuse',{user:'root'});
  for(const name of [...plan.files.map(file=>file.path),'bundle.json'])template.copy(name,`/opt/zuse-base-input/${name}`,{user:'root',mode:0o444,resolveSymlinks:false});
  template.runCmd(embeddedScript(plan.helperBase64,plan.helperSha256,helper),{user:'root'});
  template.runCmd(embeddedScript(plan.bootBase64,plan.bootSha256,boot),{user:'root'});
  template.runCmd('/bin/bash /opt/zuse-base-input/managed-base-install.sh /opt/zuse-base-input layout',{user:'root'});
  template.runCmd(`/usr/bin/python3 -I -S -B ${helper} prepare ${pins(plan)}`,{user:'root'});
  template.runCmd(`/usr/bin/python3 -I -S -B ${boot} seal ${pins(plan)}`,{user:'root'});
  return template.setStartCmd(plan.startCmd,`/usr/bin/python3 -I -S -B ${boot} ready ${pins(plan)}`);
}
async function durable(path,value,first=false){
  await mkdir(dirname(path),{recursive:true,mode:0o700});
  const bytes=Buffer.from(JSON.stringify(value,null,2)+'\n');
  if(first){const out=await open(path,'wx',0o600);try{await out.writeFile(bytes);await out.sync();}finally{await out.close();}}
  else{const temporary=path+'.'+randomUUID()+'.tmp';const out=await open(temporary,'wx',0o600);try{await out.writeFile(bytes);await out.sync();}finally{await out.close();}await rename(temporary,path);}
  const dir=await open(dirname(path),'r');try{await dir.sync();}finally{await dir.close();}
}
// SDK 2.62.0 calls onBuildLogs synchronously immediately after requestBuild.
// Its callback is NOT awaited: an async receipt would race the following uploads.
function durableSync(path,value){
  const temporary=path+'.'+randomUUID()+'.tmp';const fd=openSync(temporary,'wx',0o600);
  try{writeFileSync(fd,JSON.stringify(value,null,2)+'\n');fsyncSync(fd);}finally{closeSync(fd);}
  renameSync(temporary,path);const dir=openSync(dirname(path),'r');try{fsyncSync(dir);}finally{closeSync(dir);}
}
export async function buildWithDurableIdentity(sdk,template,name,options,hooks){
  if(typeof hooks?.identity!=='function'||typeof hooks?.stage!=='function')throw new Error('E2bBuildJournalRequired');
  const originalHashes=template.instructionsWithHashes;
  if(typeof originalHashes!=='function')throw new Error('E2bBuildIdentityGateRequired');
  let phase='request-build',captured;
  // The pinned SDK calls this unconditionally after its identity callback.
  // Missing/changed logs must fail BEFORE any COPY upload or build trigger.
  template.instructionsWithHashes=async function(...args){if(!captured)throw new Error('E2bBuildIdentityRequired');return originalHashes.apply(this,args);};
  const journal=(callback,value)=>{const result=callback(value);if(result&&typeof result.then==='function')throw new Error('E2bBuildJournalMustBeSynchronous');};
  const move=value=>{phase=value;journal(hooks.stage,value);};
  try{
    move(phase);
    const result=await sdk.Template.buildInBackground(template,name,{...options,onBuildLogs:event=>{
      const message=event?.message;if(typeof message!=='string')return;
      const ids=/^Template created with ID: ([A-Za-z0-9_-]{1,128}), Build ID: ([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})$/.exec(message);
      if(ids){
        if(ids[2]==='00000000-0000-0000-0000-000000000000')throw new Error('E2bBuildReceiptInvalid');
        const value={templateId:ids[1],buildId:ids[2]};
        if(captured&&(captured.templateId!==value.templateId||captured.buildId!==value.buildId))throw new Error('E2bBuildIdentityChanged');
        journal(hooks.identity,value);captured=value;move('upload-files');return;
      }
      if(message==='All file uploads completed'){if(!captured)throw new Error('E2bBuildIdentityRequired');move('uploads-complete');return;}
      if(message==='Starting building...'){if(!captured)throw new Error('E2bBuildIdentityRequired');move('trigger-build');return;}
      const uploaded=/^(Uploaded|Skipping upload of) '([A-Za-z0-9_.-]{1,128})'(?:, already cached)?$/.exec(message);
      if(uploaded&&captured&&hooks.upload)journal(hooks.upload,{path:uploaded[2],cached:uploaded[1]==='Skipping upload of'});
    }});
    if(!captured||result.templateId!==captured.templateId||result.buildId!==captured.buildId)throw new Error('E2bBuildIdentityChanged');
    move('trigger-confirmed');return result;
  }catch(error){
    const code=/^E2b[A-Za-z]+$/.test(error.message??'')?error.message:phase==='upload-files'?'E2bFileUploadFailed':phase==='trigger-build'?'E2bBuildTriggerUncertain':'E2bBuildRequestUncertain';
    const safe=new Error(code);safe.e2bStage=phase;safe.inspectOriginal=true;
    const type=[error.name,error.constructor?.name].find(value=>['FileUploadError','BuildError','TimeoutError','AbortError'].includes(value));if(type)safe.sdkErrorType=type;
    const systemCode=error.code??error.cause?.code;if(['ECONNRESET','ECONNREFUSED','ETIMEDOUT','ENOSPC','EACCES','EPERM'].includes(systemCode))safe.systemErrorCode=systemCode;
    const status=error.statusCode??error.status;if(Number.isInteger(status)&&status>=400&&status<=599)safe.providerHttpStatus=status;
    throw safe;
  }finally{template.instructionsWithHashes=originalHashes;}
}
async function fileContext(args,plan){
  // COPY sources are the exact literal files in the verified bundle, never a
  // directory/glob. Using it directly avoids cross-filesystem hardlink staging.
  const context=resolve(args.bundle),info=await lstat(context);
  if(info.isSymbolicLink()||!info.isDirectory()||info.uid!==process.getuid()||(info.mode&0o022))throw new Error('E2bBundleContextUnsafe');
  const ignore=await lstat(join(context,'.dockerignore')).catch(error=>{if(error.code==='ENOENT')return null;throw error;});
  if(ignore)throw new Error('E2bBundleContextIgnoreUnsupported');
  const verified=await verifyBundle(context);
  if(sha(await readFile(join(context,'bundle.json')))!==plan.bundleSha256||JSON.stringify(verified.files)!==JSON.stringify(plan.files))throw new Error('E2bBundleSourceChanged');
  return context;
}
function sameTemplate(value,record){
  return value?.templateID===record.templateId&&value.buildID===record.buildId&&value.buildCount===1&&value.buildStatus==='ready'&&value.public===false
    &&[...(value.names??[]),...(value.aliases??[])].some(name=>name===record.name||name.endsWith('/'+record.name));
}
function ownsTemplate(value,record){return value?.templateID===record.templateId&&value.public===false&&value.buildCount===1&&[...(value.names??[]),...(value.aliases??[])].some(name=>name===record.name||name.endsWith('/'+record.name))&&(value.buildID===record.buildId||(value.builds??[]).some(build=>build.buildID===record.buildId));}
function validBuild(value){return identity.test(value?.templateId??'')&&identity.test(value?.buildId??'');}
export async function publish(args,adapter,{clock=()=>Date.now(),sleep=ms=>new Promise(r=>setTimeout(r,ms))}={}){
  const plan=await publicationPlan(args);await requireFreshPublicationReport(args.output);
  const operationId=args.operationId??randomUUID();if(!/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(operationId))throw new Error('E2bOperationIdentityInvalid');const name='zuse-e2b-stage-'+operationId.replaceAll('-','');
  const record={...plan,status:'candidate-only',operationId,name,cleanupVerified:false,artifactRetained:false,templateSubmitted:false,sandboxSubmitted:false};
  const save=()=>durable(args.output,record);await durable(args.output,record,true);
  let context;let sandbox;let ownTemplate=false;let ownSandbox=false;
  const stage=async(name,work)=>{record.activeStage=name;await save();return work();};
  try{
    context=await fileContext(args,plan);
    record.templateSubmitted=true;await save();
    const built=await stage('template-submit',()=>adapter.build(plan,context,name,{
      identity:value=>{if(!validBuild(value))throw new Error('E2bBuildReceiptInvalid');record.templateId=value.templateId;record.buildId=value.buildId;record.buildIdentityDurable=true;durableSync(args.output,record);},
      stage:value=>{record.buildSubmissionStage=value;durableSync(args.output,record);},
      upload:value=>{const file=plan.files.find(file=>file.path===value.path);const hash=file?.sha256??({'bundle.json':plan.bundleSha256,'managed-base-preparation.py':plan.helperSha256,'e2b-base-boot.py':plan.bootSha256})[value.path];if(!hash)throw new Error('E2bBuildUploadIdentityInvalid');record.uploadReceipts??=[];record.uploadReceipts.push({path:value.path,sha256:hash,cached:value.cached});durableSync(args.output,record);}
    }));
    if(!validBuild(built))throw new Error('E2bBuildReceiptInvalid');
    record.templateId=built.templateId;record.buildId=built.buildId;await save();
    const until=clock()+900000;
    await stage('template-ready',async()=>{
      while(clock()<until){const state=await adapter.buildStatus(built);
        if(state.templateID!==record.templateId||state.buildID!==record.buildId)throw new Error('E2bBuildIdentityChanged');
        if(state.status==='ready')return;
        if(state.status==='error'){
          // Persist provider failure evidence before deleting the original build.
          record.buildDiagnosticPersisted=false;await save();
          let logs;try{logs=await adapter.buildDiagnostics?.(built);}catch{logs={available:false};}
          const diagnostic={version:1,templateId:record.templateId,buildId:record.buildId,state,logs:logs??{available:false}};
          const bytes=Buffer.from(JSON.stringify(diagnostic,null,2)+'\n');
          if(bytes.length>524288)throw new Error('E2bBuildDiagnosticTooLarge');
          const path=args.output+'.build-failure.private.json';await durable(path,diagnostic,true);
          record.buildFailure={version:1,stage:'template-ready',code:'provider_build_failed',diagnosticPath:path,diagnosticSha256:sha(bytes),diagnosticBytes:bytes.length};
          record.buildDiagnosticPersisted=true;await save();
          throw new Error('E2bTemplateBuildFailed');
        }
        if(!['building','waiting'].includes(state.status))throw new Error('E2bBuildReceiptInvalid');
        await sleep(1000);
      }throw new Error('E2bTemplateBuildDeadline');
    });
    if(!sameTemplate(await adapter.templateInfo(record.templateId),record))throw new Error('E2bTemplateBindingChanged');ownTemplate=true;
    record.sandboxSubmitted=true;await save();
    sandbox=await stage('fresh-restore',()=>adapter.create(record.templateId,operationId));
    if(!identity.test(sandbox?.id??''))throw new Error('E2bRestoreReceiptInvalid');
    record.machineId=sandbox.id;await save();
    const info=await adapter.inspect(sandbox.id);
    if(info.sandboxID!==sandbox.id||info.templateID!==record.templateId||info.metadata?.['zuse-operation-id']!==operationId)throw new Error('E2bRestoreIdentityChanged');ownSandbox=true;
    if(!sameTemplate(await adapter.templateInfo(record.templateId),record))throw new Error('E2bTemplateBindingChanged');
    const ready=JSON.parse(await adapter.exec(sandbox.id,`/usr/bin/python3 -I -S -B ${boot} ready ${pins(plan)}`));
    if(ready.status!=='resident-ready'||ready.certified!==false||ready.bootSealVerified!==true||ready.bootSha256!==plan.bootSha256)throw new Error('E2bResidentRestoreUnproven');
    const actual=(await adapter.exec(sandbox.id,'sha256sum /usr/local/bin/zuse-cloud-runtime')).trim().split(/\s+/)[0];
    if(actual!==plan.runtimeSha256)throw new Error('E2bRestoredRuntimeMismatch');
    const rawManifest=await adapter.exec(sandbox.id,'cat /opt/zuse/preparation/status/manifest.json');
    record.manifestSha256=sha(Buffer.from(rawManifest));record.manifest=JSON.parse(rawManifest);
    const manifest=record.manifest;
    if(manifest.managedBase!==true||manifest.credentialsRemoved!==true||manifest.repositories?.length!==0||manifest.rustRuntimeSupervisorVersion!==1||manifest.rustCommandAdmissionVersion!==2||manifest.commandAdmission?.runtimeSha256!==plan.runtimeSha256||manifest.rustGithubCliVersion!==1||manifest.githubCli?.loginShellResolvedManaged!==true)throw new Error('E2bRestoredManifestInvalid');
    record.githubCliProof=await githubCliProof(adapter,sandbox.id,manifest);requireGithubCliProof(record.githubCliProof);
    record.residentRestore={status:'resident-ready',runtimeSha256:actual,bootSealVerified:true,bootSha256:ready.bootSha256};
    record.artifactId=record.templateId;record.providerArtifactVersion=record.buildId;
    record.providerArtifactBinding={version:1,mode:'private-single-build',artifactId:record.templateId,artifactVersion:record.buildId,artifactName:record.name,runtimeSha256:actual,manifestSha256:record.manifestSha256};
    record.status='prepared-uncertified';await save();
  }catch(error){record.status='failed';record.errorCode=/^E2b[A-Za-z]+$/.test(error.message??'')?error.message:'E2bPublicationOutcomeUnknown';if(error.e2bStage)record.errorStage=error.e2bStage;if(error.inspectOriginal===true)record.inspectionRequired=true;if(error.providerHttpStatus)record.providerHttpStatus=error.providerHttpStatus;if(error.sdkErrorType)record.sdkErrorType=error.sdkErrorType;if(error.systemErrorCode)record.systemErrorCode=error.systemErrorCode;}
  finally{
    if(record.sandboxSubmitted){
      if(ownSandbox){try{await adapter.remove(record.machineId);if(!await adapter.absent(record.machineId))throw new Error();record.sandboxCleanupVerified=true;}catch{record.sandboxCleanupVerified=false;}}
      else record.sandboxCleanupVerified=false;
    }else record.sandboxCleanupVerified=true;
    if(record.templateSubmitted){
      if(ownTemplate&&record.status==='prepared-uncertified'&&args.retainArtifact){record.artifactRetained=true;record.templateCleanupVerified=true;}
      else if(record.templateId&&!record.inspectionRequired&&record.buildDiagnosticPersisted!==false){try{if(!ownsTemplate(await adapter.templateInfo(record.templateId),record))throw new Error();await adapter.removeTemplate(record.templateId);record.templateCleanupVerified=await adapter.templateAbsent(record.templateId);}catch{record.templateCleanupVerified=false;}}
      else record.templateCleanupVerified=false;
    }else record.templateCleanupVerified=true;
    record.cleanupVerified=record.sandboxCleanupVerified&&record.templateCleanupVerified;
    if(!record.cleanupVerified)record.status='cleanup-required';
    delete record.activeStage;await save();
    // The verified source bundle is shared immutable input; never remove it.
  }
  return record;
}
export async function reconcilePublication(record,adapter){
  if(!/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(record.operationId??'')||record.name!=='zuse-e2b-stage-'+record.operationId.replaceAll('-',''))throw new Error('E2bReconcileIntentInvalid');
  const templates=await adapter.findTemplates(record.name);
  const matches=templates.filter(value=>value.public===false&&[...(value.names??[]),...(value.aliases??[])].some(name=>name===record.name||name.endsWith('/'+record.name)));
  if(matches.length>1)throw new Error('E2bReconcileAmbiguous');
  const template=matches[0];if(template&&!identity.test(template.templateID??''))throw new Error('E2bReconcileIdentityInvalid');
  if(record.templateId&&template?.templateID!==record.templateId)throw new Error('E2bReconcileIdentityChanged');
  const boxes=await adapter.findSandboxes(record.operationId);
  if(boxes.some(box=>!identity.test(box.sandboxID??'')||box.metadata?.['zuse-operation-id']!==record.operationId)||boxes.length>1)throw new Error('E2bReconcileAmbiguous');
  return{version:1,status:'reconciled-read-only',operationId:record.operationId,name:record.name,templateId:template?.templateID??null,buildId:template?.buildID??null,sandboxIds:boxes.map(box=>box.sandboxID),mutationsPerformed:false,absenceIsPointInTime:true,cleanupVerified:false};
}
export async function createE2bAdapter(modulePath){
  const metadata=JSON.parse(await readFile(resolve(dirname(modulePath),'../package.json'),'utf8'));
  if(metadata.name!=='e2b'||metadata.version!==E2B_SDK_VERSION||sha(await readFile(modulePath))!==E2B_SDK_MODULE_SHA256)throw new Error('E2bUnpinnedSdk');
  const sdk=await import(pathToFileURL(resolve(modulePath)).href);
  if(!process.env.E2B_API_KEY)throw new Error('E2bKeyRequired');
  const opts={apiKey:process.env.E2B_API_KEY,retries:0,requestTimeoutMs:45000,debug:false,apiUrl:'https://api.e2b.app',domain:'e2b.app'};
  const api=new sdk.ApiClient(new sdk.ConnectionConfig(opts));const sandboxes=new Map();
  const getTemplate=async id=>{const result=await api.api.GET('/templates/{templateID}',{params:{path:{templateID:id},query:{limit:2}},signal:AbortSignal.timeout(45000)});if(result.response.status===404)return null;if(result.error||!result.data)throw new Error('E2bTemplateInspectFailed');return result.data;};
  return {
    findTemplates:async name=>{const found=[];let nextToken;for(let page=0;page<20;page++){const result=await api.api.GET('/v2/templates',{params:{query:{limit:100,nextToken}},signal:AbortSignal.timeout(45000)});if(result.error||!Array.isArray(result.data))throw new Error('E2bReconcileListingFailed');found.push(...result.data.filter(value=>[...(value.names??[]),...(value.aliases??[])].some(n=>n===name||n.endsWith('/'+name))));nextToken=result.response.headers.get('X-Next-Token');if(!nextToken)return found;}throw new Error('E2bReconcileListingIncomplete');},
    findSandboxes:async operation=>{const result=await api.api.GET('/v2/sandboxes',{params:{query:{metadata:new URLSearchParams({'zuse-operation-id':operation}).toString(),state:['running','paused'],limit:100}},signal:AbortSignal.timeout(45000)});if(result.error||!Array.isArray(result.data)||result.data.length>=100||result.response.headers.get('X-Next-Token'))throw new Error('E2bReconcileListingIncomplete');return result.data;},
    build:async(plan,context,name,hooks)=>{const files=new Map(plan.files.map(file=>[file.path,file]));files.set('bundle.json',{sha256:plan.bundleSha256});const template=await bindStreamingHashes(composeTemplate(plan,sdk,context),context,files);return buildWithDurableIdentity(sdk,template,name,{...opts,cpuCount:2,memoryMB:2048,minFreeDiskMb:4096},hooks);},
    buildStatus:build=>sdk.Template.getBuildStatus(build,opts),templateInfo:getTemplate,
    buildDiagnostics:async build=>{const result=await api.api.GET('/templates/{templateID}/builds/{buildID}/logs',{params:{path:{templateID:build.templateId,buildID:build.buildId},query:{direction:'backward',limit:100}},signal:AbortSignal.timeout(45000)});if(result.error)throw new Error('E2bBuildDiagnosticUnavailable');return result.data;},
    create:async(template,operation)=>{const box=await sdk.Sandbox.create(template,{...opts,timeoutMs:600000,idempotencyKey:operation,envs:{},metadata:{'zuse-operation-id':operation,'zuse-purpose':'managed-base-proof'}});sandboxes.set(box.sandboxId,box);return{id:box.sandboxId};},
    inspect:async id=>{const value=await sdk.Sandbox.getInfo(id,opts);return{sandboxID:value.sandboxId,templateID:value.templateId,metadata:value.metadata};},
    exec:async(id,command)=>{const box=sandboxes.get(id);if(!box)throw new Error('E2bUnknownProofSandbox');const result=await box.commands.run(command,{user:'root',timeoutMs:30000});if(result.exitCode!==0||result.stdout.length>65536)throw new Error('E2bProofCommandFailed');return result.stdout;},
    remove:id=>sdk.Sandbox.kill(id,opts),
    absent:async id=>{try{await sdk.Sandbox.getInfo(id,opts);return false;}catch(error){if(error instanceof sdk.SandboxNotFoundError||error.statusCode===404)return true;throw new Error('E2bSandboxAbsenceUnproven');}},
    removeTemplate:async id=>{const result=await api.api.DELETE('/templates/{templateID}',{params:{path:{templateID:id}},signal:AbortSignal.timeout(45000)});if(result.error&&result.response.status!==404)throw new Error('E2bTemplateDeleteUncertain');},
    templateAbsent:async id=>(await getTemplate(id))===null,
  };
}
async function main(){
  const [major,minor]=process.versions.node.split('.').map(Number);if(major<22||(major===22&&minor<19))throw new Error('E2bNodeVersionUnsupported');
  const argv=process.argv.slice(2),get=flag=>argv[argv.indexOf(flag)+1];
  if(argv.includes('--reconcile')){if(!argv.includes('--sdk')||!argv.includes('--output'))throw new Error('E2bReconcileArgumentsRequired');console.log(JSON.stringify(await reconcilePublication(JSON.parse(await readFile(get('--output'),'utf8')),await createE2bAdapter(get('--sdk')))));return;}
  for(const flag of ['--bundle','--base-image','--output'])if(!argv.includes(flag))throw new Error('E2bPublisherArgumentRequired');
  const args={bundle:resolve(get('--bundle')),baseImage:get('--base-image'),output:resolve(get('--output')),retainArtifact:argv.includes('--retain-artifact'),operationId:argv.includes('--operation-id')?get('--operation-id'):undefined};
  if(!argv.includes('--live')){console.log(JSON.stringify(await publicationPlan(args)));return;}
  if(!argv.includes('--sdk'))throw new Error('E2bPinnedSdkPathRequired');
  const result=await publish(args,await createE2bAdapter(get('--sdk')));
  console.log(JSON.stringify({status:result.status,operationId:result.operationId,artifactId:result.artifactId??null,providerArtifactVersion:result.providerArtifactVersion??null,cleanupVerified:result.cleanupVerified,artifactRetained:result.artifactRetained,errorCode:result.errorCode??null}));
  if(result.status!=='prepared-uncertified'||!result.cleanupVerified)process.exitCode=1;
}
if(import.meta.url===pathToFileURL(resolve(process.argv[1]??'')).href)main().catch(()=>{console.error(JSON.stringify({status:'failed',code:'E2bPublisherRejected'}));process.exitCode=1;});
