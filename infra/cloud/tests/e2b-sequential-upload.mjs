import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
const upstream=process.env.ZUSE_E2B_SDK_UPSTREAM_MODULE,derived=process.env.ZUSE_E2B_SDK_TEST_MODULE;
function actualBuild(source){
 const start=source.indexOf('\tasync build(client, config, name, options) {'),end=source.indexOf('\n\t/**\n\t* Add file hashes',start);assert.ok(start>0&&end>start);const body=source.slice(start,end);
 return dependencies=>Function('requestBuild','getFileUploadLink','uploadFile','triggerBuild','LogEntry','readDockerignore',`return ({${body}}).build;`)(...dependencies,()=>[]);
}
async function exercise(source,{failUpload=false,failTrigger=false,cached=false}={}){
 let active=0,maxActive=0,trigger=0,uploads=0,requests=0;const stages=[];
 const deps=[async()=>{requests++;return{templateID:'synthetic-template',buildID:'synthetic-build',tags:[]};},async()=>({present:cached,url:'https://synthetic.invalid/upload'}),async()=>{uploads++;active++;maxActive=Math.max(maxActive,active);stages.push('spool');try{await new Promise(r=>setTimeout(r,5));stages.push('put');if(failUpload)throw Error('SyntheticUploadFailure');}finally{stages.push('cleanup');active--; }},async()=>{trigger++;if(failTrigger)throw Error('SyntheticTriggerReplyLost');},class{constructor(date,level,message){this.message=message;}}];
 const build=actualBuild(source)(deps);let error;try{await build.call({instructionsWithHashes:async()=>[{type:'RUN',args:['true']},{type:'COPY',args:['one','/one'],filesHash:'one'},{type:'COPY',args:['two','/two'],filesHash:'two'}],stackTraces:[],fileContextPath:'/synthetic',fileIgnorePatterns:[],serialize:x=>({steps:x})},{},{getSignal:()=>undefined},'synthetic',{onBuildLogs:()=>{}});}catch(e){error=e.message;}return{active,maxActive,trigger,uploads,requests,stages,error};
}
// readDockerignore is an outer lexical dependency; bind it explicitly without
// changing the SDK method under test.
for(const mode of ['bounded','upload-failure','trigger-lost','cached'])test('derived actual SDK upload loop '+mode,{skip:!upstream||!derived},async()=>{
 const source=await readFile(derived,'utf8');const result=await exercise(source,{failUpload:mode==='upload-failure',failTrigger:mode==='trigger-lost',cached:mode==='cached'});assert.equal(result.requests,1);assert.equal(result.active,0);
 if(mode==='bounded'){assert.equal(result.maxActive,1);assert.deepEqual(result.stages,['spool','put','cleanup','spool','put','cleanup']);assert.equal(result.trigger,1);}
 if(mode==='upload-failure'){assert.equal(result.uploads,1);assert.equal(result.trigger,0);assert.equal(result.error,'SyntheticUploadFailure');}
 if(mode==='trigger-lost'){assert.equal(result.trigger,1);assert.equal(result.uploads,2);assert.equal(result.error,'SyntheticTriggerReplyLost');}
 if(mode==='cached'){assert.equal(result.uploads,0);assert.equal(result.trigger,1);}
});
test('stock actual SDK demonstrates concurrent spool risk; derivation changes only pinned upload scheduling',{skip:!upstream||!derived},async()=>{
 const raw=await readFile(upstream),changed=await readFile(derived);assert.equal(createHash('sha256').update(raw).digest('hex'),'1c96b58fb06dd23f45d869a20800356b8712e33ff6c7b3bd04034e2fac112eaa');const old=await exercise(raw.toString());assert.equal(old.maxActive,2,'actual upstream map/Promise.all overlaps spool lifetimes');
 const output=execFileSync('python3',['-c',`import importlib.util,sys\nsys.dont_write_bytecode=True\ns=importlib.util.spec_from_file_location('d',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);sys.stdout.buffer.write(m.derive(open(sys.argv[2],'rb').read()))`,fileURLToPath(new URL('../derive-e2b-sequential-sdk.py',import.meta.url)),upstream]);assert.deepEqual(output,changed);
});
