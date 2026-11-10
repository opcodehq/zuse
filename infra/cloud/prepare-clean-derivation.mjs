#!/usr/bin/env node
// Local artifact preparation only. Never contacts a provider or changes a parent.
import {readFile,writeFile,mkdir,link,copyFile,open} from 'node:fs/promises';
import {resolve,join,dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {verifyBundle,requireFreshPublicationReport} from './build-managed-base.mjs';
import {makeCleanLineage,loadCleanLineage,REUSED_HARNESS_FILES} from './clean-base-lineage.mjs';
const sha=v=>createHash('sha256').update(v).digest('hex');
async function pin(path,name){const handle=await open(path,'r');try{const stat=await handle.stat(),hash=createHash('sha256');for await(const chunk of handle.createReadStream())hash.update(chunk);return{path:name,bytes:stat.size,sha256:hash.digest('hex')};}finally{await handle.close();}}
export async function prepareCleanDerivation({source,output,proof,builder,parentBundle,cleanup}) {
 await requireFreshPublicationReport(output);const original=await verifyBundle(source);
 const lineage=await makeCleanLineage({proof,builder,parentBundle,cleanup});
 await mkdir(output,{mode:0o700});const here=dirname(fileURLToPath(import.meta.url));
 const files=[];
 for(const file of original.files){
  if(file.path==='managed-base-derive.py')continue;
  const target=join(output,file.path);
  if(file.path==='managed-base-install.sh')await copyFile(join(here,file.path),target);
  else await link(join(source,file.path),target); // Never chmod/write a captured inode.
  files.push(await pin(target,file.path));
 }
 await copyFile(join(here,'managed-base-derive.py'),join(output,'managed-base-derive.py'));
 files.push(await pin(join(output,'managed-base-derive.py'),'managed-base-derive.py'));
 const bundle={...original,managedBaseDerivationVersion:1,files};
 bundle.releaseId='zuse-base-'+sha(JSON.stringify(bundle)).slice(0,32);
 await writeFile(join(output,'bundle.json'),JSON.stringify(bundle,null,2)+'\n',{mode:0o600,flag:'wx'});
 const text=JSON.stringify(lineage,null,2)+'\n',lineagePath=join(output,'parent-lineage.json');
 await writeFile(lineagePath,text,{mode:0o600,flag:'wx'});
 await verifyBundle(output);const parent=await loadCleanLineage(lineagePath,sha(text),bundle);
 const parentOperatorBytes=(await Promise.all(['managed-base-derive.py','base-supervisor-hygiene.py','clean-parent-quiescence.py'].map(n=>readFile(join(here,n))))).reduce((n,b)=>n+b.length,0);
 const plan={status:'read-only-publication-plan',provider:'boat',bundle:output,bundleSha256:sha(await readFile(join(output,'bundle.json'))),runtimeSha256:bundle.runtimeSha256,harnessRelease:bundle.harnessRelease,parentArtifactId:parent.artifactId,parentProviderArtifactVersion:parent.providerArtifactVersion,parentLineage:lineagePath,parentLineageSha256:parent.sha256,parentRuntimeSha256:parent.parentRuntimeSha256,fullBundleBytes:files.reduce((n,f)=>n+f.bytes,0)+(await readFile(join(output,'bundle.json'))).length,reusedHarnessBytes:files.filter(f=>REUSED_HARNESS_FILES.has(f.path)).reduce((n,f)=>n+f.bytes,0),parentOperatorBytes,plannedUploadBytes:files.filter(f=>!REUSED_HARNESS_FILES.has(f.path)).reduce((n,f)=>n+f.bytes,0)+Buffer.byteLength(text)+(await readFile(join(output,'bundle.json'))).length+parentOperatorBytes,maxActiveMachines:1,builderTtlSeconds:1800,restoreTtlSeconds:1200,providerAllocationAuthorized:false,agentAcceptanceVerified:false};
 await writeFile(join(output,'derivation-plan.json'),JSON.stringify(plan,null,2)+'\n',{mode:0o600,flag:'wx'});return plan;
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){
 const a=process.argv.slice(2),get=k=>a[a.indexOf(k)+1];
 const names=['source','output','proof','builder','parent-bundle','cleanup'];
 if(!names.every(k=>a.includes('--'+k)))throw new Error('ExactParentDocumentsRequired');
 prepareCleanDerivation(Object.fromEntries(names.map(k=>[k==='parent-bundle'?'parentBundle':k,resolve(get('--'+k))]))).then(v=>console.log(JSON.stringify(v))).catch(()=>{console.error('Clean derivation preparation failed; no provider allocation occurred.');process.exitCode=1;});
}
