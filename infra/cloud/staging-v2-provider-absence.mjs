// Independent provider inspection only. No execution or lifecycle mutations.
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createBoatBuilderAdapter,createBoxdBuilderAdapter} from './build-managed-base.mjs';

export async function inspectAbsence(input,{env=process.env,fetchFn=fetch,
 boxd=createBoxdBuilderAdapter,boat=createBoatBuilderAdapter}={}){
 const {provider,targets,sdk}=input;
 if(!['boxd','boat','e2b','daytona'].includes(provider)||!Array.isArray(targets)||targets.length<1||targets.length>4||new Set(targets).size!==targets.length||targets.some(id=>typeof id!=='string'||!/^[-a-zA-Z0-9_]{1,100}$/.test(id)))throw Error('invalid_exact_resource_inventory');
 let adapter;
 try{
  if(provider==='boxd')adapter=await boxd(sdk,undefined,{nativeDefault:true});
  else if(provider==='boat')adapter=await boat();
  else{
   const key=env[provider==='e2b'?'E2B_API_KEY':'DAYTONA_API_KEY'];
   if(typeof key!=='string'||!key||/[\r\n\0]/.test(key))throw Error('provider_key_required');
   const origin=provider==='e2b'?'https://api.e2b.app/sandboxes/':'https://app.daytona.io/api/sandbox/';
   const headers=provider==='e2b'?{'X-API-Key':key}:{Authorization:'Bearer '+key};
   adapter={absent:async id=>{
    const response=await fetchFn(origin+encodeURIComponent(id),{method:'GET',redirect:'error',headers,signal:AbortSignal.timeout(15000)});
    // Authentication, quota and transport failures never establish absence.
    if(response.status===404)return true;
    if(response.status===200)return false;
    throw Error('provider_inspection_unavailable');
   }};
  }
  let absent=0;
  for(const id of targets)if(await adapter.absent(id))absent++;
  return {metadataOnly:true,provider,checkedCount:targets.length,absentCount:absent,allAbsent:absent===targets.length};
 }finally{adapter?.close?.();}
}
async function main(){
 try{
  const report=await inspectAbsence(JSON.parse(readFileSync(0,'utf8')));
  console.log(JSON.stringify(report));if(!report.allAbsent)process.exitCode=1;
 }catch{console.log(JSON.stringify({metadataOnly:true,allAbsent:false,code:'provider_inspection_unavailable'}));process.exitCode=1;}
}
if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href)await main();
