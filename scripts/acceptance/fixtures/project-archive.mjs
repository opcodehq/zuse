// Real archive metadata and restoration; isolated saved-transcript and failure responses.
import fs from 'node:fs';import path from 'node:path';
import {archiveDates,archiveMessages,archiveMetadata} from './project-archive-data.mjs';
const [ipc,ready,assets]=process.argv.slice(2),clients=new Set(),events=[];
let restoreMode='ok',releaseRestore,failDoc=null,holdDocs=new Set();const delayed=[];
let fileStatuses={},fileStatusMode='ok',holdStatuses=new Set();const delayedStatuses=[];
const emit=(c,f)=>{try{c.send(JSON.stringify(f));}catch{}};
const saved=(c,id,chat)=>emit(c,{id,item:{reset:archiveMessages(chat)}});
const headers={'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'};
const server=Bun.serve({hostname:'127.0.0.1',port:0,async fetch(request,server){
 const url=new URL(request.url);
 if(url.pathname==='/__fixture'){
  if(request.method==='POST'){
   const p=await request.json();
   if(Object.hasOwn(p,'restoreMode')){restoreMode=p.restoreMode;if(restoreMode!=='hold'){releaseRestore?.();releaseRestore=null;}}
   if(Object.hasOwn(p,'failDoc'))failDoc=p.failDoc;
   if(Object.hasOwn(p,'holdDocs'))holdDocs=new Set(p.holdDocs);
   if(p.fileStatuses)fileStatuses={...fileStatuses,...p.fileStatuses};
   if(Object.hasOwn(p,'fileStatusMode'))fileStatusMode=p.fileStatusMode;
   if(p.holdStatuses)holdStatuses=new Set(p.holdStatuses);
   if(p.releaseStatuses)for(const [c,id,status]of delayedStatuses.splice(0)){emit(c,{id,ok:status});events.push({lateStatus:status.chatId});}
   if(p.releaseDocs)for(const [c,id,chat]of delayed.splice(0)){saved(c,id,chat);events.push({lateReply:chat});}
  }
  return Response.json({events,restoreMode,failDoc,holdDocs:[...holdDocs],fileStatuses,fileStatusMode},{headers});
 }
 if(url.pathname==='/api/session/check'||url.pathname==='/api/session')return Response.json({ok:true},{headers});
 if(url.pathname==='/api/rpc'||(url.pathname==='/'&&request.headers.get('upgrade')?.toLowerCase()==='websocket')){
  if(server.upgrade(request,{data:{handled:new Map(),forwarded:new Map(),waiting:[],ready:false,upstream:null}}))return;
  return new Response('WebSocket required',{status:400});
 }
 const target=path.resolve(assets,'.'+(url.pathname==='/'?'/index.html':url.pathname));
 if(!target.startsWith(path.resolve(assets)+path.sep))return new Response('Not found',{status:404});
 const file=Bun.file(target);return await file.exists()?new Response(file,{headers}):new Response('Not found',{status:404});
},websocket:{
 open(c){clients.add(c);const up=new WebSocket('ws://127.0.0.1:'+ipc);c.data.upstream=up;
  up.onopen=()=>{c.data.ready=true;for(const f of c.data.waiting)up.send(f);c.data.waiting=[];};
  up.onmessage=event=>{try{const f=JSON.parse(event.data);if(c.data.forwarded.get(f.id)==='WatchChats'){
   const key=Object.hasOwn(f,'item')?'item':'ok';if(Array.isArray(f[key]))f[key]=f[key].map(chat=>archiveDates[chat.id]?{...chat,...archiveMetadata[chat.id],lastMessageAt:archiveDates[chat.id]}:chat);
  }emit(c,f);}catch{}};up.onclose=()=>c.close();
 },
 async message(c,raw){const text=raw.toString(),f=JSON.parse(text),p=f.params??{};
  if(f.cancel){events.push({cancel:f.id});if(c.data.handled.delete(f.id))return;}
  if(f.method)events.push({method:f.method,chatId:p.chatId,targetDeviceId:p.targetDeviceId,op:p.op,archived:p.archived});
  if(['QueueMessage','QueueCommand','CloudCreate','WriteWorkspaceFile','RunProjectAction'].includes(f.method)){events.push({blocked:f.method});emit(c,{id:f.id,err:'Execution is disabled in this fixture',done:true});return;}
  if(f.method==='GetChatArchiveStatus'&&(fileStatuses[p.chatId]||fileStatusMode==='fail')){
   c.data.handled.set(f.id,f.method);
   if(fileStatusMode==='fail'){emit(c,{id:f.id,err:'File status is temporarily unavailable. Retry.',done:true});return;}
   const status=structuredClone(fileStatuses[p.chatId]);
   if(holdStatuses.has(p.chatId)){delayedStatuses.push([c,f.id,status]);return;}
   emit(c,{id:f.id,ok:status});return;
  }
  if(f.method==='WatchDocMessages'){
   c.data.handled.set(f.id,f.method);
   if(failDoc===p.chatId){emit(c,{id:f.id,err:'Saved transcript is unavailable. Retry to load it again.',done:true});return;}
   if(holdDocs.has(p.chatId)){delayed.push([c,f.id,p.chatId]);return;}saved(c,f.id,p.chatId);return;
  }
  if(f.method==='Mutate'&&p.op==='setChatArchived'&&p.archived===false){
   if(restoreMode==='hold')await new Promise(resolve=>{releaseRestore=resolve;});
   if(restoreMode==='fail'){emit(c,{id:f.id,err:'The owning computer could not restore this chat. Retry.',done:true});return;}
  }
  if(f.method)c.data.forwarded.set(f.id,f.method);
  if(c.data.ready)c.data.upstream.send(text);else c.data.waiting.push(text);
 },close(c){clients.delete(c);c.data.upstream?.close();}
}});fs.writeFileSync(ready,'http://127.0.0.1:'+server.port);console.log('Project archive fixture ready.');
