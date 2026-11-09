// Capture only a stopped, unbound builder with no persisted supervision state.
import{readFile}from'node:fs/promises';
import{createHash}from'node:crypto';
const quote=v=>`'${v.replaceAll("'","'\\''")}'`;
export function verifyHygiene(receipt,{operationId,runtimeSha256,legacyBuilder=false,disabledBootService=false}){
 if(receipt?.version!==1||receipt.operationId!==operationId||receipt.runtimeSha256!==runtimeSha256||receipt.originalResidentExited!==true||receipt.supervisorLockReleased!==true||receipt.runtimeDataAbsent!==true||(disabledBootService?(receipt.enabledBootServiceRetained!==false||receipt.disabledBootServiceRetained!==true):receipt.enabledBootServiceRetained!==true)||!/^[a-f0-9]{64}$/.test(receipt.bootServiceSha256??'')||receipt.legacyBuilder!==legacyBuilder||(!legacyBuilder&&receipt.atomicStopAcknowledged!==true)||!Array.isArray(receipt.removed)||!receipt.removed.includes('supervisor.lock')||receipt.removed.some(v=>!['supervisor.lock','supervisor.json','control.sock'].includes(v)))throw new Error('CleanSupervisorHygieneRejected');
 return receipt;
}
export async function cleanBuilderSupervisor({adapter,id,rootShell,operationId,runtimeSha256,legacyBuilder=false,disabledBootService=false,record,persist}){
 const source=await readFile(new URL('./base-supervisor-hygiene.py',import.meta.url));
 const sourceSha256=createHash('sha256').update(source).digest('hex');
 record.baseHygieneSourceSha256=sourceSha256;await persist();
 if(disabledBootService&&legacyBuilder)throw Error('CleanSupervisorHygieneRejected');const request={operationId,runtimeSha256,legacyBuilder,...(disabledBootService?{disabledBootService:true}:{})};
 const command='/usr/bin/python3 -I -B -c '+quote(source.toString('utf8'))+' --runtime-sha256 '+quote(runtimeSha256)+' --operation-id '+quote(operationId)+' --report-only'+(legacyBuilder?' --legacy-builder':'')+(disabledBootService?' --disabled-boat-owner':'');
 const receipt=adapter.cleanSupervisorForSnapshot?await adapter.cleanSupervisorForSnapshot(id,request):JSON.parse(await adapter.exec(id,`${rootShell} ${quote(command)}`));
 record[legacyBuilder?'parentSupervisorHygiene':'supervisorHygiene']=receipt;await persist();return verifyHygiene(receipt,request);
}
