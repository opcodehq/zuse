// Explicit external-process stub; real RPC/locking/path safety has Python tests.
export async function cleanSupervisorForSnapshot(_id,{operationId,runtimeSha256,legacyBuilder}){
 return{version:1,operationId,runtimeSha256,legacyBuilder,originalResidentExited:true,supervisorLockReleased:true,runtimeDataAbsent:true,enabledBootServiceRetained:true,bootServiceSha256:'e'.repeat(64),atomicStopAcknowledged:!legacyBuilder,removed:['supervisor.json','supervisor.lock']};
}
export async function quiesceCleanParent(_id,r){
 const receipt=await cleanSupervisorForSnapshot(_id,r);
 return{...receipt,residentPid:42,residentStartTicks:123,residentUid:10001,controlGroup:'/system.slice/zuse-runtime-supervisor.service',unitHadOnlyResident:true,frozenCleanVerified:true,pidfdRetirement:true,retiredWithoutPathCleanup:true,visibleLockProtected:true,parentLineageSha256:r.parentLineageSha256,parentImageProof:{status:'quiescence-image-verified',parentArtifactId:r.parentArtifactId,parentLineageSha256:r.parentLineageSha256,parentRuntimeSha256:r.runtimeSha256,runtimeSha256:r.targetRuntimeSha256,harnessRelease:r.harnessRelease,completeCodexAssetsVerified:true,sourceClean:true}};
}
