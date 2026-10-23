// Independent original AES-GCM/gzip envelope reader and synthetic fixture generator.
import { createCipheriv, createDecipheriv, createHash } from 'node:crypto';
import { gzipSync, gunzipSync } from 'node:zlib';
import { readFileSync } from 'node:fs';
const key=Buffer.alloc(32,29), master=Buffer.alloc(32,28);
const encrypt=(key,aad,bytes,nonce)=>{const iv=Buffer.alloc(12,nonce), cipher=createCipheriv('aes-256-gcm',key,iv);cipher.setAAD(Buffer.from(aad));return {version:1,iv:iv.toString('base64url'),ciphertext:Buffer.concat([cipher.update(bytes),cipher.final(),cipher.getAuthTag()]).toString('base64url')}};
const aad=(workspace,session,cursor,before)=>`zuse-cloud-transcript-v1\n${workspace}\n${session}\n${cursor.epoch}\n${cursor.version}\n1\n${before===undefined?'head':`page:${before}`}`;
if(process.argv[2]==='--decrypt'){
 const input=JSON.parse(readFileSync(0,'utf8')), access=input.access, metadata=access.metadata??access, env=JSON.parse(access.ciphertext), bytes=Buffer.from(env.ciphertext,'base64url');
 const cipher=createDecipheriv('aes-256-gcm',Buffer.from(access.transcriptKey,'base64url'),Buffer.from(env.iv,'base64url'));cipher.setAAD(Buffer.from(aad(input.workspaceId,input.sessionId,metadata.cursor,access.beforeSequence)));cipher.setAuthTag(bytes.subarray(-16));
 console.log(gunzipSync(Buffer.concat([cipher.update(bytes.subarray(0,-16)),cipher.final()])).toString());
}else{
 const message=(id,text)=>({id,sessionId:'session-bob',role:'user',content:{_tag:'user',text},createdAt:'2026-10-23T00:00:00.000Z'});
 const head=(version,text)=>({schemaVersion:1,workspaceId:'ws-bob',sessionId:'session-bob',cursor:{epoch:'original/epoch !é',version},projection:{messages:[message(`message-${version}`,text)],olderMessageSequence:9,status:'idle',activeTurn:null,turnCount:0,queue:{items:[],paused:true},permissionMode:'default',runtimeMode:'approval-required',interactions:[]}});
 const page={schemaVersion:1,workspaceId:'ws-bob',sessionId:'session-bob',cursor:{epoch:'original/epoch !é',version:2},beforeSequence:10,messages:[message('older-message','Older preserved user text')],olderMessageSequence:null};
 const upload=(payload,nonce)=>{const envelope=encrypt(key,aad('ws-bob','session-bob',payload.cursor,payload.beforeSequence),gzipSync(Buffer.from(JSON.stringify(payload))),nonce);const ciphertext=JSON.stringify({version:1,compression:'gzip',iv:envelope.iv,ciphertext:envelope.ciphertext});return {sessionId:'session-bob',cursor:payload.cursor,...(payload.beforeSequence?{beforeSequence:payload.beforeSequence}:{}),ciphertext,ciphertextSha256:createHash('sha256').update(ciphertext).digest('base64url')}};
 console.log(JSON.stringify({masterKey:master.toString('base64url'),transcriptKey:key.toString('base64url'),wrappedKey:JSON.stringify(encrypt(master,'zuse-cloud-transcript-wrapping-v1\nuser-bob\nws-bob',Buffer.from(key.toString('base64url')),17)),head1:upload(head(1,'Original preserved user text'),1),head2:upload(head(2,'Latest preserved user text'),2),page:upload(page,3)},null,2));
}
