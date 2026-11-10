import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,writeFile,chmod,rm,symlink} from 'node:fs/promises';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';
import {bindStreamingHashes} from '../e2b-streaming-hash.mjs';
const sha=value=>createHash('sha256').update(value).digest('hex');
const modulePath=process.env.ZUSE_E2B_SDK_TEST_MODULE;
for(const mode of [0o444,0o555,0o600])test('streaming COPY digest matches exact official SDK including metadata '+mode.toString(8),{skip:!modulePath},async()=>{
 const sdk=await import(pathToFileURL(modulePath));const root=await mkdtemp(join(tmpdir(),'e2b-hash-parity-'));
 try{
  const payload=Buffer.concat([Buffer.from('UTF8: λ and zero\0'),Buffer.alloc(131077,0xa5)]);await writeFile(join(root,'payload.bin'),payload);await chmod(join(root,'payload.bin'),mode);await writeFile(join(root,'config.json'),'{}\n');await chmod(join(root,'config.json'),0o444);
  const build=()=>sdk.Template({fileContextPath:root}).fromImage('ubuntu@sha256:'+'a'.repeat(64)).setUser('root').copy('payload.bin','/root/payload',{user:'root',mode:0o444,resolveSymlinks:false,gzip:true}).copy('payload.bin','/other/payload',{mode:0o555}).copy('config.json','/root/config').runCmd('true').setStartCmd('exec foreground','read-only-ready');
  const original=globalThis.fetch;globalThis.fetch=()=>{throw Error('NetworkForbidden')};
  try{
   const official=await build().toJSON(true);const streamed=await bindStreamingHashes(build(),root,new Map([['payload.bin',{sha256:sha(payload),bytes:payload.length}],['config.json',{sha256:sha('{}\n'),bytes:3}]]));
   assert.equal(await streamed.toJSON(true),official,'identical serialized request, not merely equivalent digest');
   await writeFile(join(root,'payload.bin'),'changed');await assert.rejects(streamed.toJSON(true),/InputChanged/);
  }finally{globalThis.fetch=original;}
 }finally{await rm(root,{recursive:true});}
});
test('pinned seam, payload SHA, literal paths and instruction drift fail closed',{skip:!modulePath},async()=>{
 const sdk=await import(pathToFileURL(modulePath));const root=await mkdtemp(join(tmpdir(),'e2b-hash-fence-'));
 try{await writeFile(join(root,'payload'),'fixture');await chmod(join(root,'payload'),0o444);const files=new Map([['payload',{sha256:sha('fixture'),bytes:7}]]);
 const build=src=>sdk.Template({fileContextPath:root}).copy(src,'/payload');
 const changed=build('payload');changed.instructionsWithHashes=async()=>[];await assert.rejects(bindStreamingHashes(changed,root,files),/SeamChanged/);
 await assert.rejects(bindStreamingHashes(build('*'),root,files),/Unsupported/);
 await assert.rejects(bindStreamingHashes(build('payload'),root,new Map([['payload',{sha256:'a'.repeat(64),bytes:7}]])),/InputChanged/);
 const t=await bindStreamingHashes(build('payload'),root,files);t.runCmd('unexpected');await assert.rejects(t.toJSON(true),/InstructionsChanged/);
 await symlink('payload',join(root,'link'));await assert.rejects(bindStreamingHashes(build('link'),root,new Map([['link',{sha256:sha('fixture'),bytes:7}]])),/InputUnsafe/);
 }finally{await rm(root,{recursive:true});}
});
