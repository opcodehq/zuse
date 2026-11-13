// The same protected image bootstrap is used by product dispatch and proof.
import{readFile}from'node:fs/promises';import{createHash}from'node:crypto';
const sha=b=>createHash('sha256').update(b).digest('hex');const q=s=>`'${s.replaceAll("'","'\\''")}'`;
const fields=['runtimeSha256','supervisorSha256','serviceSha256','manifestSha256','helperSha256'];
// Optional sixth pin: bases with the root runtime swap helper (runtime-update.py).
const UPDATE_HELPER='runtimeUpdateHelperSha256';
const pinFields=spec=>spec&&UPDATE_HELPER in spec?[...fields,UPDATE_HELPER]:fields;
function canonical(value){return JSON.stringify(value,(_k,v)=>v&&typeof v==='object'&&!Array.isArray(v)?Object.fromEntries(Object.keys(v).sort().map(k=>[k,v[k]])):v);}
export function validFreshBootSpec(spec){return spec?.version===1&&['stopped-memory-snapshot-v1','post-hydration-first-start-v1'].includes(spec.mode)&&Object.keys(spec).length===2+pinFields(spec).length&&pinFields(spec).every(k=>/^[a-f0-9]{64}$/.test(spec[k]??''));}
export async function freshBootSpec(adapter,id,bundle){
 const helper=bundle.files.find(f=>f.path==='managed-base-fresh-boot.py');if(bundle.managedBaseFreshBootVersion!==1||!helper)throw Error('FreshBootBundleRequired');
 const updateHelper=bundle.managedBaseRuntimeUpdateVersion===1?bundle.files.find(f=>f.path==='runtime-update.py'):null;if(bundle.managedBaseRuntimeUpdateVersion===1&&!updateHelper)throw Error('RuntimeUpdateBundleRequired');
 const program="import hashlib,json,os,stat; paths={'runtimeSha256':'/usr/local/bin/zuse-cloud-runtime','supervisorSha256':'/usr/local/lib/zuse/runtime-supervisor.py','serviceSha256':'/etc/systemd/system/zuse-runtime-supervisor.service','manifestSha256':'/opt/zuse/preparation/status/manifest.json','helperSha256':'/usr/local/lib/zuse/managed-base-fresh-boot.py'"+(updateHelper?",'runtimeUpdateHelperSha256':'/usr/local/libexec/zuse/runtime-update.py'":'')+"}; r={'version':1,'mode':'stopped-memory-snapshot-v1'}\nfor key,path in paths.items():\n f=os.open(path,os.O_RDONLY|os.O_NOFOLLOW);m=os.fstat(f);assert stat.S_ISREG(m.st_mode) and m.st_nlink==1 and m.st_uid in ({0,10001} if key=='manifestSha256' else {0}) and not m.st_mode&0o022;h=hashlib.sha256()\n with os.fdopen(f,'rb') as s:\n  while b:=s.read(1048576):h.update(b)\n  n=os.fstat(s.fileno());assert (m.st_dev,m.st_ino,m.st_size,m.st_mtime_ns,m.st_ctime_ns)==(n.st_dev,n.st_ino,n.st_size,n.st_mtime_ns,n.st_ctime_ns)\n r[key]=h.hexdigest()\nprint(json.dumps(r))";
 const value=JSON.parse(await adapter.exec(id,'sudo -n /usr/bin/python3 -I -B -c '+q(program)));value.mode=bundle.managedBaseFreshBootMode??'stopped-memory-snapshot-v1';
 if(!validFreshBootSpec(value)||value.runtimeSha256!==bundle.runtimeSha256||value.helperSha256!==helper.sha256||value.supervisorSha256!==bundle.files.find(f=>f.path==='runtime-supervisor.py')?.sha256||(updateHelper?value[UPDATE_HELPER]!==updateHelper.sha256:UPDATE_HELPER in value))throw Error('FreshBootImagePinMismatch');return value;
}
export async function freshBootCommand(spec,binding){
 if(!validFreshBootSpec(spec))throw Error('FreshBootSpecInvalid');
 const allowed=['purpose','operationId','workspaceId','generation','storageIncarnationId','sourceMachineId'];if(Object.keys(binding).length!==allowed.length||Object.keys(binding).some(k=>!allowed.includes(k)))throw Error('FreshBootBindingInvalid');
 const request={version:1,provider:spec.mode==='post-hydration-first-start-v1'?'boat':'boxd',...binding,imagePins:Object.fromEntries(pinFields(spec).map(k=>[k,spec[k]]))};
 if(!['initial-allocation','initial-restore','publication-proof','cache-preparation',...(request.provider==='boat'?['retained-resume']:[])].includes(request.purpose)||!Number.isSafeInteger(request.generation)||request.generation<1||['operationId','workspaceId','storageIncarnationId','sourceMachineId'].some(k=>!/^[A-Za-z0-9_.:-]{1,180}$/.test(request[k]??'')))throw Error('FreshBootBindingInvalid');
 const loader=await readFile(new URL('./managed-base-fresh-boot-loader.py',import.meta.url),'utf8');
 return {request,requestSha256:sha(Buffer.from(canonical(request))),command:'sudo -n /usr/bin/python3 -I -B -c '+q(loader)+' '+q(spec.helperSha256)+' '+q(Buffer.from(canonical(request)).toString('base64'))};
}
export function verifyFreshBootReceipt(request,receipt){
 if(receipt?.status!=='ready'||Object.keys(request).some(k=>canonical(request[k])!==canonical(receipt[k]))||receipt.requestSha256!==sha(Buffer.from(canonical(request)))||receipt.resident?.ipcVerified!==true||(request.purpose==='retained-resume'?(receipt.resident?.unbound!==false||receipt.resident?.identityVerified!==true||typeof receipt.resident?.childAlive!=='boolean'):(receipt.resident?.unbound!==true||receipt.resident?.childAlive!==false))||receipt.resident?.recoveryAlive!==false||!Number.isSafeInteger(receipt.resident?.residentPid)||receipt.resident.residentPid<=1||!Number.isSafeInteger(receipt.resident?.residentStartTicks)||receipt.resident.residentStartTicks<1||!/^[a-f0-9]{64}$/.test(receipt.resident?.bootNonceSha256??''))throw Error('FreshBootReceiptInvalid');return receipt;
}
export async function proveFreshBoot(adapter,id,spec,binding,{onIntent=async()=>{},onObservation=async()=>{},sleep=ms=>new Promise(r=>setTimeout(r,ms)),deadlineMs=30000}={}){
 const prepared=await freshBootCommand(spec,{...binding,sourceMachineId:id});await onIntent({request:prepared.request,requestSha256:prepared.requestSha256});
 const deadline=Date.now()+deadlineMs;
 for(;;){
  const receipt=JSON.parse(await adapter.exec(id,prepared.command));await onObservation(receipt);
  if(receipt.status==='ready')return {request:prepared.request,receipt:verifyFreshBootReceipt(prepared.request,receipt)};
  if(!['starting','pending'].includes(receipt.status)||receipt.requestSha256!==prepared.requestSha256||Object.keys(prepared.request).some(k=>canonical(prepared.request[k])!==canonical(receipt[k])))throw Error('FreshBootReceiptInvalid');
  if(Date.now()>=deadline)throw Error('FreshBootObservationPending');await sleep(250);
 }
}

export async function disableBoatCapture(adapter,id,spec,binding,stopReceipt,{onIntent=async()=>{},onReceipt=async()=>{}}={}){
 if(spec.mode!=='post-hydration-first-start-v1')throw Error('FreshBootSpecInvalid');
 const prepared=await freshBootCommand(spec,{...binding,sourceMachineId:id});
 const payload=Buffer.from(canonical({request:prepared.request,stopReceipt})).toString('base64');
 const loader=await readFile(new URL('./managed-base-fresh-boot-loader.py',import.meta.url),'utf8');
 // Same verified loader, different static entrypoint ONLY for clean factory
 // disable. No task/resume path can call this through the canonical Request.
 const factory=loader.replace("sys.argv=[str(path),'--request-base64',sys.argv[2]]", "sys.argv=[str(path),'--capture-disable-base64',sys.argv[2]]");
 if(factory===loader)throw Error('FreshBootLoaderInvalid');
 await onIntent({request:prepared.request,atomicStopReceiptSha256:sha(Buffer.from(canonical(stopReceipt)))});
 const receipt=JSON.parse(await adapter.exec(id,'sudo -n /usr/bin/python3 -I -B -c '+q(factory)+' '+q(spec.helperSha256)+' '+q(payload)));
 await onReceipt(receipt);
 if(receipt?.version!==1||receipt.sourceMachineId!==id||receipt.requestSha256!==prepared.requestSha256||receipt.operationId!==prepared.request.operationId||receipt.runtimeSha256!==spec.runtimeSha256||receipt.helperSha256!==spec.helperSha256||receipt.serviceSha256!==spec.serviceSha256||receipt.unitDisabled!==true||receipt.noRuntimeData!==true||receipt.atomicStopAcknowledged!==true||receipt.atomicStopReceiptSha256!==sha(Buffer.from(canonical(stopReceipt))))throw Error('FreshBootCaptureDisableInvalid');return receipt;
}
