// Isolated Cloud archive/account replies; ordinary chat metadata uses a real engine.
import fs from 'node:fs';
import path from 'node:path';
import {archiveMessages} from './project-archive-data.mjs';
const [ipc,ready,assets,dataFile]=process.argv.slice(2);
const data=JSON.parse(fs.readFileSync(dataFile));
const clients=new Set(),events=[];
let account='archive-owner',orgId=null,responseOwner=null;
let rows=data.rows.map(row=>structuredClone(row));
const modes={list:'ok',read:'ok',restore:'ok',delete:'ok',cleanup:'ok'},waiting={};
const emit=(client,value)=>{try{client.send(JSON.stringify(value));}catch{}};
const status=()=>({user:account?{id:account,name:'Avery',email:account+'@example.test'}:null,orgId,login:null,error:null});
const headers={'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'};
const server=Bun.serve({hostname:'127.0.0.1',port:0,async fetch(request,server){
 const url=new URL(request.url);
 if(url.pathname==='/__fixture'){
  if(request.method==='POST'){
   const p=await request.json();
   if(Object.hasOwn(p,'rows'))rows=structuredClone(p.rows);
   if(Object.hasOwn(p,'responseOwner'))responseOwner=p.responseOwner;
   if(Object.hasOwn(p,'orgId'))orgId=p.orgId;
   if(Object.hasOwn(p,'account'))account=p.account;
   if(Object.hasOwn(p,'account')||Object.hasOwn(p,'orgId'))for(const c of clients)for(const [id,method]of c.data.handled)if(method==='CloudAccountStatus')emit(c,{id,item:status()});
   for(const [kind,mode]of Object.entries(p.modes??{})){modes[kind]=mode;if(mode!=='hold')for(const resolve of waiting[kind]?.splice(0)??[])resolve();}
  }
  return Response.json({events,rows,account,orgId,modes},{headers});
 }
 if(['/api/session/check','/api/session'].includes(url.pathname))return Response.json({ok:true},{headers});
 if(url.pathname==='/api/rpc'||(url.pathname==='/'&&request.headers.get('upgrade')?.toLowerCase()==='websocket')){
  if(server.upgrade(request,{data:{handled:new Map(),upstream:null,waiting:[],ready:false}}))return;
  return new Response('WebSocket required',{status:400});
 }
 const target=path.resolve(assets,'.'+(url.pathname==='/'?'/index.html':url.pathname));
 if(!target.startsWith(path.resolve(assets)+path.sep))return new Response('Not found',{status:404});
 const file=Bun.file(target);return await file.exists()?new Response(file,{headers}):new Response('Not found',{status:404});
},websocket:{
 open(c){clients.add(c);const up=c.data.upstream=new WebSocket('ws://127.0.0.1:'+ipc);
  up.onopen=()=>{c.data.ready=true;for(const f of c.data.waiting)up.send(f);c.data.waiting=[];};
  up.onmessage=event=>{try{emit(c,JSON.parse(event.data));}catch{}};up.onclose=()=>c.close();
 },
 async message(c,raw){
  const text=raw.toString(),f=JSON.parse(text),p=f.params??{};
  if(f.cancel&&c.data.handled.delete(f.id))return;
  if(f.method==='CloudAccountStatus'){c.data.handled.set(f.id,f.method);emit(c,{id:f.id,item:status()});return;}
  if(['QueueMessage','QueueCommand','CloudCreate','CloudResume','CreateTerminal','OpenTerminal'].includes(f.method)){
   events.push({blocked:f.method});emit(c,{id:f.id,err:'Execution is disabled in this fixture',done:true});return;
  }
  const kind={CloudWorkspaces:'list',CloudHistory:'read',CloudUnarchive:'restore',CloudDelete:'delete',CloudCleanupMirror:'cleanup'}[f.method];
  if(kind){
   const scope=account,row=structuredClone(rows.find(row=>row.workspaceId===p.workspaceId)),list=structuredClone(rows);
   const event={method:f.method,workspaceId:p.workspaceId,sessionId:p.sessionId,expectedAccountId:p.expectedAccountId,expectedOrganizationId:p.expectedOrganizationId,commandId:p.commandId};events.push(event);
   c.data.handled.set(f.id,f.method);
   if(!scope||orgId!==null||p.expectedAccountId!==scope||p.expectedOrganizationId!==null){event.rejected=true;emit(c,{id:f.id,err:'The request must use its current personal account',done:true});return;}
   if(modes[kind]==='hold')await new Promise(resolve=>(waiting[kind]??=[]).push(resolve));
   if(modes[kind]==='fail'){event.failed=true;emit(c,{id:f.id,err:kind==='restore'?'The saved chat could not be restored. Retry to continue.':kind==='read'?'Saved Cloud history is temporarily unavailable. Try again.':'Cloud archives are temporarily unavailable. Refresh to try again.',done:true});return;}
   const owner=responseOwner??scope;let value;
   if(kind==='list')value={accountId:owner,workspaces:list};
   else if(kind==='read'){
    if(!row){emit(c,{id:f.id,err:'Archive was not found',done:true});return;}
    const session=p.sessionId??row.initialSessionId,chat=data.chats.find(chat=>chat.id===session)??data.chats[0];
    value={accountId:owner,workspace:row,head:{schemaVersion:1,payloadFormat:'zuse-rust-transcript-v1',workspaceId:row.workspaceId,sessionId:chat.id,runtimeGeneration:1,cursor:{epoch:'archive-fixture',version:1},chat,conversations:data.chats,transcript:{reset:archiveMessages(chat.id)},queue:[],queuePaused:true,olderMessageSequence:null}};
   }else{
    if(!row){emit(c,{id:f.id,err:'Archive was not found',done:true});return;}
    const next=kind==='restore'?{...row,state:'paused',desiredState:'paused',statusCode:'paused'}:kind==='delete'?{...row,state:'deleting',desiredState:'deleted',statusCode:'cloud_object_purge_failed'}:{...row,localMirrorCleanup:{state:'complete',removed:2,preserved:1}};
    if(owner===scope&&account===scope)rows=rows.map(item=>item.workspaceId===row.workspaceId?next:item);
    value={accountId:owner,workspace:next};
   }
   event.reply={accountId:owner,rows:value.workspaces?.map(w=>({workspaceId:w.workspaceId,state:w.state,archiveStatus:w.archiveStatus})),workspaceId:value.workspace?.workspaceId,state:value.workspace?.state,sessionId:value.head?.sessionId};
   emit(c,{id:f.id,ok:value});return;
  }
  if(c.data.ready)c.data.upstream.send(text);else c.data.waiting.push(text);
 },close(c){clients.delete(c);c.data.upstream?.close();}
}});fs.writeFileSync(ready,'http://127.0.0.1:'+server.port);console.log('Cloud archive settings fixture ready.');
