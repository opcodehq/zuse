#!/usr/bin/env node
// Private bounded child observer; the parent owns the durable original operation.
import{readFile,open}from'node:fs/promises';import{resolve,join,dirname}from'node:path';import{pathToFileURL}from'node:url';import{createHash}from'node:crypto';import{isDeepStrictEqual}from'node:util';
export const SNAPSHOT_RESOURCES=Object.freeze({cpu:1,memory:2,disk:8});
export async function submitSnapshot(spec,sdk){
 if(sdk.SandboxClass.CONTAINER!=='container')throw new Error('UnsupportedPersistentClass');
 if(spec.resources!==undefined&&!isDeepStrictEqual(spec.resources,SNAPSHOT_RESOURCES))throw new Error('SnapshotResourcesChanged');
 const image=sdk.Image.fromDockerfile(join(spec.context,'Dockerfile'),{strictContext:true});
 const client=new sdk.Daytona({apiKey:process.env.DAYTONA_API_KEY,apiUrl:'https://app.daytona.io/api',target:spec.region??undefined,requestTimeoutMs:30000,useDeprecatedPolling:true,otelEnabled:false});
 const result=await client.snapshot.create({name:spec.name,image,sandboxClass:sdk.SandboxClass.CONTAINER,resources:{...SNAPSHOT_RESOURCES},...(spec.region?{regionId:spec.region}:{})},{timeout:900});
 return{id:result.id,name:result.name,state:result.state,sandboxClass:result.sandboxClass,imageName:result.imageName};
}
async function main(){const spec=JSON.parse(await readFile(process.argv[2],'utf8'));const metadata=JSON.parse(await readFile(join(spec.sdk,'package.json'),'utf8'));if(metadata.name!=='@daytona/sdk'||metadata.version!=='0.227.0')throw new Error('UnpinnedSdk');const sdk=await import(pathToFileURL(join(resolve(spec.sdk),'esm/index.js')).href);const result=await submitSnapshot(spec,sdk);console.log(JSON.stringify({status:'snapshot-created',snapshot:result}));}
export async function preserveSubmissionFailure(specPath,error,secret){
 const spec=JSON.parse(await readFile(specPath,'utf8'));
 const redact=value=>secret?String(value).split(secret).join('[REDACTED]'):String(value);
 const value={version:1,name:spec.name,httpStatus:Number.isInteger(error.statusCode)?error.statusCode:null,errorName:/^[A-Za-z]{1,80}$/.test(error.name??'')?error.name:'ProviderError',message:redact(error.message??'').slice(0,4000)};
 const bytes=Buffer.from(JSON.stringify(value,null,2)+'\n');
 const fd=await open(specPath+'.failure.private.json','wx',0o600);
 try{await fd.writeFile(bytes);await fd.sync();}finally{await fd.close();}
 const directory=await open(dirname(specPath),'r');try{await directory.sync();}finally{await directory.close();}
 return{diagnosticSha256:createHash('sha256').update(bytes).digest('hex'),diagnosticBytes:bytes.length};
}
if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href)main().catch(async error=>{
 let diagnostic={diagnosticSaved:false};
 try{diagnostic={diagnosticSaved:true,...await preserveSubmissionFailure(process.argv[2],error,process.env.DAYTONA_API_KEY)};}catch{}
 console.error(JSON.stringify({status:'unconfirmed',code:'DaytonaSnapshotSubmissionUnconfirmed',...(Number.isInteger(error.statusCode)?{httpStatus:error.statusCode}:{}),...diagnostic}));process.exitCode=1;
});
