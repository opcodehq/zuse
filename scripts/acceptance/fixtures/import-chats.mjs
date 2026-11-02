// Only faults transport responses; successful discovery/imports and turns use the real host.
import fs from 'node:fs';
import path from 'node:path';
const [ipc,ready,assets]=process.argv.slice(2),events=[];
let failList=true,loseImport=true,holdImport=false,held=[];
const headers={'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'};
const emit=(client,frame)=>{try{client.send(JSON.stringify(frame));}catch{}};
const server=Bun.serve({hostname:'127.0.0.1',port:0,async fetch(request,server){
 const url=new URL(request.url);
 if(url.pathname==='/__fixture'){
  if(request.method==='POST'){
   const settings=await request.json();if('holdImport' in settings)holdImport=settings.holdImport;
   if(settings.release){for(const [client,frame] of held)emit(client,frame);held=[];}
  }
  return Response.json({events,held:held.length},{headers});
 }
 if(url.pathname==='/api/session/check'||url.pathname==='/api/session')return Response.json({ok:true},{headers});
 if(url.pathname==='/api/rpc'||(url.pathname==='/'&&request.headers.get('upgrade')?.toLowerCase()==='websocket')){
  if(server.upgrade(request,{data:{methods:new Map(),waiting:[],ready:false,upstream:null}}))return;
  return new Response('WebSocket required',{status:400});
 }
 const target=path.resolve(assets,'.'+(url.pathname==='/'?'/index.html':url.pathname));
 if(!target.startsWith(path.resolve(assets)+path.sep))return new Response('Not found',{status:404});
 const file=Bun.file(target);return await file.exists()?new Response(file,{headers}):new Response('Not found',{status:404});
},websocket:{
 open(client){const upstream=new WebSocket('ws://127.0.0.1:'+ipc);client.data.upstream=upstream;
  upstream.onopen=()=>{client.data.ready=true;for(const raw of client.data.waiting)upstream.send(raw);client.data.waiting=[];};
  upstream.onmessage=event=>{
   const frame=JSON.parse(event.data),method=client.data.methods.get(frame.id);
   if(method==='ImportExternalChat'&&frame.ok){
    if(loseImport){loseImport=false;events.push({lostAcceptedImport:frame.ok.chat.id});emit(client,{id:frame.id,err:'Import response was interrupted. Retry to recover it.',done:true});return;}
    if(holdImport){held.push([client,frame]);return;}
   }
   emit(client,frame);
  };upstream.onclose=()=>client.close();
 },
 message(client,raw){const frame=JSON.parse(raw.toString());
  if(frame.method){events.push({method:frame.method,params:frame.params??{}});client.data.methods.set(frame.id,frame.method);}
  if(frame.method==='ListExternalChats'&&failList){failList=false;emit(client,{id:frame.id,err:'Provider history was temporarily unavailable.',done:true});return;}
  if(frame.method?.startsWith('Cloud')&&frame.method!=='CloudAccountStatus'){emit(client,{id:frame.id,err:'Sign in to load cloud projects.',done:true});return;}
  if(frame.method==='QueueCommand'&&frame.params?.command?.request?.harness!=='claude-code'){emit(client,{id:frame.id,err:'Only the isolated import provider can execute.',done:true});return;}
  if(client.data.ready)client.data.upstream.send(raw.toString());else client.data.waiting.push(raw.toString());
 },close(client){client.data.upstream?.close();}
}});
fs.writeFileSync(ready,'http://127.0.0.1:'+server.port);
