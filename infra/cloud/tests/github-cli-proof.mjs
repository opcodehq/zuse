import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {githubCliProof} from '../github-cli-proof.mjs';
const wrapper=createHash('sha256').update('#!/bin/sh\nexec /usr/local/bin/zuse-cloud-runtime github-from-profile "$@"\n').digest('hex');
const legacy={version:'gh version 2.45.0 (2025-01-01)',sha256:'a'.repeat(64)};
const attested={...legacy,wrapperSha256:wrapper,loginShellResolvedManaged:true};
const proof=r=>({version:1,required:true,status:'passed',accountRequired:false,modelCalled:false,canonicalPath:'/opt/zuse/bin/gh',ancestorsControlled:true,profileMatches:true,
 systemReceipt:r,globalShim:{contentMatches:true,sha256:wrapper},loginShellMatchesGlobal:true,globalVersionMatchesReceipt:true});
test('publisher forwards complete legacy and attested receipt to actual Python proof command',async()=>{
 for(const receipt of [legacy,attested]){
  let calls=0;
  const result=await githubCliProof({exec:async(id,cmd)=>{calls++;assert.equal(id,'exact-builder');assert.ok(cmd.includes(JSON.stringify(receipt)));assert.ok(cmd.includes('valid_receipt'));return JSON.stringify(proof(receipt));}},'exact-builder',{rustGithubCliVersion:1,githubCli:receipt});
  assert.equal(result.status,'passed');assert.equal(calls,1);
 }
});
test('unknown, mixed, nontrue attestation and invalid wrapper digest fail before any remote execution',async()=>{
 for(const receipt of [{...legacy,extra:1},{...legacy,wrapperSha256:wrapper},{...legacy,loginShellResolvedManaged:true},{...attested,loginShellResolvedManaged:false},{...attested,wrapperSha256:'bad'}]){
  let calls=0;await assert.rejects(githubCliProof({exec:async()=>{calls++;}},'original',{rustGithubCliVersion:1,githubCli:receipt}),/ManagedGithubCliReceiptInvalid/);assert.equal(calls,0);
 }
});
test('attested publisher rejects forged or downgraded wrapper proof and missing shim remains failed',async()=>{
 for(const mutate of [p=>delete p.systemReceipt.wrapperSha256,p=>p.systemReceipt.loginShellResolvedManaged=false,p=>p.globalShim.sha256='0'.repeat(64),p=>p.canonicalPath='/usr/local/bin/gh',p=>p.ancestorsControlled=false,p=>p.profileMatches=false]){
  const p=proof({...attested});mutate(p);
  await assert.rejects(githubCliProof({exec:async()=>JSON.stringify(p)},'original',{rustGithubCliVersion:1,githubCli:attested}),/ManagedGithubCliProofInvalid/);
 }
 const absent={version:1,required:true,status:'rejected',code:'managed_github_global_shim_invalid',globalShim:{lexists:false},accountRequired:false,modelCalled:false};
 assert.equal((await githubCliProof({exec:async()=>JSON.stringify(absent)},'original',{rustGithubCliVersion:1,githubCli:attested})).status,'rejected');
});
