#!/usr/bin/env node
import{cleanBuilderSupervisor}from'./base-supervisor-hygiene.mjs';
import{quiesceCleanParent}from'./clean-parent-quiescence.mjs';
import{waitBoatHydration}from'./boat-hydration.mjs';
import{freshBootSpec,disableBoatCapture}from'./managed-base-fresh-boot.mjs';
// Native Boxd/Boat candidate builder. No model credentials, task disk or registration.
import{githubCliProof,requireGithubCliProof}from'./github-cli-proof.mjs';
import { createHash, randomUUID } from 'node:crypto';
import { readFile, writeFile, mkdir, open, lstat } from 'node:fs/promises';
import { resolve, dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import{uploadParentPreflight,inspectParentPreflight,removeParentPreflight,persistStoppedParent,repairableMetadata,VERIFIED_PROVIDER_ANCESTORS}from'./parent-preflight.mjs';
import {loadCleanLineage,REUSED_HARNESS_FILES} from './clean-base-lineage.mjs';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const quote = value => `'${value.replaceAll("'", "'\\''")}'`;
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const safeErrors=new Set(['BoatRequestFailed','BuilderCommandFailed','InvalidBuilderExecutorIdentity','BuilderRootPrivilegeRequired','GuestSanitationRejected','InvalidPreparedArtifactIdentity','ProviderArtifactFailed','ProviderArtifactDeadline','OriginalBuilderIdentityMismatch','InvalidChunkReceipt','InvalidPreparationReceipt','BuilderCommandUncertain','ProviderBootOwnerUnavailable','ProviderMachineFailed','ProviderStopUncertain','DerivedRuntimeReadbackMismatch','ParentPreflightRejected','ParentPreflightReceiptInvalid','ManagedAncestorReadbackInvalid','CleanParentContainsTask','CleanParentArtifactMismatch','CleanDerivationProviderUnsupported','CleanSupervisorHygieneRejected','PublicationPrimaryOwnerUnsupported']);
safeErrors.add('CleanParentQuiescenceRejected');
safeErrors.add('CleanDisabledParentRejected');
for(const code of['OriginalHydrationPending','ProviderHydrationUnavailable'])safeErrors.add(code);
safeErrors.add('ManagedGithubCliCapabilityMissing');
for(const code of ['ManagedGithubCliReceiptInvalid','ManagedGithubCliProofInvalid','ManagedGithubCliProofFailed'])safeErrors.add(code);
// Never replace the durable identity of an earlier uncertain publication.
// Operators must inspect/reconcile that report before choosing a fresh path.
export async function requireFreshPublicationReport(path) {
  try { await lstat(path); } catch(error) { if(error.code==='ENOENT')return;throw error; }
  throw new Error('ExistingPublicationOperation');
}
export async function verifyBundle(directory) {
  const manifest = JSON.parse(await readFile(join(directory, 'bundle.json'), 'utf8'));
  const allowed = new Set(['runtime','claude','codex','grok','harness-manifest.json','harness-release.py',
    'launch-runtime.sh','launch-recovery.sh','managed-base-install.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py','prepare-plan.json']);
  if (manifest.managedBaseDerivationVersion===1) allowed.add('managed-base-derive.py');
  if (manifest.managedBaseFreshBootVersion===1) allowed.add('managed-base-fresh-boot.py');
  if(manifest.managedBaseFreshBootMode!==undefined&&(manifest.managedBaseFreshBootMode!=='post-hydration-first-start-v1'||manifest.managedBaseFreshBootVersion!==1))throw new Error('UnsupportedFreshBootMode');
  if (manifest.managedBaseFreshBootVersion!==undefined&&manifest.managedBaseFreshBootVersion!==1)throw new Error('UnsupportedFreshBootVersion');
  if (manifest.managedBaseDerivationVersion!==undefined&&manifest.managedBaseDerivationVersion!==1)throw new Error('UnsupportedDerivationVersion');
  if (Array.isArray(manifest.files) && manifest.files.some(file=>file.path==='codex-assets.tar')) allowed.add('codex-assets.tar');
  if (Array.isArray(manifest.files) && !manifest.files.some(file=>['runtime-repair.py','runtime-recreation.py'].includes(file.path))) { allowed.delete('runtime-repair.py'); allowed.delete('runtime-recreation.py'); }
  if (manifest.schema !== 1 || manifest.status !== 'candidate-only' || !/^zuse-base-[a-f0-9]{32}$/.test(manifest.releaseId)
      || !/^[a-f0-9]{64}$/.test(manifest.harnessRelease) || !Array.isArray(manifest.files)
      || manifest.files.length !== allowed.size || new Set(manifest.files.map(file=>file.path)).size !== allowed.size) throw new Error('InvalidBundle');
  if(manifest.files.find(file=>file.path==='runtime')?.sha256!==manifest.runtimeSha256)throw new Error('RuntimeBindingMismatch');
  for (const file of manifest.files) {
    if (!allowed.has(file.path) || !/^[a-f0-9]{64}$/.test(file.sha256)
        || !Number.isSafeInteger(file.bytes) || file.bytes < 1 || file.bytes > 512*1024*1024) throw new Error('InvalidBundleFile');
    if ((await lstat(join(directory,file.path))).isSymbolicLink()) throw new Error('BundleSourceSymlink');
    const source = await open(join(directory,file.path),'r');
    try { const stat=await source.stat(); if (!stat.isFile() || stat.size!==file.bytes) throw new Error('BundleSizeMismatch');
      const digest=createHash('sha256'); for await(const chunk of source.createReadStream()) digest.update(chunk);
      if(digest.digest('hex')!==file.sha256) throw new Error('BundleIntegrityMismatch');
    } finally { await source.close(); }
  }
  if (allowed.has('codex-assets.tar')) await promisify(execFile)('python3',[join(directory,'harness-release.py'),'verify-asset-archive','--manifest',join(directory,'harness-manifest.json'),'--archive',join(directory,'codex-assets.tar'),'--release',manifest.harnessRelease],{timeout:30000,maxBuffer:4096,env:{PATH:'/usr/bin:/bin'}});
  return manifest;
}
// File PUT overwrites one exact pathname. After an uncertain gateway response,
// recheck the original VM and inspect the exact bytes before any bounded retry.
export async function uploadExactChunk(adapter,id,name,path,data) {
  const expected={bytes:data.length,sha256:sha(data)};
  for(let attempt=0;attempt<3;attempt++) {
    try {
      if(await adapter.upload(id,path,data)!==expected.bytes)throw new Error('InvalidChunkReceipt');
      return expected;
    }catch(error){
      if(![502,503].includes(error.status)||attempt===2)throw error;
      const machine=await adapter.identity(id);
      if(machine.id!==id||machine.name!==name)throw new Error('OriginalBuilderIdentityMismatch');
      const result=await adapter.exec(id,`if test -f ${quote(path)}; then stat -c %s ${quote(path)}; sha256sum ${quote(path)}; fi`);
      const lines=result.trim().split('\n');
      if(lines.length===2&&Number(lines[0])===expected.bytes&&lines[1].split(/\s+/)[0]===expected.sha256)return expected;
      await new Promise(resolve=>setTimeout(resolve,200*(attempt+1)));
    }
  }
}
export async function createBoatBuilderAdapter() {
  if(!process.env.BOAT_API_KEY) throw new Error('MissingBoatKey');
  async function request(method,path,body,headers={},timeoutMs=45000) {
    const result=await fetch('https://boat.dev/api/v1'+path,{method,
      headers:{Authorization:`Bearer ${process.env.BOAT_API_KEY}`,'content-type':'application/json',...headers},
      body:body===undefined?undefined:JSON.stringify(body),redirect:'error',signal:AbortSignal.timeout(timeoutMs)});
    if(!result.ok){await result.body?.cancel();const error=new Error('BoatRequestFailed');error.status=result.status;throw error;}
    return result.status===204?{}:result.json();
  }
  return {
    createFromCleanSnapshot:async(name,artifact,{ttlSeconds=1800}={})=>{if(![600,1800].includes(ttlSeconds))throw new Error('InvalidBuilderTtl');return(await request('POST','/sandboxes',{name,from:artifact,type:'small',ttlSeconds,noEnv:true,visibility:'private'},{'Idempotency-Key':name})).sandbox.id;},
    create:async name=>(await request('POST','/sandboxes',{name,type:'small',ttlSeconds:1800,noEnv:true,visibility:'private'},{'Idempotency-Key':name})).sandbox.id,
    find:async name=>{let cursor;let found;for(let i=0;i<20;i++){
      const page=await request('GET',`/sandboxes${cursor?'?cursor='+encodeURIComponent(cursor):''}`);
      for(const machine of page.sandboxes??[]) if(machine.name===name){if(found)throw new Error('AmbiguousBuilder');found=machine.id;}
      cursor=page.pageInfo?.nextCursor;if(!cursor)return found;
    }throw new Error('IncompleteBuilderListing');},
    ready:async id=>{
      const state=(await request('GET',`/sandboxes/${encodeURIComponent(id)}`)).sandbox?.state;
      if(['error','deleted'].includes(state))throw new Error('ProviderMachineFailed');
      if(!['ready','idle','running'].includes(state))return false;
      try{const result=await request('POST',`/sandboxes/${encodeURIComponent(id)}/commands`,{command:'true',timeoutSeconds:10});return result.exitCode===0&&!result.timedOut;}
      catch(error){if([404,409,503].includes(error.status))return false;throw error;}
    },
    exec:async(id,command,{commandTimeoutSeconds=30,fetchTimeoutMs=45000}={})=>{
      if(!Number.isInteger(commandTimeoutSeconds)||commandTimeoutSeconds<1||commandTimeoutSeconds>60||!Number.isInteger(fetchTimeoutMs)||fetchTimeoutMs<commandTimeoutSeconds*1000+5000||fetchTimeoutMs>65000)throw new Error('InvalidCommandBudget');
      const result=await request('POST',`/sandboxes/${encodeURIComponent(id)}/commands`,{command,timeoutSeconds:commandTimeoutSeconds},{},fetchTimeoutMs);
      if(result.exitCode!==0||result.timedOut)throw new Error('BuilderCommandFailed');return result.stdout;},
    identity:async id=>(await request('GET',`/sandboxes/${encodeURIComponent(id)}`)).sandbox,
    commandState:async(id,processId)=>{
      if(!Number.isSafeInteger(processId)||processId<1)throw new Error('InvalidPreparationReceipt');
      const v=await request('GET',`/sandboxes/${encodeURIComponent(id)}/commands/${processId}`);
      if(v.processId!==processId)throw new Error('InvalidPreparationReceipt');
      return{processId:v.processId,running:v.running===true,status:v.status,exitCode:v.exitCode};
    },
    prepare:async(id,command,onReceipt)=>{
      // Use Boat's tracked detached process, not shell backgrounding that may
      // remain owned by a synchronous command's executor/cancellation group.
      const started=await request('POST',`/sandboxes/${encodeURIComponent(id)}/commands`,{command,detached:true});
      if(!Number.isSafeInteger(started.processId)||started.processId<1)throw new Error('InvalidPreparationReceipt');
      const receipt={mode:'boat-detached-command',machineId:id,processId:started.processId};
      if(typeof started.commandId==='string')receipt.commandId=started.commandId;
      if(onReceipt)await onReceipt(receipt);
      await waitFor(async()=>{
        const state=await request('GET',`/sandboxes/${encodeURIComponent(id)}/commands/${started.processId}`);
        if(state.processId!==started.processId)throw new Error('InvalidPreparationReceipt');
        if(state.status==='lost')throw new Error('BuilderCommandUncertain');
        if(state.running===true)return {status:'pending'};
        if(state.status!=='exited'||state.exitCode!==0)throw new Error('BuilderCommandFailed');
        return {status:'ready'};
      },state=>state.status==='ready',900000);
      return receipt;
    },
    upload:async(id,path,data)=>(await request('PUT',`/sandboxes/${encodeURIComponent(id)}/files`,{path,encoding:'base64',content:data.toString('base64')})).size,
    restore:async(name,artifact)=>(await request('POST','/sandboxes',{name,from:artifact,type:'small',ttlSeconds:1200,noEnv:true,visibility:'private'},{'Idempotency-Key':name})).sandbox,
    stop:async id=>{
      const value=await request('POST',`/sandboxes/${encodeURIComponent(id)}/stop`,{});
      const receipt=value.stop??value.sandbox?.stop;
      if(typeof receipt?.id!=='string'||!['pending','failing','completed','superseded'].includes(receipt.status))throw new Error('ProviderStopUncertain');
      return receipt;
    },
    stopStatus:async(id,operation)=>{
      const value=(await request('GET',`/sandboxes/${encodeURIComponent(id)}/stops/${encodeURIComponent(operation)}`)).stop;
      if(value?.id!==operation||!['pending','failing','completed','superseded'].includes(value.status))throw new Error('ProviderStopUncertain');
      return value;
    },
    resume:async id=>(await request('POST',`/sandboxes/${encodeURIComponent(id)}/resume`,{ttlSeconds:1200})).sandbox,
    snapshot:async(id,name)=>(await request('POST','/saved-snapshots',{sandboxId:id,name})).snapshot,
    snapshotState:async name=>(await request('GET','/saved-snapshots/'+encodeURIComponent(name))).snapshot,
    removeSnapshot:name=>request('DELETE','/saved-snapshots/'+encodeURIComponent(name)),
    snapshotAbsent:async name=>{try{await request('GET','/saved-snapshots/'+encodeURIComponent(name));return false;}catch(error){if(error.status===404)return true;throw error;}},
    remove:id=>request('DELETE','/sandboxes/'+encodeURIComponent(id),undefined,{'X-Ascii-Confirm-Delete':id}),
    absent:async id=>{try{await request('GET','/sandboxes/'+encodeURIComponent(id));return false;}catch(error){if(error.status===404)return true;throw error;}},
    close:()=>{},
  };
}
export const BOXD_BOOT_COMMAND=['/bin/sh','-c','while [ ! -x /usr/local/lib/zuse/runtime-supervisor.py ]; do sleep 0.1; done; exec /usr/bin/python3 /usr/local/lib/zuse/runtime-supervisor.py serve'];
export async function createBoxdBuilderAdapter(sdkPath,image,{nativeDefault=false}={}) {
  if(nativeDefault?image!==undefined:!/^[^\s]+@sha256:[a-f0-9]{64}$/.test(image))throw new Error('ImmutableImageRequired');
  if(!process.env.BOXD_API_KEY)throw new Error('MissingBoxdKey');
  const vendor=await import(sdkPath?pathToFileURL(resolve(sdkPath)).href:'@boxd-sh/sdk');
  if(vendor.VERSION!=='0.2.14')throw new Error('UnpinnedBoxdSdk');
  const sdk=new vendor.Boxd({apiKey:process.env.BOXD_API_KEY,maxRetries:0,timeout:45000});
  return {
    create:async name=>(await sdk.machines.create({name,...(nativeDefault?{}:{image,cmd:BOXD_BOOT_COMMAND,restartPolicy:'always'}),org:process.env.BOXD_ORG,isolated:true,
      config:{vcpu:1,memory:'4G',autoDestroyTimeout:1800,autoSuspendTimeout:0}})).id,
    find:async name=>{try{return(await sdk.machines.get(name)).id;}catch(error){if(error.name==='NotFoundError')return undefined;throw error;}},
    ready:async id=>{const machine=await sdk.machines.get(id);if(machine.status!=='running')return false;
      try{return(await sdk.machines.exec(id,{command:'true',timeout:10000})).success;}catch{return false;}},
    exec:async(id,command)=>{const result=await sdk.machines.exec(id,{command,timeout:30000});
      if(!result.success)throw new Error('BuilderCommandFailed');return result.stdout;},
    identity:id=>sdk.machines.get(id),
    sourceInfo:async id=>{const vm=await sdk.machines.get(id);return{mode:nativeDefault?'boxd-native-systemd':'boxd-oci-primary-command',imageRef:vm.imageRef,restartPolicy:vm.restartPolicy,...(vm.resources?{resources:vm.resources}:{})};},
    bootOwnerReady:async id=>{
      if(!nativeDefault)return;
      const proof=await sdk.machines.exec(id,{command:'sudo -n systemd-run --quiet --collect --wait --uid=root -- /bin/true',timeout:15000});
      if(!proof.success)throw new Error('ProviderBootOwnerUnavailable');
    },
    upload:(id,path,data)=>sdk.machines.files.upload(id,path,data),
    prepare:async(id,command)=>{const installer=new vendor.Boxd({apiKey:process.env.BOXD_API_KEY,maxRetries:0,timeout:900000});
      try{const result=await installer.machines.exec(id,{command,timeout:840000});if(!result.success)throw new Error('BuilderCommandFailed');}
      finally{installer.close();}},
    snapshot:(id,name)=>sdk.snapshots.create(id,name),
    snapshotState:name=>sdk.snapshots.get(name,{org:process.env.BOXD_ORG}),
    removeSnapshot:name=>sdk.snapshots.delete(name,{org:process.env.BOXD_ORG}),
    snapshotAbsent:async name=>{try{await sdk.snapshots.get(name,{org:process.env.BOXD_ORG});return false;}catch(error){if(error.name==='NotFoundError')return true;throw error;}},
    remove:id=>sdk.machines.delete(id),
    absent:async id=>{try{await sdk.machines.get(id);return false;}catch(error){if(error.name==='NotFoundError')return true;throw error;}},
    close:()=>sdk.close(),
  };
}
async function waitFor(task,predicate,timeout=600000) {
  const end=Date.now()+timeout;
  while(Date.now()<end){const result=await task();if(predicate(result))return result;
    if(result?.status==='failed')throw new Error('ProviderArtifactFailed');await new Promise(r=>setTimeout(r,1000));}
  throw new Error('ProviderArtifactDeadline');
}
export function rootPathFactsCommand(){const code="import os,json,stat,errno;paths=['/','/opt','/opt/zuse','/opt/zuse/harnesses','/opt/zuse-base-input','/tmp','/tmp/zuse-base-input','/var/lib/zuse'];r={'executor':dict(uid=os.getuid(),effectiveUid=os.geteuid(),sudoUid=int(os.environ['SUDO_UID']) if os.environ.get('SUDO_UID','').isdigit() else None)};exec(\"for p in paths:\\n try:\\n  m=os.lstat(p);r[p]=dict(uid=m.st_uid,gid=m.st_gid,mode=oct(stat.S_IMODE(m.st_mode)),symlink=stat.S_ISLNK(m.st_mode),inode=str(m.st_ino),device=str(m.st_dev))\\n except OSError as e:r[p]=dict(error=errno.errorcode.get(e.errno,'UNAVAILABLE'))\");print(json.dumps(r))";return 'python3 -c '+quote(code);}
export async function build(args,adapter) {
  await requireFreshPublicationReport(args.output);
  const bundle=await verifyBundle(args.bundle); // Validate before any allocation.
  if(bundle.managedBaseFreshBootVersion===1&&args.provider!==(bundle.managedBaseFreshBootMode==='post-hydration-first-start-v1'?'boat':'boxd'))throw new Error('FreshBootProviderUnsupported');
  const parent=args.deriveLineage?await loadCleanLineage(args.deriveLineage,args.deriveLineageSha256,bundle):null;
  if((args.parentPreflightOnly||args.parentQuiescenceOnly)&&!parent)throw new Error('ParentPreflightRequiresCleanSource');
  if(args.parentPreflightOnly&&args.parentQuiescenceOnly)throw new Error('ParentDiagnosticModeAmbiguous');
  if(parent){
    if(args.provider!=='boat'||!adapter.createFromCleanSnapshot)throw new Error('CleanDerivationProviderUnsupported');
    const source=await adapter.snapshotState(parent.artifactId);
    if(source.status!=='ready'||source.name!==parent.artifactId||(parent.providerArtifactVersion!==null&&(source.version??source.snapshotId??null)!==parent.providerArtifactVersion))throw new Error('CleanParentArtifactMismatch');
  }
  const runId='zuse-base-build-'+randomUUID().replaceAll('-','').slice(0,24);
  const record={schema:1,status:'candidate-only',provider:args.provider,runId,releaseId:bundle.releaseId,
    harnessRelease:bundle.harnessRelease,runtimeSha256:bundle.runtimeSha256,builderRequestedAt:Date.now(),builderTtlSeconds:args.parentPreflightOnly||args.parentQuiescenceOnly?600:1800,parentPreflightOnly:args.parentPreflightOnly===true,parentQuiescenceOnly:args.parentQuiescenceOnly===true,
    bundleSha256:sha(await readFile(join(args.bundle,'bundle.json'))),builderSha256:sha(await readFile(new URL(import.meta.url))),stages:[],uploadReceipts:[],uploadedBytes:0,cleanupVerified:false};
  if(parent)record.cleanParent={lineageSha256:parent.sha256,artifactId:parent.artifactId,providerArtifactVersion:parent.providerArtifactVersion,parentRuntimeSha256:parent.parentRuntimeSha256,parentBundleSha256:parent.lineage.parentBundleFileSha256,harnessRelease:bundle.harnessRelease,reusedFiles:[...REUSED_HARNESS_FILES]};
  await mkdir(dirname(args.output),{recursive:true});
  let reportClaimed=false;
  const persist=async()=>{try{await writeFile(args.output,JSON.stringify(record,null,2)+'\n',{mode:0o600,flag:reportClaimed?'w':'wx'});reportClaimed=true;}catch(error){if(error.code==='EEXIST')throw new Error('ExistingPublicationOperation');throw error;}};
  const stage=async(name,task)=>{const started=performance.now();record.activeStage=name;await persist();const result=await task();
    record.stages.push({name,elapsedMs:Math.round((performance.now()-started)*100)/100});await persist();return result;};
  let snapshotSubmitted=false;let snapshotObserved=false;let allocationSubmitted=false;let success=false;
  await persist(); // Durable unique name precedes uncertain allocation.
  try {
    allocationSubmitted=true;record.machineId=await stage('allocate',()=>parent?adapter.createFromCleanSnapshot(runId,parent.artifactId,{ttlSeconds:args.parentPreflightOnly||args.parentQuiescenceOnly?600:1800}):adapter.create(runId));await persist();
    await stage('command-ready',()=>waitFor(()=>adapter.ready?adapter.ready(record.machineId):Promise.resolve(true),Boolean,120000));
    if(args.provider==='boat')record.parentHydration=await stage('parent-filesystem-hydrated',()=>waitBoatHydration(adapter,record.machineId,runId,{onObservation:async value=>{record.parentHydration=value;await persist();}}));
    if(adapter.bootOwnerReady)await stage('provider-boot-owner',()=>adapter.bootOwnerReady(record.machineId));
    if(parent&&args.parentPreflightOnly)record.restoredParentResident=await stage('clean-parent-resident-ready',()=>waitFor(async()=>{
      let value;try{value=JSON.parse(await adapter.exec(record.machineId,'sudo -n -u zuse python3 /usr/local/lib/zuse/runtime-supervisor.py ping'));}catch{return null;}
      if(value.bound===true||value.childAlive===true||value.recoveryAlive===true)throw new Error('CleanParentContainsTask');
      return value.version===1&&value.bound===false&&value.childAlive===false&&value.recoveryAlive===false?{version:1,bound:false,childAlive:false,recoveryAlive:false}:null;
    },Boolean,120000));
    if(parent&&!args.parentPreflightOnly){record.parentAdmission='full-image-and-owned-clean-quiescence';await persist();}
    if(adapter.sourceInfo){record.providerSource=await adapter.sourceInfo(record.machineId);await persist();}
    const uid=(await adapter.exec(record.machineId,'id -u')).trim();
    if(!/^[0-9]{1,10}$/.test(uid))throw new Error('InvalidBuilderExecutorIdentity');
    record.executorUid=Number(uid);record.installationPrivilege=uid==='0'?'root':'sudo-root';
    if(uid!=='0'&&(await adapter.exec(record.machineId,'sudo -n id -u')).trim()!=='0')throw new Error('BuilderRootPrivilegeRequired');
    const rootShell=uid==='0'?'sh -c':'sudo -n sh -c';
    await persist();
    if(parent){record.rootPathFactsBefore=JSON.parse(await adapter.exec(record.machineId,`${rootShell} ${quote(rootPathFactsCommand())}`));await persist();}
    let preflight;
    if(parent){
      await stage('upload-parent-preflight',()=>uploadParentPreflight({adapter,id:record.machineId,runId,bundle:args.bundle,parent,rootShell,uploadExactChunk,record,persist,prepareQuiescence:!args.parentPreflightOnly}));
      preflight=await stage('strict-parent-preflight',()=>inspectParentPreflight({adapter,id:record.machineId,parent,rootShell,bundle:args.bundle,record,persist,quiescenceImage:!args.parentPreflightOnly}));
      record.parentPreflightBefore=preflight;await persist();
      if(args.parentPreflightOnly){record.status=preflight.status==='passed'?'parent-preflight-verified':'parent-preflight-rejected';return record;}
      if(preflight.status!=='quiescence-image-verified'&&!repairableMetadata(preflight))throw new Error('ParentPreflightRejected');
    }
    if(args.provider==='boat'&&(!parent||preflight.status!=='quiescence-image-verified')){
      const ownershipSource=await readFile(new URL('./managed-base-ownership.py',import.meta.url));
      const receipt=await stage('protect-managed-ancestor',async()=>JSON.parse(await adapter.exec(record.machineId,`${rootShell} ${quote('/usr/bin/python3 -I -c '+quote(ownershipSource.toString('utf8')))}`)));
      if(receipt.version!==2||receipt.recursive!==false||!Array.isArray(receipt.paths)||receipt.paths.length!==VERIFIED_PROVIDER_ANCESTORS.length||receipt.paths.some((r,i)=>r.path!==VERIFIED_PROVIDER_ANCESTORS[i]||r.recursive!==false||r.after?.uid!==0||r.after?.gid!==0||r.after?.mode!=='0o755'))throw new Error('ManagedAncestorReadbackInvalid');
      record.managedAncestor={sourceSha256:sha(ownershipSource),receipt};await persist();
    }
    if(parent){
      const after=preflight.status==='quiescence-image-verified'?preflight:await stage('strict-parent-preflight-confirmed',()=>inspectParentPreflight({adapter,id:record.machineId,parent,rootShell,bundle:args.bundle,record,persist,quiescenceImage:true}));
      record.parentPreflight=after;await persist();if(after.status!=='quiescence-image-verified')throw new Error('ParentPreflightRejected');
    }
    if(parent){
      const hygiene=await stage('stop-idle-parent-supervisor',()=>quiesceCleanParent({adapter,id:record.machineId,rootShell,operationId:runId,parent,bundle:args.bundle,record,persist}));
      await stage('persist-stopped-parent',()=>persistStoppedParent({adapter,id:record.machineId,rootShell,parent,bundle:args.bundle,operationId:runId,hygiene}));
      record.stoppedParentPreflight=await stage('strict-stopped-parent-preflight',()=>inspectParentPreflight({adapter,id:record.machineId,parent,rootShell,bundle:args.bundle,record,persist,operationId:runId}));await persist();
      if(record.stoppedParentPreflight.status!=='passed')throw new Error('ParentPreflightRejected');
      if(args.parentQuiescenceOnly){record.status='parent-quiescence-verified';return record;}
    }
    await stage('upload',async()=>{
      await adapter.exec(record.machineId,'mkdir -p /tmp/zuse-base-input');
      // Avoid provider unary limits and unbounded buffering of a large guest binary.
      for(const file of [...bundle.files.filter(file=>!parent||(!REUSED_HARNESS_FILES.has(file.path)&&!['harness-release.py','harness-manifest.json'].includes(file.path))),...(!parent?[{path:'bundle.json',bytes:(await readFile(join(args.bundle,'bundle.json'))).length}]:[])]){
        const handle=await open(join(args.bundle,file.path),'r');const chunks=[];
        try{const buffer=Buffer.alloc(2*1024*1024);let position=0;
          while(true){const {bytesRead}=await handle.read(buffer,0,buffer.length,position);if(!bytesRead)break;
            const path='/tmp/zuse-base-input/'+file.path+'.part-'+String(chunks.length).padStart(5,'0');
            const receipt=await uploadExactChunk(adapter,record.machineId,runId,path,buffer.subarray(0,bytesRead));
            record.uploadReceipts.push({path,...receipt});record.uploadedBytes+=bytesRead;
            if(record.uploadReceipts.length%8===0)await persist();chunks.push(path);position+=bytesRead;}
        }finally{await handle.close();}
        await adapter.exec(record.machineId,`cat ${chunks.map(quote).join(' ')} > ${quote('/tmp/zuse-base-input/'+file.path)} && rm -- ${chunks.map(quote).join(' ')}`);
      }
    });
    await stage('promote-overlay-input',()=>adapter.exec(record.machineId,`${rootShell} ${quote('test ! -e /opt/zuse-base-input && mv -- /tmp/zuse-base-input /opt/zuse-base-input')}`));
    if(parent){
      await stage('copy-verified-parent-metadata',()=>adapter.exec(record.machineId,`${rootShell} ${quote('cp -P -- /var/lib/zuse/base-preflight/bundle.json /var/lib/zuse/base-preflight/harness-release.py /var/lib/zuse/base-preflight/harness-manifest.json /var/lib/zuse/base-preflight/derivation.json /var/lib/zuse/base-preflight/parent-stopped.json /opt/zuse-base-input/')}`));
      await stage('remove-parent-preflight',()=>removeParentPreflight(adapter,record.machineId,rootShell));
      await stage('protect-overlay-input',()=>adapter.exec(record.machineId,`${rootShell} ${quote('chown -hR 0:0 -- /opt/zuse-base-input && chmod -R go-w -- /opt/zuse-base-input')}`));
      record.rootPathFactsAfter=JSON.parse(await adapter.exec(record.machineId,`${rootShell} ${quote(rootPathFactsCommand())}`));await persist();
    }
    await stage('install-and-sanitize',async()=>{
      const install=(parent?'ZUSE_CLEAN_DERIVATION_SHA256='+quote(parent.sha256)+' ZUSE_CLEAN_DERIVATION_OPERATION_ID='+quote(runId)+' ':'')+'bash /opt/zuse-base-input/managed-base-install.sh /opt/zuse-base-input >/var/lib/zuse-base-install.log 2>&1';
      if(adapter.prepare){
        // Provider-native preparation: keep Boxd exec init alive, or retain
        // Boat's original tracked detached command receipt before observing it.
        const receipt=await adapter.prepare(record.machineId,`${rootShell} ${quote(install)}`,async value=>{record.preparationReceipt=value;await persist();});
        if(receipt)record.preparationReceipt=receipt;
      }else{
        await adapter.exec(record.machineId,`${rootShell} ${quote("nohup sh -c 'bash /opt/zuse-base-input/managed-base-install.sh /opt/zuse-base-input && touch /opt/zuse/preparation/install-success || touch /var/lib/zuse-base-install-failed' >/var/lib/zuse-base-install.log 2>&1 </dev/null &")}`);
        await waitFor(async()=>{
          const state=(await adapter.exec(record.machineId,`${rootShell} ${quote('if test -f /var/lib/zuse-base-install-failed; then echo failed; elif test -f /opt/zuse/preparation/install-success; then echo ready; else echo pending; fi')}`)).trim();
          return {status:state};
        },value=>value.status==='ready',900000);
      }
    });
    record.manifest=JSON.parse(await adapter.exec(record.machineId,`${rootShell} ${quote('cat /opt/zuse/preparation/status/manifest.json')}`));
    if(record.manifest.managedBase!==true||record.manifest.credentialsRemoved!==true||record.manifest.repositories?.length!==0)throw new Error('GuestSanitationRejected');
    if(parent){
      const installed=(await adapter.exec(record.machineId,`${rootShell} ${quote('sha256sum /usr/local/bin/zuse-cloud-runtime')}`)).trim().split(/\s+/)[0];
      if(installed!==bundle.runtimeSha256)throw new Error('DerivedRuntimeReadbackMismatch');
    }
    if(args.requireGithubCli&&record.manifest.rustGithubCliVersion!==1)throw new Error('ManagedGithubCliCapabilityMissing');
    record.githubCliProof=await stage('managed-github-cli-builder',()=>githubCliProof(adapter,record.machineId,record.manifest));await persist();requireGithubCliProof(record.githubCliProof);
    if(bundle.managedBaseFreshBootVersion===1){record.initialRuntimeBootstrap=await stage('fresh-runtime-image-pins',()=>freshBootSpec(adapter,record.machineId,bundle));await persist();}
    if(record.providerSource?.mode==='boxd-oci-primary-command')throw new Error('PublicationPrimaryOwnerUnsupported');
    await stage('quiesce-clean-builder',()=>cleanBuilderSupervisor({adapter,id:record.machineId,rootShell,operationId:runId,runtimeSha256:bundle.runtimeSha256,record,persist}));
    if(bundle.managedBaseFreshBootMode==='post-hydration-first-start-v1'){
      record.disabledBootCapture=await stage('disable-clean-boat-boot-owner',()=>disableBoatCapture(adapter,record.machineId,record.initialRuntimeBootstrap,{purpose:'publication-proof',operationId:runId,workspaceId:'image-proof-'+runId,generation:1,storageIncarnationId:'image-proof-'+runId},record.supervisorHygiene,{onIntent:async value=>{record.disableCaptureIntent=value;await persist();},onReceipt:async value=>{record.disabledBootCapture=value;await persist();}}));await persist();
    }
    record.snapshotName=runId; snapshotSubmitted=true; await persist();
    await stage('capture',()=>adapter.snapshot(record.machineId,runId));
    const artifact=await stage('artifact-ready',()=>waitFor(()=>adapter.snapshotState(runId),value=>value.status==='ready'));
    snapshotObserved=true;
    record.artifactId=args.provider==='boat'?artifact.name:artifact.id;
    if(typeof record.artifactId!=='string'||!record.artifactId||record.artifactId.length>512)throw new Error('InvalidPreparedArtifactIdentity');
    record.providerArtifactVersion=artifact.version??artifact.snapshotId??null;
    record.status='prepared-uncertified';success=true;
  } catch(error){record.status='failed';record.failedStage=record.activeStage;
    if(Number.isInteger(error.status)&&error.status>=400&&error.status<=599)record.providerHttpStatus=error.status;
    record.errorCode=safeErrors.has(error.message)?error.message:'ProviderBuildFailed';
    if(['OriginalHydrationPending','ProviderHydrationUnavailable'].includes(error.message)){record.status='observation-deferred';record.inspectionRequired=true;}}
  finally {
    if(record.inspectionRequired){record.cleanupVerified=false;record.artifactRetained=false;await persist();await adapter.close();return record;}
    if(record.failedStage==='install-and-sanitize'&&record.machineId){
      try{const prefix=record.installationPrivilege==='sudo-root'?'sudo -n ':'';
        const diagnostic=await adapter.exec(record.machineId,prefix+'tail -c 16384 /var/lib/zuse-base-install.log');
        await writeFile(args.output+'.install-log',diagnostic.slice(-16384),{mode:0o600});
      }catch{/* Preserve original error and always continue resource cleanup. */}
    }
    try{
      // Inspect by the original unique name after an ambiguous allocation; never allocate again.
      const machine=record.machineId??await adapter.find(runId);
      if(!machine&&allocationSubmitted&&!record.machineId)throw new Error('OriginalAllocationStillUncertain');
      if(machine){try{await adapter.remove(machine);}catch{if(!await adapter.absent(machine))throw new Error('BuilderDeleteUncertain');}
        await waitFor(()=>adapter.absent(machine),Boolean,90000);}
      record.cleanupVerified=true;
    }catch{record.status='cleanup-required';}
    if(snapshotSubmitted&&(!success||!args.retainArtifact)){
      try{if(!snapshotObserved){await adapter.snapshotState(runId);snapshotObserved=true;}
        try{await adapter.removeSnapshot(runId);}catch{if(!await adapter.snapshotAbsent(runId))throw new Error('ArtifactDeleteUncertain');}
        await waitFor(()=>adapter.snapshotAbsent(runId),Boolean,90000);record.artifactRetained=false;}
      catch{record.status='cleanup-required';record.cleanupVerified=false;}
    }else record.artifactRetained=success;
    await persist();await adapter.close();
  }
  return record;
}
async function main(){
  const argv=process.argv.slice(2);const get=name=>argv[argv.indexOf(name)+1];
  if(argv.includes('--help')){console.log('build-managed-base.mjs --provider boxd|boat --bundle DIR --output REPORT [--live] [--retain-artifact] [--boxd-sdk PATH] [--boxd-native-default | --boxd-image OCI@sha256:DIGEST]');return;}
  const args={requireGithubCli:argv.includes('--require-github-cli'),parentPreflightOnly:argv.includes('--parent-preflight-only'),parentQuiescenceOnly:argv.includes('--parent-quiescence-only'),deriveLineage:argv.includes('--derive-lineage')?resolve(get('--derive-lineage')):undefined,deriveLineageSha256:argv.includes('--derive-lineage-sha256')?get('--derive-lineage-sha256'):undefined,provider:get('--provider'),bundle:resolve(get('--bundle')??''),output:resolve(get('--output')??''),retainArtifact:argv.includes('--retain-artifact')};
  if(!['boxd','boat'].includes(args.provider)||!argv.includes('--bundle')||!argv.includes('--output'))throw new Error('InvalidArguments');
  const bundle=await verifyBundle(args.bundle);
  const parent=args.deriveLineage?await loadCleanLineage(args.deriveLineage,args.deriveLineageSha256,bundle):null;
  if((args.parentPreflightOnly||args.parentQuiescenceOnly)&&!parent)throw new Error('ParentPreflightRequiresCleanSource');
  if(args.parentPreflightOnly&&args.parentQuiescenceOnly)throw new Error('ParentDiagnosticModeAmbiguous');
  const image=argv.includes('--boxd-image')?get('--boxd-image'):undefined;
  const nativeDefault=argv.includes('--boxd-native-default');
  if((args.provider!=='boxd'&&nativeDefault)||(nativeDefault&&image!==undefined))throw new Error('InvalidBoxdSource');
  if(args.provider==='boxd'&&!nativeDefault&&!/^[^\s]+@sha256:[a-f0-9]{64}$/.test(image??''))throw new Error('ImmutableBoxdImageRequired');
  if(!argv.includes('--live')){
    let preflightBytes=parent?parent.bytes.length+(await Promise.all(['bundle.json','harness-release.py','harness-manifest.json'].map(name=>readFile(join(args.bundle,name))))).reduce((n,b)=>n+b.length,0):0;
    if(args.parentQuiescenceOnly)preflightBytes+=(await Promise.all(['managed-base-derive.py','base-supervisor-hygiene.py','clean-parent-quiescence.py'].map(name=>readFile(new URL('./'+name,import.meta.url))))).reduce((n,b)=>n+b.length,0);
    const diagnostic=args.parentPreflightOnly||args.parentQuiescenceOnly;
    console.log(JSON.stringify({status:'plan-only',provider:args.provider,releaseId:bundle.releaseId,maxMachines:1,lifetimeSeconds:diagnostic?600:1800,parentPreflightOnly:args.parentPreflightOnly,parentQuiescenceOnly:args.parentQuiescenceOnly,snapshotCapture:!diagnostic,retainArtifact:args.retainArtifact,cleanParent:parent?{artifactId:parent.artifactId,lineageSha256:parent.sha256,parentRuntimeSha256:parent.parentRuntimeSha256}:null,preflightUploadBytes:preflightBytes,plannedUploadBytes:diagnostic?preflightBytes:bundle.files.filter(f=>!parent||!REUSED_HARNESS_FILES.has(f.path)).reduce((n,f)=>n+f.bytes,0)+(parent?.bytes.length??0)+(await readFile(join(args.bundle,'bundle.json'))).length}));return;
  }
  const adapter=args.provider==='boat'?await createBoatBuilderAdapter():await createBoxdBuilderAdapter(argv.includes('--boxd-sdk')?get('--boxd-sdk'):undefined,image,{nativeDefault});
  const result=await build(args,adapter);console.log(JSON.stringify({status:result.status,cleanupVerified:result.cleanupVerified,artifactRetained:result.artifactRetained}));
  if(result.status!=='prepared-uncertified'||!result.cleanupVerified)process.exitCode=1;
}
if(process.argv[1]&&pathToFileURL(resolve(process.argv[1])).href===import.meta.url){main().catch(()=>{console.error('Managed base build failed; no provider response or secret is printed.');process.exitCode=1;});}
