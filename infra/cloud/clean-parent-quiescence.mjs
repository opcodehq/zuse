// Factory-only legacy-parent retirement. Ordinary task IPC and final atomic stop
// remain separate; no cached parent receipt can attest the new image's resident.
import{readFile}from'node:fs/promises';
import{join}from'node:path';
import{createHash}from'node:crypto';
import{verifyHygiene}from'./base-supervisor-hygiene.mjs';
const sha=v=>createHash('sha256').update(v).digest('hex');
const q=v=>`'${v.replaceAll("'","'\\''")}'`;
export async function quiesceCleanParent({adapter,id,rootShell,operationId,parent,bundle,record,persist}){
 const[program,validator,hygiene,bundleBytes]=await Promise.all(['clean-parent-quiescence.py','managed-base-derive.py','base-supervisor-hygiene.py'].map(n=>readFile(new URL('./'+n,import.meta.url))).concat([readFile(join(bundle,'bundle.json'))]));
 const manifest=JSON.parse(bundleBytes),pins={program:sha(program),validator:sha(validator),hygiene:sha(hygiene)};
 const disabled=parent.docs?.parentBundle?.managedBaseFreshBootMode==='post-hydration-first-start-v1';
 const request={operationId,runtimeSha256:parent.parentRuntimeSha256,legacyBuilder:!disabled,parentLineageSha256:parent.sha256,targetBundleSha256:sha(bundleBytes),parentArtifactId:parent.artifactId,targetRuntimeSha256:manifest.runtimeSha256,harnessRelease:manifest.harnessRelease};
 record.parentQuiescenceSources=pins;await persist();
 const bootstrap="import os,stat,hashlib,sys; p='/var/lib/zuse/base-preflight/parent-quiescence.py'; f=os.open(p,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC); m=os.fstat(f); assert stat.S_ISREG(m.st_mode) and m.st_uid==0 and m.st_nlink==1 and not m.st_mode&0o022 and m.st_size<=131072; body=os.read(f,131073); os.close(f); assert hashlib.sha256(body).hexdigest()==sys.argv[1]; sys.argv=[p]+sys.argv[2:]; exec(compile(body,p,'exec'),{'__name__':'__main__','__file__':p})";
 const command='/usr/bin/python3 -I -B -c '+q(bootstrap)+' '+q(pins.program)+' --operation-id '+q(operationId)+' --lineage-sha256 '+q(parent.sha256)+' --bundle-sha256 '+q(request.targetBundleSha256)+' --validator-sha256 '+q(pins.validator)+' --hygiene-sha256 '+q(pins.hygiene)+' --report-only';
 let receipt;
 if(adapter.quiesceCleanParent)receipt=await adapter.quiesceCleanParent(id,request);
 else if(adapter.prepare){
  // Full image validation may exceed a synchronous command budget. Submit ONE
  // tracked original command, persist its receipt, and never resubmit on loss.
  const path='/var/lib/zuse/base-preflight/parent-quiescence-result.json';
  const promote="import os; p='"+path+"'; f=os.open(p+'.tmp',os.O_RDONLY|os.O_NOFOLLOW); os.fsync(f); os.close(f); os.rename(p+'.tmp',p); d=os.open('/var/lib/zuse/base-preflight',os.O_DIRECTORY); os.fsync(d); os.close(d)";
  const script='umask 077; test ! -e '+q(path)+' && test ! -e '+q(path+'.tmp')+' && '+command+' > '+q(path+'.tmp')+' && /usr/bin/python3 -I -B -c '+q(promote);
  try{
   await adapter.prepare(id,`${rootShell} ${q(script)}`,async value=>{record.parentQuiescenceProcess=value;await persist();});
   receipt=JSON.parse(await adapter.exec(id,`${rootShell} ${q('cat -- '+q(path))}`));
  }catch(error){record.inspectionRequired=true;record.parentQuiescenceObservation='original-command-only';await persist();throw error;}
 }else receipt=JSON.parse(await adapter.exec(id,`${rootShell} ${q(command)}`));
 record.parentSupervisorHygiene=receipt;await persist();
 if(disabled){
  const proof=receipt.parentImageProof;
  if(receipt.version!==1||receipt.operationId!==operationId||receipt.runtimeSha256!==parent.parentRuntimeSha256||receipt.parentMode!=='post-hydration-first-start-v1'||receipt.alreadyDisabledParent!==true||receipt.noResidentPresent!==true||receipt.noAuthorityPresent!==true||receipt.startedOrStoppedProcess!==false||receipt.legacyBuilder!==false||receipt.atomicStopAcknowledged!==false||receipt.enabledBootServiceRetained!==false||receipt.disabledBootServiceRetained!==true||receipt.bootServiceSha256!==parent.docs.builder.initialRuntimeBootstrap.serviceSha256||receipt.parentLineageSha256!==parent.sha256||proof?.status!=='quiescence-image-verified'||proof.parentMode!=='post-hydration-first-start-v1'||proof.noResidentPresent!==true||proof.noAuthorityPresent!==true||proof.parentArtifactId!==parent.artifactId||proof.parentLineageSha256!==parent.sha256||proof.parentRuntimeSha256!==parent.parentRuntimeSha256||proof.harnessRelease!==manifest.harnessRelease||proof.runtimeSha256!==manifest.runtimeSha256||proof.completeCodexAssetsVerified!==true||proof.sourceClean!==true)throw new Error('CleanDisabledParentRejected');
  return receipt;
 }
 verifyHygiene(receipt,request);
 if(receipt.retiredWithoutPathCleanup!==true||receipt.visibleLockProtected!==true||receipt.unitHadOnlyResident!==true||receipt.frozenCleanVerified!==true||receipt.pidfdRetirement!==true||receipt.residentUid!==10001||typeof receipt.controlGroup!=='string'||!receipt.controlGroup.endsWith('/zuse-runtime-supervisor.service')||receipt.parentLineageSha256!==parent.sha256||receipt.parentImageProof?.status!=='quiescence-image-verified'||receipt.parentImageProof.parentArtifactId!==parent.artifactId||receipt.parentImageProof.parentLineageSha256!==parent.sha256||receipt.parentImageProof.parentRuntimeSha256!==parent.parentRuntimeSha256||receipt.parentImageProof.harnessRelease!==manifest.harnessRelease||receipt.parentImageProof.runtimeSha256!==manifest.runtimeSha256||receipt.parentImageProof.completeCodexAssetsVerified!==true||receipt.parentImageProof.sourceClean!==true)throw new Error('CleanParentQuiescenceRejected');
 return receipt;
}
