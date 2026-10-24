// Engine owns workspace state; this proxy supplies isolated recorded-history/provider DTOs.
import fs from 'node:fs';import path from 'node:path';
import {overview,page,accounts} from './usage-dashboard-data.mjs';
const [ipc,ready,assets]=process.argv.slice(2),events=[],delayed=[];
let hold=false,fail=false,unpriced=false;
const emit=(c,f)=>{try{c.send(JSON.stringify(f));}catch{}};
const headers={'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'};
const server=Bun.serve({hostname:'127.0.0.1',port:0,async fetch(request,server){
 const url=new URL(request.url);
 if(url.pathname==='/__fixture'){
  if(request.method==='POST'){
   const p=await request.json();if(Object.hasOwn(p,'hold'))hold=p.hold;if(Object.hasOwn(p,'fail'))fail=p.fail;if(Object.hasOwn(p,'unpriced'))unpriced=p.unpriced;
   if(p.release)for(const[c,id,value]of delayed.splice(0)){emit(c,{id,ok:value});events.push({lateReply:id});}
  }
  return Response.json({events,hold,fail,unpriced},{headers});
 }
 if(url.pathname==='/api/session/check'||url.pathname==='/api/session')return Response.json({ok:true},{headers});
 if(url.pathname==='/api/rpc'||(url.pathname==='/'&&request.headers.get('upgrade')?.toLowerCase()==='websocket')){
  if(server.upgrade(request,{data:{handled:new Set(),waiting:[],ready:false,upstream:null}}))return;
  return new Response('WebSocket required',{status:400});
 }
 const target=path.resolve(assets,'.'+(url.pathname==='/'?'/index.html':url.pathname));
 if(!target.startsWith(path.resolve(assets)+path.sep))return new Response('Not found',{status:404});
 const file=Bun.file(target);return await file.exists()?new Response(file,{headers}):new Response('Not found',{status:404});
},websocket:{
 open(c){const up=new WebSocket('ws://127.0.0.1:'+ipc);c.data.upstream=up;up.onopen=()=>{c.data.ready=true;for(const f of c.data.waiting)up.send(f);c.data.waiting=[];};up.onmessage=e=>{try{emit(c,JSON.parse(e.data));}catch{}};up.onclose=()=>c.close();},
 message(c,raw){const text=raw.toString(),f=JSON.parse(text),p=f.params??{};
  if(f.cancel){events.push({cancel:f.id});if(c.data.handled.has(f.id))return;}
  if(f.method)events.push({id:f.id,method:f.method,params:p});
  if(['QueueMessage','QueueCommand','CloudCreate','WriteWorkspaceFile','RunProjectAction'].includes(f.method)){events.push({blocked:f.method});emit(c,{id:f.id,err:'Execution disabled in this fixture',done:true});return;}
  if(['UsageOverview','UsageSessions','ListAgentAccounts'].includes(f.method)){
   c.data.handled.add(f.id);
   if(fail&&f.method.startsWith('Usage')){emit(c,{id:f.id,err:'Recorded history is temporarily unavailable. Retry.',done:true});return;}
   let value=f.method==='UsageOverview'?overview(p):f.method==='UsageSessions'?page(p):accounts;
   if(unpriced&&f.method.startsWith('Usage')){
    value=structuredClone(value);
    const clear=v=>{if(!v||typeof v!=='object')return;if(Object.hasOwn(v,'costStatus')){v.costStatus='unknown';v.costUsd=null;}for(const child of Object.values(v))clear(child);};clear(value);
   }
   if(hold&&f.method.startsWith('Usage'))delayed.push([c,f.id,value]);else emit(c,{id:f.id,ok:value});return;
  }
  if(c.data.ready)c.data.upstream.send(text);else c.data.waiting.push(text);
 },close(c){c.data.upstream?.close();}
}});fs.writeFileSync(ready,'http://127.0.0.1:'+server.port);console.log('Usage dashboard fixture ready.');
