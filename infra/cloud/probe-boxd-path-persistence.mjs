#!/usr/bin/env node
// Operator-only diagnostic. No harness, credentials, model or project inputs.
import{readFile,writeFile,mkdir}from'node:fs/promises';
import{resolve,dirname}from'node:path';
import{pathToFileURL}from'node:url';
import{createHash,randomUUID}from'node:crypto';
export const PATHS=['/usr/local/bin/gh','/usr/local/bin/zuse-snapshot-path-marker'];
export const BODY='#!/bin/sh\nprintf "%s\\n" "zuse-snapshot-marker-only"\n';
const sha=b=>createHash('sha256').update(b).digest('hex');
export function command(write=false){
 const code=`import os,stat,json,hashlib\npaths=${JSON.stringify(PATHS)}\nbody=${JSON.stringify(BODY)}.encode()\ndef fact(p):\n try:\n  s=os.lstat(p);r=dict(exists=True,uid=s.st_uid,gid=s.st_gid,mode=oct(stat.S_IMODE(s.st_mode)),inode=s.st_ino,dev=s.st_dev,symlink=stat.S_ISLNK(s.st_mode),regular=stat.S_ISREG(s.st_mode),bytes=s.st_size)\n  if p in paths and stat.S_ISREG(s.st_mode):\n   fd=os.open(p,os.O_RDONLY|os.O_NOFOLLOW)\n   try:r['sha256']=hashlib.sha256(os.read(fd,4096)).hexdigest()\n   finally:os.close(fd)\n  return r\n except FileNotFoundError:return dict(exists=False)\nassert os.geteuid()==0\n${write?`for p in paths:\n if os.path.lexists(p):\n  s=os.lstat(p);assert stat.S_ISREG(s.st_mode) and s.st_uid==0 and not stat.S_IMODE(s.st_mode)&0o022\n fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_TRUNC|os.O_NOFOLLOW,0o755)\n try:os.write(fd,body);os.fchmod(fd,0o755);os.fchown(fd,0,0);os.fsync(fd)\n finally:os.close(fd)\nfd=os.open('/usr/local/bin',os.O_DIRECTORY|os.O_RDONLY|os.O_NOFOLLOW)\ntry:os.fsync(fd)\nfinally:os.close(fd)\nos.sync()\n`:''}print(json.dumps(dict(uid=os.geteuid(),bootId=open('/proc/sys/kernel/random/boot_id').read().strip(),facts={p:fact(p) for p in ['/', '/usr','/usr/local','/usr/local/bin','/usr/bin/gh']+paths})))`;
 return ['sudo','-n','/usr/bin/python3','-I','-B','-c',code];
}
export function validateWritten(facts){for(const path of PATHS){const f=facts.facts?.[path];if(!f?.regular||f.symlink||f.uid!==0||f.gid!==0||f.mode!=='0o755'||f.sha256!==sha(BODY))throw new Error('MarkerWriteReadbackInvalid');}return true;}
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function wait(task,predicate,timeout=60000){const end=Date.now()+timeout;while(Date.now()<end){const v=await task();if(predicate(v))return v;await sleep(500);}throw new Error('DiagnosticObservationDeadline');}
const notFound=e=>e?.name==='NotFoundError';
export async function run(sdk,r,save,{waitFor=wait,delay=sleep}={}){
 const absent=async id=>{try{await sdk.machines.get(id);return false;}catch(e){if(notFound(e))return true;throw e;}};
 async function create(key,args){r[key+'Submitted']=true;await save();let vm;try{vm=await sdk.machines.create(args);}catch{try{vm=await sdk.machines.get(args.name);}catch{throw new Error('OriginalAllocationUncertain');}}if(vm.name!==args.name)throw new Error('OriginalIdentityMismatch');r[key]={id:vm.id,name:vm.name};await save();return vm;}
 async function exec(id,cmd){const result=await sdk.machines.exec(id,{command:cmd,timeout:10000});if(!result.success)throw new Error('DiagnosticCommandFailed');return JSON.parse(result.stdout);}
 async function ready(id){await waitFor(async()=>{const vm=await sdk.machines.get(id);if(vm.status!=='running')return false;try{return(await sdk.machines.exec(id,{command:['/bin/true'],timeout:3000})).success;}catch{return false;}},Boolean);}
 async function remove(key){const vm=r[key];if(!vm)return !r[key+'Submitted'];let found;try{found=await sdk.machines.get(vm.id);}catch(e){if(notFound(e))return true;throw e;}if(found.id!==vm.id||found.name!==vm.name)throw new Error('OriginalIdentityMismatch');try{await sdk.machines.delete(vm.id);}catch{if(!await absent(vm.id))throw new Error('OriginalDeleteUncertain');}await waitFor(()=>absent(vm.id),Boolean);return true;}
 try{
  const builder=await create('builder',{name:r.builderName,org:r.org,isolated:true,config:{vcpu:1,memory:'4G',autoSuspendTimeout:0,autoDestroyTimeout:600}});
  await ready(builder.id);r.before=await exec(builder.id,command());await save();
  r.written=await exec(builder.id,command(true));validateWritten(r.written);await save();
  r.snapshotSubmitted=true;await save();try{r.capture=await sdk.snapshots.create(builder.id,r.snapshotName);}catch{r.captureUncertain=true;await save();}
  const snapshot=await waitFor(async()=>{try{return await sdk.snapshots.get(r.snapshotName,{org:r.org});}catch(e){if(notFound(e))return null;throw e;}},v=>v?.status==='ready');
  if(snapshot.name!==r.snapshotName)throw new Error('OriginalSnapshotIdentityMismatch');r.snapshot={id:snapshot.id,name:snapshot.name,version:snapshot.version};await save();
  r.afterCapture=await exec(builder.id,command());await save();
  r.builderAbsent=await remove('builder');await save();if(!r.builderAbsent)throw new Error('BuilderCleanupRequired');
  const restore=await create('restore',{fromSnapshot:r.snapshot.id,name:r.restoreName,org:r.org,isolated:true,config:{autoSuspendTimeout:0,autoDestroyTimeout:600}});
  await ready(restore.id);r.restored=[];for(const delayMs of [0,1000,9000]){if(delayMs)await delay(delayMs);r.restored.push(await exec(restore.id,command()));await save();}
  r.status='diagnostic-observed';
 }catch(e){r.status='failed';r.errorCode=['OriginalAllocationUncertain','OriginalIdentityMismatch','MarkerWriteReadbackInvalid','OriginalSnapshotIdentityMismatch','BuilderCleanupRequired','DiagnosticObservationDeadline','DiagnosticCommandFailed'].includes(e.message)?e.message:'DiagnosticFailed';}
 finally{
  let clean=true;for(const key of ['restore','builder'])try{if(!r[key]&&r[key+'Submitted']){const name=r[key+'Name'];try{const vm=await sdk.machines.get(name);if(vm.name!==name)throw new Error('identity');r[key]={id:vm.id,name:vm.name};}catch{clean=false;}}if(!await remove(key))clean=false;}catch{clean=false;}
  if(r.snapshotSubmitted)try{const snapshot=await sdk.snapshots.get(r.snapshotName,{org:r.org});if(snapshot.name!==r.snapshotName||(r.snapshot&&snapshot.id!==r.snapshot.id))throw new Error('identity');await sdk.snapshots.delete(r.snapshotName,{org:r.org});await waitFor(async()=>{try{await sdk.snapshots.get(r.snapshotName,{org:r.org});return false;}catch(e){if(notFound(e))return true;throw e;}},Boolean);}catch(e){if(!notFound(e)||!r.snapshot)clean=false;}
  r.cleanupVerified=clean;if(!clean)r.status='cleanup-required';await save();
 }
 return r;
}
async function main(){
 const a=process.argv.slice(2),get=n=>a[a.indexOf(n)+1];if(!a.includes('--output')||!a.includes('--boxd-sdk'))throw new Error('ArgumentsRequired');
 const source=await readFile(new URL(import.meta.url));const plan={schema:1,sourceSha256:sha(source),sdkVersion:'0.2.14',maxActiveMachines:1,builderTtlSeconds:600,restoreTtlSeconds:600,paths:PATHS,markerSha256:sha(BODY),writeSync:true,harnessUploadBytes:0,modelRequests:false,credentials:false,snapshotRetained:false};
 if(!a.includes('--live')){console.log(JSON.stringify(plan));return;}
 if(!a.includes('--approved-plan'))throw new Error('ApprovedPlanRequired');const approved=JSON.parse(await readFile(get('--approved-plan'),'utf8'));if(JSON.stringify(approved)!==JSON.stringify(plan))throw new Error('ApprovedPlanMismatch');
 const output=resolve(get('--output'));await mkdir(dirname(output),{recursive:true});const id=randomUUID().replaceAll('-','');const r={...plan,runId:id,status:'incomplete',org:process.env.BOXD_ORG,builderName:'zuse-path-builder-'+id,restoreName:'zuse-path-restore-'+id,snapshotName:'zuse-path-snapshot-'+id,cleanupVerified:false};let claimed=false;const save=async()=>{await writeFile(output,JSON.stringify(r,null,2)+'\n',{mode:0o600,flag:claimed?'w':'wx'});claimed=true;};await save();const v=await import(pathToFileURL(resolve(get('--boxd-sdk'))).href);if(v.VERSION!=='0.2.14')throw new Error('PinnedSdkRequired');const sdk=new v.Boxd({apiKey:process.env.BOXD_API_KEY,maxRetries:0,timeout:30000});try{await run(sdk,r,save);}finally{sdk.close();}console.log(JSON.stringify({status:r.status,cleanupVerified:r.cleanupVerified,output}));if(!r.cleanupVerified||r.status!=='diagnostic-observed')process.exitCode=1;
}
if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href)main().catch(()=>{console.error('Path diagnostic rejected; inspect private original report.');process.exitCode=1;});
