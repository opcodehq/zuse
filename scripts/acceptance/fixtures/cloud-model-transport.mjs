// Transport faults surround real host/API writes; successful results are never fabricated.
import fs from 'node:fs';
import path from 'node:path';
const [ipc,ready,assets]=process.argv.slice(2);
const events=[],held=[];
let faults={failContext:false,holdCreate:false,loseCreateResponse:false};
const headers={'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'};
function emit(client,frame){try{client.send(JSON.stringify(frame));}catch{}}
function release(client,frame){
 if(faults.loseCreateResponse){faults.loseCreateResponse=false;emit(client,{id:frame.id,err:'Cloud response interrupted. Retry to check the accepted request.',done:true});}
 else emit(client,frame);
}
const server=Bun.serve({hostname:'127.0.0.1',port:0,
 async fetch(request,server){
  const url=new URL(request.url);
  if(url.pathname==='/__fixture'){
   if(request.method==='POST'){
    faults={...faults,...await request.json()};
    if(!faults.holdCreate)for(const [client,frame] of held.splice(0))release(client,frame);
   }
   return Response.json({faults,events,held:held.length},{headers});
  }
  if(url.pathname==='/api/session/check'||url.pathname==='/api/session')return Response.json({ok:true},{headers});
  if(url.pathname==='/api/rpc'||(url.pathname==='/'&&request.headers.get('upgrade')?.toLowerCase()==='websocket')){
   if(server.upgrade(request,{data:{pending:new Map(),waiting:[],upstream:null,ready:false}}))return;
   return new Response('WebSocket required',{status:400});
  }
  const target=path.resolve(assets,'.'+(url.pathname==='/'?'/index.html':url.pathname));
  if(!target.startsWith(path.resolve(assets)+path.sep))return new Response('Not found',{status:404});
  const file=Bun.file(target);return await file.exists()?new Response(file,{headers}):new Response('Not found',{status:404});
 },websocket:{
  open(client){
   const upstream=new WebSocket('ws://127.0.0.1:'+ipc);client.data.upstream=upstream;
   upstream.onopen=()=>{client.data.ready=true;for(const text of client.data.waiting)upstream.send(text);client.data.waiting=[];};
   upstream.onmessage=event=>{
    let frame;try{frame=JSON.parse(event.data);}catch{return;}
    const method=client.data.pending.get(frame.id);
    if(method==='CloudCreate'&&frame.ok){
     events.push({accepted:frame.ok});
     if(faults.holdCreate)held.push([client,frame]);else release(client,frame);
    }else emit(client,frame);
   };
   upstream.onclose=()=>client.close();
  },
  message(client,message){
   const text=message.toString(),frame=JSON.parse(text);
   if(frame.method){
    client.data.pending.set(frame.id,frame.method);
    events.push({method:frame.method,...(['CloudCreate','DiscoverModels','DiscoverHarnesses'].includes(frame.method)?{params:frame.params}:{})});
    if(frame.method==='CloudContext'&&faults.failContext){faults.failContext=false;emit(client,{id:frame.id,err:'Cloud catalogue is temporarily unavailable. Retry.',done:true});return;}
   }
   if(client.data.ready)client.data.upstream.send(text);else client.data.waiting.push(text);
  },close(client){client.data.upstream?.close();}
 }});
fs.writeFileSync(ready,`http://127.0.0.1:${server.port}`);
