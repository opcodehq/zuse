// Small strict parent admission before uploading the new runtime or native payloads.
import{readFile}from'node:fs/promises';import{join}from'node:path';import{createHash}from'node:crypto';
const q=v=>`'${v.replaceAll("'","'\\''")}'`;
const sha=v=>createHash('sha256').update(v).digest('hex');
export const VERIFIED_PROVIDER_ANCESTORS=['/opt','/usr/local','/usr/local/bin','/usr/local/lib'];
export function parentPreflightCommand(source,input,expected,bundleSha,{quiescenceImage=false,operationId}={}){
 return '/usr/bin/python3 -I -B -c '+q(source)+' --input '+q(input)+' --sha256 '+q(expected)+' --bundle-sha256 '+q(bundleSha)+' --parent-preflight'+(quiescenceImage?' --quiescence-image':'')+(operationId?' --operation-id '+q(operationId):'');
}
export function repairableMetadata(receipt){return receipt?.status==='rejected'&&receipt.code==='parent_metadata_not_protected'&&Array.isArray(receipt.failures)&&receipt.failures.length>0&&receipt.failures.every(f=>VERIFIED_PROVIDER_ANCESTORS.includes(f.path)&&f.uid===1000&&f.gid===1000&&f.mode==='0o755'&&f.symlink===false);}
export async function uploadParentPreflight({adapter,id,runId,bundle,parent,rootShell,uploadExactChunk,record,persist,prepareQuiescence=false}){
 const root='/tmp/zuse-parent-preflight';await adapter.exec(id,'test ! -e '+root+' && mkdir -p '+root);
 const names=['bundle.json','harness-release.py','harness-manifest.json'];let bytes=0;
 for(const name of names){const data=await readFile(join(bundle,name));const r=await uploadExactChunk(adapter,id,runId,root+'/'+name,data);bytes+=data.length;record.uploadReceipts.push({path:root+'/'+name,...r});}
 const receipt=await uploadExactChunk(adapter,id,runId,root+'/derivation.json',parent.bytes);bytes+=parent.bytes.length;record.uploadReceipts.push({path:root+'/derivation.json',...receipt});record.preflightUploadedBytes=bytes;record.uploadedBytes+=bytes;await persist();
 if(prepareQuiescence){
  for(const[name,source]of[['parent-validator.py','managed-base-derive.py'],['parent-hygiene.py','base-supervisor-hygiene.py'],['parent-quiescence.py','clean-parent-quiescence.py']]){
   const body=await readFile(new URL('./'+source,import.meta.url));const pin=await uploadExactChunk(adapter,id,runId,root+'/'+name,body);bytes+=body.length;record.uploadReceipts.push({path:root+'/'+name,...pin});record.uploadedBytes+=body.length;
  }
  record.preflightUploadedBytes=bytes;await persist();
 }
 await adapter.exec(id,`${rootShell} ${q('test ! -e /var/lib/zuse/base-preflight && mv -- /tmp/zuse-parent-preflight /var/lib/zuse/base-preflight && chown -hR 0:0 -- /var/lib/zuse/base-preflight && chmod -R go-w -- /var/lib/zuse/base-preflight && chmod 0700 /var/lib/zuse/base-preflight')}`);
 return bytes;
}
export async function inspectParentPreflight({adapter,id,parent,rootShell,bundle,record,persist,quiescenceImage=false,operationId}){
 const source=await readFile(new URL('./managed-base-derive.py',import.meta.url)),bundleBytes=await readFile(join(bundle,'bundle.json')),bundleSha=sha(bundleBytes),manifest=JSON.parse(bundleBytes);
 record.parentPreflightSourceSha256=sha(source);await persist();
 const command=parentPreflightCommand(source.toString('utf8'),'/var/lib/zuse/base-preflight',parent.sha256,bundleSha,{quiescenceImage,operationId});
 const result=JSON.parse(await adapter.exec(id,`${rootShell} ${q(command)}`));
 const admitted=quiescenceImage?'quiescence-image-verified':'passed';
 if(result.version!==1||![admitted,'rejected'].includes(result.status))throw new Error('ParentPreflightReceiptInvalid');
 if(result.status===admitted&&(result.parentArtifactId!==parent.artifactId||result.parentLineageSha256!==parent.sha256||result.completeCodexAssetsVerified!==true||result.sourceClean!==true||result.runtimeSha256!==manifest.runtimeSha256||result.harnessRelease!==manifest.harnessRelease||result.parentRuntimeSha256!==parent.parentRuntimeSha256))throw new Error('ParentPreflightReceiptInvalid');
 return result;
}
export async function removeParentPreflight(adapter,id,rootShell){
 const root='/var/lib/zuse/base-preflight';const optional=['parent-stopped.json','parent-validator.py','parent-hygiene.py','parent-quiescence.py','parent-quiescence-result.json','parent-quiescence-result.json.tmp'].map(n=>'if test -e '+root+'/'+n+'; then rm -- '+root+'/'+n+'; fi').join(' && ');await adapter.exec(id,`${rootShell} ${q('rm -- '+['bundle.json','harness-release.py','harness-manifest.json','derivation.json'].map(n=>root+'/'+n).join(' ')+' && '+optional+' && rmdir '+root)}`);
}
export async function persistStoppedParent({adapter,id,rootShell,parent,bundle,operationId,hygiene}){
 const receipt={version:1,operationId,machineId:id,parentLineageSha256:parent.sha256,targetBundleSha256:sha(await readFile(join(bundle,'bundle.json'))),hygiene};
 const code='import os,sys; p="/var/lib/zuse/base-preflight/parent-stopped.json"; f=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600); body=sys.argv[1].encode(); os.write(f,body); os.fsync(f); os.close(f)';
 await adapter.exec(id,`${rootShell} ${q('/usr/bin/python3 -I -B -c '+q(code)+' '+q(JSON.stringify(receipt)))}`);
 return receipt;
}
