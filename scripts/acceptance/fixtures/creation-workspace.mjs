// Real workspace/queue/transcript RPCs; the visible test provider runs the built-in script.
import fs from 'node:fs';
import path from 'node:path';
const [ipc,ready,assets]=process.argv.slice(2),events=[];
const headers={'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'};
const emit=(client,frame)=>{try{client.send(JSON.stringify(frame));}catch{}};
const server=Bun.serve({hostname:'127.0.0.1',port:0,async fetch(request,server){
 const url=new URL(request.url);
 if(url.pathname==='/__fixture')return Response.json({events},{headers});
 if(url.pathname==='/api/session/check'||url.pathname==='/api/session')return Response.json({ok:true},{headers});
 if(url.pathname==='/api/rpc'||(url.pathname==='/'&&request.headers.get('upgrade')?.toLowerCase()==='websocket')){
  if(server.upgrade(request,{data:{handled:new Set(),waiting:[],ready:false,upstream:null}}))return;
  return new Response('WebSocket required',{status:400});
 }
 const target=path.resolve(assets,'.'+(url.pathname==='/'?'/index.html':url.pathname));
 if(!target.startsWith(path.resolve(assets)+path.sep))return new Response('Not found',{status:404});
 const file=Bun.file(target);return await file.exists()?new Response(file,{headers}):new Response('Not found',{status:404});
},websocket:{
 open(client){const upstream=new WebSocket('ws://127.0.0.1:'+ipc);client.data.upstream=upstream;
  upstream.onopen=()=>{client.data.ready=true;for(const raw of client.data.waiting)upstream.send(raw);client.data.waiting=[];};
  upstream.onmessage=event=>{try{emit(client,JSON.parse(event.data));}catch{}};upstream.onclose=()=>client.close();
 },
 message(client,raw){let text=raw.toString();const frame=JSON.parse(text),p=frame.params??{};
  if(frame.cancel&&client.data.handled.delete(frame.id))return;
  if(frame.method)events.push({method:frame.method,params:p});
  if(frame.method==='ListHarnesses'){emit(client,{id:frame.id,ok:[{id:'codex',name:'Codex',installed:true,enabled:true,supportsSteering:true,steeringMode:'step-boundary',reasoningLevels:['medium']}]});return;}
  if(frame.method==='CloudAccountStatus'){client.data.handled.add(frame.id);emit(client,{id:frame.id,item:{user:null,login:null,error:null}});return;}
  if(['CloudContext','CloudWorkspaces'].includes(frame.method)){emit(client,{id:frame.id,err:'Sign in to load cloud projects.',done:true});return;}
  if((frame.method==='QueueCommand'&&!['codex','mock'].includes(p.command?.request?.harness))||frame.method?.startsWith('Cloud')&&frame.method!=='CloudAccountStatus'){
   events.push({blocked:frame.method});emit(client,{id:frame.id,err:'Only isolated scripted local runs are allowed.',done:true});return;
  }
  // Browser production pickers hide the scripted agent. Map only its identity;
  // leave the actual checkout, queue, prompt and transcript contracts intact.
  if(frame.method==='ListModels'&&p.harness==='codex'){p.harness='mock';text=JSON.stringify(frame);}
  if(frame.method==='QueueCommand'){
   const forwarded=structuredClone(frame);forwarded.params.command.request.harness='mock';
   events.push({forwardedScriptedRun:forwarded.params});text=JSON.stringify(forwarded);
  }
  if(client.data.ready)client.data.upstream.send(text);else client.data.waiting.push(text);
 },close(client){client.data.upstream?.close();}
}});fs.writeFileSync(ready,'http://127.0.0.1:'+server.port);console.log('Workspace creation fixture ready.');
