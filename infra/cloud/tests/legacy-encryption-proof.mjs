import test from 'node:test';
import assert from 'node:assert/strict';
import {createCipheriv,randomBytes} from 'node:crypto';
import {spawnSync} from 'node:child_process';
const script=new URL('../legacy-encryption-proof.mjs',import.meta.url);
test('private compatibility probe uses exact legacy AAD and outputs counts only',()=>{
 const key=randomBytes(32),iv=randomBytes(12),secret='fictional never-print-token';
 const cipher=createCipheriv('aes-256-gcm',key,iv);cipher.setAAD(Buffer.from('zuse-cloud-api-v1\ngithub-user\nfixture-owner'));
 const ciphertext=Buffer.concat([cipher.update(secret),cipher.final(),cipher.getAuthTag()]);
 const sealed=JSON.stringify({version:1,iv:iv.toString('base64url'),ciphertext:ciphertext.toString('base64url')});
 const r=spawnSync(process.execPath,[script.pathname],{input:JSON.stringify({key:key.toString('base64url'),rows:[{accountId:'fixture-owner',sealed},{accountId:'foreign-owner',sealed}]}),encoding:'utf8'});
 assert.equal(r.status,0);assert.deepEqual(JSON.parse(r.stdout),{metadataOnly:true,checked:2,opened:1,failed:1,keyMatchesAllSamples:false});
 for(const text of [secret,key.toString('base64url'),sealed,'fixture-owner'])assert.ok(!r.stdout.includes(text));
});
