// Instance-only compatibility seam for the exact pinned E2B 2.62.0 builder.
// Upstream package bytes and its streaming tar/PUT transport remain unchanged.
import {createHash} from 'node:crypto';
import {constants} from 'node:fs';
import {lstat,open} from 'node:fs/promises';
import {join} from 'node:path';
const sha=value=>createHash('sha256').update(value).digest('hex');
export const HASH_METHOD_SHA256='0f1ec5bfd5d07cc6ec62db9afcc154289bc104900fc79aa0747af549a82294e9';
export const SERIALIZE_METHOD_SHA256='765f2f217a1c003b54cd01287226fd0c17a0477fc5113a065505e902820119ca';
const identity=value=>['dev','ino','mode','size','mtimeNs','ctimeNs'].map(k=>String(value[k])).join(':');
export async function bindStreamingHashes(template,context,files){
 if(typeof template.instructionsWithHashes!=='function'||typeof template.serialize!=='function'||sha(template.instructionsWithHashes.toString())!==HASH_METHOD_SHA256||sha(template.serialize.toString())!==SERIALIZE_METHOD_SHA256)throw new Error('E2bSdkHashSeamChanged');
 const root=await lstat(context);if(root.isSymbolicLink()||!root.isDirectory()||root.uid!==process.getuid()||(root.mode&0o022))throw new Error('E2bHashContextUnsafe');
 const signature=JSON.stringify(template.instructions),snapshots=[],hashed=[];
 for(const instruction of template.instructions){
  if(instruction.type!=='COPY'){hashed.push({...instruction});continue;}
  const [src,dest]=instruction.args;
  if(!/^[a-zA-Z0-9_.-]+$/.test(src??'')||!files.has(src)||typeof dest!=='string'||instruction.resolveSymlinks===true)throw new Error('E2bHashInputUnsupported');
  const path=join(context,src),expected=files.get(src),named=await lstat(path,{bigint:true});
  if(!named.isFile()||named.isSymbolicLink()||named.uid!==BigInt(process.getuid())||(named.mode&0o022n)||named.size<1n||named.size>512n*1024n*1024n||(expected.bytes!==undefined&&named.size!==BigInt(expected.bytes)))throw new Error('E2bHashInputUnsafe');
  const handle=await open(path,constants.O_RDONLY|constants.O_NOFOLLOW);
  const hash=createHash('sha256'),payload=createHash('sha256');
  try{
   const stat=await handle.stat({bigint:true});if(identity(stat)!==identity(named))throw new Error('E2bHashInputChanged');
   // Exact upstream order for our literal regular-file COPY subset: statement,
   // relative name, full stat.mode decimal, byte count decimal, then file bytes.
   hash.update(`COPY ${src} ${dest}`);hash.update(src);hash.update(stat.mode.toString());hash.update(stat.size.toString());
   for await(const chunk of handle.createReadStream({autoClose:false,highWaterMark:1024*1024})){hash.update(chunk);payload.update(chunk);}
   if(payload.digest('hex')!==expected.sha256||identity(await handle.stat({bigint:true}))!==identity(stat)||identity(await lstat(path,{bigint:true}))!==identity(stat))throw new Error('E2bHashInputChanged');
   snapshots.push({path,identity:identity(stat)});hashed.push({...instruction,filesHash:hash.digest('hex')});
  }finally{await handle.close();}
 }
 // Hash all payloads before calling SDK buildInBackground (which first creates
 // a provider-side template). At the SDK seam, reject any instruction/file drift.
 template.instructionsWithHashes=async()=>{
  if(JSON.stringify(template.instructions)!==signature)throw new Error('E2bHashInstructionsChanged');
  for(const snapshot of snapshots)if(identity(await lstat(snapshot.path,{bigint:true}))!==snapshot.identity)throw new Error('E2bHashInputChanged');
  return hashed.map(instruction=>({...instruction}));
 };
 return template;
}
