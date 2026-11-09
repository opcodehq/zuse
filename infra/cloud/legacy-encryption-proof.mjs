// Private stdin only; never print key, account, envelope or decrypted material.
// Pinned TS api-sealing.ts uses AES256-GCM and this exact account-bound AAD.
import {createDecipheriv} from 'node:crypto';
import {readFileSync} from 'node:fs';
let key;
try{
 const input=JSON.parse(readFileSync(0,'utf8'));
 key=Buffer.from(input.key,'base64url');
 if(key.length!==32||!Array.isArray(input.rows)||input.rows.length>64)throw Error();
 let opened=0,failed=0;
 for(const row of input.rows){
  let plaintext;
  try{
   if(typeof row.accountId!=='string'||row.accountId.length>200||typeof row.sealed!=='string'||row.sealed.length>131072)throw Error();
   const envelope=JSON.parse(row.sealed);
   if(envelope.version!==1)throw Error();
   const bytes=Buffer.from(envelope.ciphertext,'base64url'),iv=Buffer.from(envelope.iv,'base64url');
   if(iv.length!==12||bytes.length<16)throw Error();
   const decipher=createDecipheriv('aes-256-gcm',key,iv);
   decipher.setAAD(Buffer.from('zuse-cloud-api-v1\ngithub-user\n'+row.accountId));
   decipher.setAuthTag(bytes.subarray(-16));
   plaintext=Buffer.concat([decipher.update(bytes.subarray(0,-16)),decipher.final()]);
   opened++;
  }catch{failed++;}
  finally{plaintext?.fill(0);}
 }
 console.log(JSON.stringify({metadataOnly:true,checked:input.rows.length,opened,failed,keyMatchesAllSamples:opened>0&&failed===0}));
}catch{console.log(JSON.stringify({metadataOnly:true,checked:0,opened:0,failed:0,keyMatchesAllSamples:false,code:'private_crypto_proof_unavailable'}));process.exitCode=1;}
finally{key?.fill(0);}
