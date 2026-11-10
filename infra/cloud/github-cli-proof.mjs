import{readFile}from'node:fs/promises';
const q=v=>`'${v.replaceAll("'","'\\''")}'`;
export async function githubCliProof(adapter,id,manifest){
 if(manifest?.rustGithubCliVersion!==1)return{version:1,required:false,status:'legacy-unattested'};
 const receipt=manifest.githubCli;
 const fields=receipt&&typeof receipt==='object'?Object.keys(receipt).sort().join(','):'';
 const legacy=fields==='sha256,version',attested=fields==='loginShellResolvedManaged,sha256,version,wrapperSha256';
 if(!legacy&&!attested)throw new Error('ManagedGithubCliReceiptInvalid');
 if(attested&&(!/^[a-f0-9]{64}$/.test(receipt.wrapperSha256??'')||receipt.loginShellResolvedManaged!==true))throw new Error('ManagedGithubCliReceiptInvalid');
 if(!receipt||typeof receipt.version!=='string'||!receipt.version.startsWith('gh version 2.')||receipt.version.length>160||!/^[\x20-\x7e]+$/.test(receipt.version)||!/^[a-f0-9]{64}$/.test(receipt.sha256??''))throw new Error('ManagedGithubCliReceiptInvalid');
 const source=await readFile(new URL('./github-cli-proof.py',import.meta.url));
 const command='sudo -n /usr/bin/python3 -I -B -c '+q(source.toString('utf8'))+' --receipt '+q(JSON.stringify(receipt));
 const proof=JSON.parse(await adapter.exec(id,command));
 if(proof.version!==1||proof.required!==true||!['passed','rejected'].includes(proof.status)||proof.accountRequired!==false||proof.modelCalled!==false)throw new Error('ManagedGithubCliProofInvalid');
 if(proof.status==='passed'&&(proof.systemReceipt?.version!==receipt.version||proof.systemReceipt?.sha256!==receipt.sha256||proof.globalShim?.contentMatches!==true||proof.loginShellMatchesGlobal!==true||proof.globalVersionMatchesReceipt!==true||proof.canonicalPath!=='/opt/zuse/bin/gh'||proof.ancestorsControlled!==true||proof.profileMatches!==true))throw new Error('ManagedGithubCliProofInvalid');
 if(proof.status==='passed'&&attested&&(proof.systemReceipt?.wrapperSha256!==receipt.wrapperSha256||proof.systemReceipt?.loginShellResolvedManaged!==true||proof.globalShim?.sha256!==receipt.wrapperSha256))throw new Error('ManagedGithubCliProofInvalid');
 return proof;
}
export function requireGithubCliProof(proof){if(proof.required&&proof.status!=='passed')throw new Error('ManagedGithubCliProofFailed');}
export async function githubCliReadiness(adapter,id,manifest,{missingTimeoutMs=30000,onObservation=async()=>{}}={}){
 const deadline=Date.now()+missingTimeoutMs;
 for(;;){const proof=await githubCliProof(adapter,id,manifest);await onObservation(proof);
  if(!proof.required||proof.status==='passed')return proof;
  // A restored filesystem can become available after command readiness.
  // Only an actually absent path is pending; unsafe contents/permissions/PATH
  // immediately reject. No command or account action is replayed here.
  if(proof.code!=='managed_github_global_shim_invalid'||proof.globalShim?.lexists!==false||Date.now()>=deadline)requireGithubCliProof(proof);
  await new Promise(r=>setTimeout(r,500));
 }
}
