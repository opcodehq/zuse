#!/usr/bin/env node
// One active VM at a time. Temporary snapshot is always deleted; never registers support.
import{githubCliProof,githubCliReadiness,requireGithubCliProof}from'./github-cli-proof.mjs';
import{proveFreshBoot}from'./managed-base-fresh-boot.mjs';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {dirname,resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {build,verifyBundle,createBoxdBuilderAdapter,requireFreshPublicationReport} from './build-managed-base.mjs';
const argv=process.argv.slice(2),get=name=>argv[argv.indexOf(name)+1];
const pause=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function wait(task,predicate,timeout=120000){const end=Date.now()+timeout;while(Date.now()<end){const value=await task();if(predicate(value))return value;await pause(1000);}throw new Error('ProofDeadline');}
export async function cleanupUncertainBuilder(sdk,prepared,removeOwned){
 if(!prepared||prepared.cleanupVerified)return true;
 try{
  if(prepared.machineId){await removeOwned(prepared.machineId,prepared.runId);return true;}
  // An invisible failed create remains uncertain, even when name lookup is404.
  const vm=await sdk.machines.get(prepared.runId);
  if(vm.name!==prepared.runId)throw new Error('OriginalIdentityMismatch');
  await removeOwned(vm.id,prepared.runId);return true;
 }catch{return false;}
}
export function residentExtensions(manifest,resident,required=false) {
 const expected={repairVersion:manifest?.rustRuntimeRepairVersion,recreationVersion:manifest?.rustRecreationVersion};
 if(required&&Object.values(expected).some(version=>version!==1))throw new Error('RequiredResidentExtensionMissing');
 for(const [name,version] of Object.entries(expected)) {
  if(version!==undefined&&version!==null&&version!==1)throw new Error('UnknownResidentExtension');
  if(version===1&&resident[name]!==1)throw new Error('ResidentExtensionMismatch');
 }
 return {repairVersion:resident.repairVersion??null,recreationVersion:resident.recreationVersion??null};
}
async function main(){
 if(argv.includes('--help')){console.log('probe-boxd-supervisor.mjs --bundle DIR --output REPORT --boxd-sdk PATH [--boxd-native-default | --boxd-image OCI@sha256:DIGEST] [--live] [--retain-staging-candidate --candidate-owner ACCOUNT]');return;}
 if(!['--bundle','--output','--boxd-sdk'].every(key=>argv.includes(key)))throw new Error('ArgumentsRequired');
 const bundlePath=resolve(get('--bundle')),output=resolve(get('--output')),sdkPath=resolve(get('--boxd-sdk')),image=argv.includes('--boxd-image')?get('--boxd-image'):undefined,nativeDefault=argv.includes('--boxd-native-default');
 await requireFreshPublicationReport(output);
 await requireFreshPublicationReport(output+'.builder.json');
 const bundleManifest=await verifyBundle(bundlePath);
 const requireExtensions=bundleManifest.files.some(file=>file.path==='runtime-repair.py');
 const plan=JSON.parse(await readFile(resolve(bundlePath,'prepare-plan.json'),'utf8'));
 if(plan.rustRuntimeSupervisorVersion!==1)throw new Error('ExplicitSupervisorPlanRequired');
 if(nativeDefault?image!==undefined:!/^[^\s]+@sha256:[a-f0-9]{64}$/.test(image))throw new Error('ImmutableImageRequired');
 const retainCandidate=argv.includes('--retain-staging-candidate');
 const candidateOwner=argv.includes('--candidate-owner')?get('--candidate-owner'):undefined;
 if(retainCandidate&&(process.env.ZUSE_DEPLOYMENT_ENVIRONMENT!=='staging'||(candidateOwner!==undefined&&!/^[A-Za-z0-9_.-]{1,160}$/.test(candidateOwner))))throw new Error('ExplicitStagingCandidateIntentRequired');
 if(!argv.includes('--live')){console.log(JSON.stringify({status:'plan-only',maxActiveMachines:1,maxTemporarySnapshots:1,scope:'boot supervisor, snapshot restore, cold restart; no model acceptance'}));return;}
 await mkdir(dirname(output),{recursive:true});
 const report={schema:1,status:'incomplete',scope:'provider boot supervision only',agentAcceptanceVerified:false,stages:[],cleanupVerified:false,...(retainCandidate?(candidateOwner?{candidateOwner}:{candidateOwnerPending:true}):{} )};
 let reportClaimed=false;
 const save=async()=>{try{await writeFile(output,JSON.stringify(report,null,2)+'\n',{mode:0o600,flag:reportClaimed?'w':'wx'});reportClaimed=true;}catch(error){if(error.code==='EEXIST')throw new Error('ExistingPublicationOperation');throw error;}};
 const stage=async(name,operation)=>{report.activeStage=name;await save();const start=performance.now();const value=await operation();report.stages.push({name,elapsedMs:Math.round((performance.now()-start)*100)/100});await save();return value;};
 const vendor=await import(pathToFileURL(sdkPath).href);if(vendor.VERSION!=='0.2.14')throw new Error('PinnedSdkRequired');
 const sdk=new vendor.Boxd({apiKey:process.env.BOXD_API_KEY,maxRetries:0,timeout:45000});
 let prepared,restore,restoreSubmitted=false;
 const owned=[];
 async function absent(id){try{await sdk.machines.get(id);return false;}catch(error){if(error.name==='NotFoundError')return true;throw error;}}
 async function removeOwned(id,name){
  try{const vm=await sdk.machines.get(id);if(vm.id!==id||vm.name!==name)throw new Error('OriginalIdentityMismatch');
   try{await sdk.machines.delete(id);}catch{if(!await absent(id))throw new Error('DeleteUncertain');}
   await wait(()=>absent(id),Boolean,90000);
  }catch(error){if(error.name!=='NotFoundError')throw error;}
 }
 async function exec(id,command,timeout=15000){const result=await sdk.machines.exec(id,{command,timeout});if(!result.success){report.lastCommandExitCode=result.exitCode;report.lastCommandStderr=result.stderr?.slice(-4096);throw new Error('ProofCommandFailed');}return result.stdout;}
 async function ready(id){await wait(async()=>{const vm=await sdk.machines.get(id);if(vm.status!=='running')return false;try{return(await sdk.machines.exec(id,{command:'true',timeout:5000})).success;}catch{return false;}},Boolean);}
 async function ping(id){const value=JSON.parse(await exec(id,['sudo','-n','-u','zuse','python3','/usr/local/lib/zuse/runtime-supervisor.py','ping']));
  if(value.version!==1||value.bound!==false||value.childAlive!==false||value.recoveryAlive!==false||typeof value.bootNonce!=='string')throw new Error('IdleResidentProofInvalid');
  residentExtensions(prepared?.manifest,value,requireExtensions);return value;}
 async function readyPing(id){return wait(async()=>{try{return await ping(id);}catch{return false;}},Boolean,30000);}
 await save();
 try{
  prepared=await stage('prepare-clean-builder',async()=>build({provider:'boxd',requireGithubCli:argv.includes('--require-github-cli'),bundle:bundlePath,output:output+'.builder.json',retainArtifact:true},await createBoxdBuilderAdapter(sdkPath,image,{nativeDefault})));
  report.builderReport=output+'.builder.json';
  if(prepared.status!=='prepared-uncertified'||!prepared.cleanupVerified||!prepared.artifactRetained||prepared.manifest?.rustRuntimeSupervisorVersion!==1)throw new Error('BuilderNotReady');
  report.snapshotName=prepared.snapshotName;report.artifactId=prepared.artifactId;report.runtimeSha256=prepared.runtimeSha256;report.providerSource=prepared.providerSource;
  // build() verified source deletion before any restore can allocate.
  report.sourceMachineAbsent=true;report.restoreName=prepared.runId+'-restore';restoreSubmitted=true;await save();
  restore=await stage('restore-captured-boot-owner',()=>sdk.machines.create({fromSnapshot:prepared.artifactId,name:report.restoreName,org:process.env.BOXD_ORG,isolated:true,config:{autoSuspendTimeout:0,autoDestroyTimeout:1200}}));
  report.restoreMachineId=restore.id;owned.push({id:restore.id,name:report.restoreName});await save();
  await stage('restored-command-ready',()=>ready(restore.id));
  if(nativeDefault){if((await exec(restore.id,'sudo -n systemctl is-enabled zuse-runtime-supervisor.service')).trim()!=='enabled')throw new Error('CapturedSupervisorServiceMissing');}
  else if((await sdk.machines.get(restore.id)).restartPolicy!=='always')throw new Error('CapturedRestartPolicyMissing');
  if(prepared.initialRuntimeBootstrap){
   report.initialRuntimeBootstrap=prepared.initialRuntimeBootstrap;await save();
   report.initialRuntimeBootstrapReceipt=await stage('initial-clean-snapshot-bootstrap',()=>proveFreshBoot({exec:(id,command)=>exec(id,command,60000)},restore.id,prepared.initialRuntimeBootstrap,{purpose:'publication-proof',operationId:prepared.runId,workspaceId:'image-proof-'+prepared.runId,generation:1,storageIncarnationId:'image-proof-'+prepared.runId},{onIntent:async value=>{report.initialRuntimeBootstrapIntent=value;await save();},onObservation:async value=>{report.initialRuntimeBootstrapObservation=value;await save();}}));await save();
  }
  const first=await stage('resident-after-restore',()=>readyPing(restore.id));
  report.githubCliProof=await stage('managed-github-cli-after-restore',()=>githubCliReadiness({exec},restore.id,prepared.manifest,{onObservation:async value=>{report.githubCliProof=value;await save();}}));await save();requireGithubCliProof(report.githubCliProof);
  await exec(restore.id,'true');const second=await ping(restore.id);
  if(first.bootNonce!==second.bootNonce||first.residentPid!==second.residentPid)throw new Error('ResidentDidNotSurviveExec');
  report.residentSurvivesSeparateExec=true;report.installedExtensions=residentExtensions(prepared.manifest,first,requireExtensions);
  const marker='retained-work-'+prepared.runId;
  await exec(restore.id,['sudo','-n','-u','zuse','sh','-c',`printf '%s' '${marker}' > /home/zuse/workspace/supervisor-dirty-work`]);
  await stage('cold-stop-retained-disk',async()=>{await sdk.machines.stop(restore.id);await wait(()=>sdk.machines.get(restore.id),vm=>vm.status==='stopped',90000);});
  await stage('cold-start-captured-boot-command',async()=>{await sdk.machines.start(restore.id);await ready(restore.id);});
  const cold=await stage('resident-after-cold-start',()=>readyPing(restore.id));
  if(cold.bootNonce===first.bootNonce)throw new Error('ColdBootResidentNotRenewed');
  report.capturedBootOwnerRestarts=true;report.coldBootExtensions=residentExtensions(prepared.manifest,cold,requireExtensions);
  const retained=await stage('retained-work-readback',()=>exec(restore.id,'sudo -n -u zuse cat /home/zuse/workspace/supervisor-dirty-work'));
  if(retained.trim()!==marker)throw new Error('DirtyWorkLost');
  report.dirtyWorkPreserved=true;report.status='mechanism-verified-uncertified';
 }catch(error){report.status='failed';report.errorCode=['ManagedGithubCliReceiptInvalid','ManagedGithubCliProofInvalid','ManagedGithubCliProofFailed'].includes(error?.message)?error.message:'SupervisorProofIncomplete';}
 finally{
  let clean=true;
  // This probe only owns a credential-free base: capture bounded resident
  // startup facts BEFORE deleting it, never arbitrary environment or chat logs.
  if(restore?.id){
   try{const facts=await sdk.machines.exec(restore.id,{command:'sudo -n systemctl show zuse-runtime-supervisor.service --property=ActiveState,SubState,Result,ExecMainStatus; sudo -n journalctl --no-pager -u zuse-runtime-supervisor.service -n 12',timeout:10000});
    report.residentStartup={exitCode:facts.exitCode,stdout:facts.stdout?.slice(-8192),stderr:facts.stderr?.slice(-2048)};
   }catch{report.residentStartupUnavailable=true;}
  }
  if(restoreSubmitted&&!restore){try{const vm=await sdk.machines.get(report.restoreName);if(vm.name!==report.restoreName)throw new Error('identity');owned.push({id:vm.id,name:report.restoreName});}catch(error){if(error.name!=='NotFoundError')clean=false;else clean=false;}}
  for(const vm of owned){try{await removeOwned(vm.id,vm.name);}catch{clean=false;}}
  if(!await cleanupUncertainBuilder(sdk,prepared,removeOwned))clean=false;
  const snapshot=prepared?.snapshotName;report.artifactRetained=Boolean(snapshot);
  const retain=Boolean(snapshot)&&retainCandidate&&clean&&report.status==='mechanism-verified-uncertified';
  if(retain){report.candidateArtifactStatus='uncertified-staging';report.cleanupScope='all-test-machines-absent-one-intended-staging-candidate-snapshot-retained';}
  if(snapshot&&!retain){try{try{await sdk.snapshots.delete(snapshot,{org:process.env.BOXD_ORG});}catch(error){if(error.name!=='NotFoundError')throw error;}
    await wait(async()=>{try{await sdk.snapshots.get(snapshot,{org:process.env.BOXD_ORG});return false;}catch(error){if(error.name==='NotFoundError')return true;throw error;}},Boolean,90000);report.artifactRetained=false;
  }catch{clean=false;}}
  report.cleanupVerified=clean;if(!clean)report.status='cleanup-required';await save();sdk.close();
 }
 console.log(JSON.stringify({status:report.status,cleanupVerified:report.cleanupVerified,artifactRetained:report.artifactRetained,agentAcceptanceVerified:false}));
 if(report.status!=='mechanism-verified-uncertified'||!report.cleanupVerified)process.exitCode=1;
}
if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href)main().catch(()=>{console.error('Supervisor probe failed; inspect private report.');process.exitCode=1;});
