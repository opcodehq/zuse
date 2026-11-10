// Exact local proof documents, verified before any provider mutation. No account data.
import {readFile,lstat} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {dirname,join} from 'node:path';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {fileURLToPath} from 'node:url';
export const REUSED_HARNESS_FILES=new Set(['claude','codex','grok','codex-assets.tar']);
const sha=value=>createHash('sha256').update(value).digest('hex');
export async function loadCleanLineage(path,expected,bundle) {
 if(!/^[a-f0-9]{64}$/.test(expected)|| (await lstat(path)).isSymbolicLink())throw new Error('InvalidCleanParentLineage');
 const bytes=await readFile(path);if(bytes.length>1024*1024||sha(bytes)!==expected)throw new Error('CleanParentLineageDigestMismatch');
 const lineage=JSON.parse(bytes);if(lineage.schema!==1||lineage.provider!=='boat')throw new Error('InvalidCleanParentLineage');
 const docs={};for(const key of ['proof','builder','parentBundle','cleanup']){
  const text=lineage.documents?.[key];if(typeof text!=='string'||Buffer.byteLength(text)>262144||sha(text)!==lineage.digests?.[key])throw new Error('ParentDocumentDigestMismatch');docs[key]=JSON.parse(text);
 }
 // Shared validation with the actual guest-side preflight, without root/file access.
 const helper=join(dirname(fileURLToPath(import.meta.url)),'managed-base-derive.py');
 await promisify(execFile)('python3',['-c',`import importlib.util,json,sys\ns=importlib.util.spec_from_file_location('derive',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);raw=open(sys.argv[2],'rb').read();assert m.hashlib.sha256(raw).hexdigest()==sys.argv[4];m.parent_binding(json.loads(raw),json.loads(sys.argv[3]))`,helper,path,JSON.stringify(bundle),expected],{env:{PATH:'/usr/bin:/bin'},timeout:10000,maxBuffer:4096});
 if(bundle.managedBaseDerivationVersion!==1||!bundle.files.some(f=>f.path==='managed-base-derive.py'))throw new Error('DerivationInstallerRequired');
 return {lineage,docs,sha256:expected,bytes,artifactId:docs.proof.artifactId,providerArtifactVersion:docs.builder.providerArtifactVersion??null,parentRuntimeSha256:docs.builder.runtimeSha256};
}
export async function makeCleanLineage(paths) {
 const documents={};const digests={};for(const key of ['proof','builder','parentBundle','cleanup']){
  if((await lstat(paths[key])).isSymbolicLink())throw new Error('ParentDocumentSymlink');
  const text=await readFile(paths[key],'utf8');if(Buffer.byteLength(text)>262144)throw new Error('ParentDocumentTooLarge');documents[key]=text;digests[key]=sha(text);
 }
 return {schema:1,provider:'boat',documents,digests,parentBundleFileSha256:digests.parentBundle};
}
