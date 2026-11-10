// Integrity-checked local context; the outer operator owns runtime mounts.
import{lstat,readdir,open,readFile,writeFile,unlink}from'node:fs/promises';
import{join}from'node:path';import{constants}from'node:fs';import{createHash}from'node:crypto';
const hash=b=>createHash('sha256').update(b).digest('hex');
export async function verifyFiles(dir,files){
 const directory=await lstat(dir);if(!directory.isDirectory()||directory.isSymbolicLink())throw Error('DaytonaContextDirectory');
 const names=await readdir(dir);if(names.length!==files.length||new Set(names).size!==files.length||names.some(n=>!files.some(f=>f.path===n)))throw Error('DaytonaContextUnexpectedFile');
 for(const f of files){if(!/^[A-Za-z0-9._-]+$/.test(f.path))throw Error('DaytonaContextPath');const p=join(dir,f.path),s=await lstat(p);if(!s.isFile()||s.isSymbolicLink()||s.size!==f.bytes)throw Error('DaytonaContextFileMetadata');const fd=await open(p,constants.O_RDONLY|constants.O_NOFOLLOW);try{const opened=await fd.stat();if(opened.dev!==s.dev||opened.ino!==s.ino||opened.size!==f.bytes)throw Error('DaytonaContextFileChanged');const digest=createHash('sha256');for await(const chunk of fd.createReadStream())digest.update(chunk);if(digest.digest('hex')!==f.sha256)throw Error('DaytonaContextIntegrity');}finally{await fd.close();}}
}
export async function prepareDirectContext(bundle,plan,helperPath,dockerfile){
 const manifest=await readFile(join(bundle,'bundle.json'));if(hash(manifest)!==plan.bundleSha256)throw Error('DaytonaContextBundleMismatch');
 const base=[...plan.files,{path:'bundle.json',bytes:manifest.length,sha256:plan.bundleSha256}];await verifyFiles(bundle,base);
 const helperMetadata=await lstat(helperPath);if(!helperMetadata.isFile()||helperMetadata.isSymbolicLink())throw Error('DaytonaHelperChanged');
 const helper=await readFile(helperPath);if(hash(helper)!==plan.helperSha256)throw Error('DaytonaHelperChanged');if(hash(Buffer.from(dockerfile))!==plan.dockerfileSha256)throw Error('DaytonaDockerfileChanged');
 const generated=[{path:'managed-base-preparation.py',bytes:helper.length,sha256:plan.helperSha256},{path:'Dockerfile',bytes:Buffer.byteLength(dockerfile),sha256:plan.dockerfileSha256}];const owned=[];
 try{for(const [name,body]of[['managed-base-preparation.py',helper],['Dockerfile',dockerfile]]){await writeFile(join(bundle,name),body,{flag:'wx',mode:0o444});owned.push(name);}await verifyFiles(bundle,[...base,...generated]);return{directory:bundle,generated,cleanup:async()=>{await verifyFiles(bundle,[...base,...generated]);for(const name of owned)await unlink(join(bundle,name));}};}catch(e){for(const name of owned)await unlink(join(bundle,name));throw e;}
}
