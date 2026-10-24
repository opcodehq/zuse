// Usage replies come unchanged from the real engine. Provider allowances are isolated.
import fs from 'node:fs';
import path from 'node:path';
import {accounts} from './usage-dashboard-data.mjs';
const [ipc, ready, assets] = process.argv.slice(2), events = [];
const headers = {'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'};
const emit = (client, frame) => { try { client.send(JSON.stringify(frame)); } catch {} };
const server = Bun.serve({hostname:'127.0.0.1',port:0,async fetch(request, server) {
  const url = new URL(request.url);
  if (url.pathname === '/__fixture') return Response.json({events}, {headers});
  if (url.pathname === '/api/session/check' || url.pathname === '/api/session') return Response.json({ok:true}, {headers});
  if (url.pathname === '/api/rpc' || (url.pathname === '/' && request.headers.get('upgrade')?.toLowerCase() === 'websocket')) {
    if (server.upgrade(request,{data:{ready:false,waiting:[],methods:new Map(),upstream:null}})) return;
    return new Response('WebSocket required',{status:400});
  }
  const target = path.resolve(assets, '.' + (url.pathname === '/' ? '/index.html' : url.pathname));
  if (!target.startsWith(path.resolve(assets) + path.sep)) return new Response('Not found',{status:404});
  const file = Bun.file(target);
  return await file.exists() ? new Response(file,{headers}) : new Response('Not found',{status:404});
},websocket:{
  open(client) {
    const upstream = new WebSocket('ws://127.0.0.1:' + ipc); client.data.upstream = upstream;
    upstream.onopen = () => { client.data.ready = true; for (const frame of client.data.waiting) upstream.send(frame); client.data.waiting = []; };
    upstream.onmessage = event => {
      const frame = JSON.parse(event.data), method = client.data.methods.get(frame.id);
      const value = frame.ok ?? frame.item;
      if (method?.startsWith('Usage') && value) events.push({replyTo:frame.id,method,summary:value.summary,total:value.total,sessionCount:value.sessionCount});
      emit(client, frame);
    };
    upstream.onclose = () => client.close();
  },
  message(client, raw) {
    const text = raw.toString(), frame = JSON.parse(text);
    if (frame.method) { events.push({id:frame.id,method:frame.method,params:frame.params ?? {}}); client.data.methods.set(frame.id,frame.method); }
    if (['QueueMessage','QueueCommand','CloudCreate','WriteWorkspaceFile','RunProjectAction'].includes(frame.method)) {
      events.push({blocked:frame.method}); emit(client,{id:frame.id,err:'Execution disabled in this fixture',done:true}); return;
    }
    if (frame.method === 'ListAgentAccounts') { emit(client,{id:frame.id,ok:accounts}); return; }
    if (client.data.ready) client.data.upstream.send(text); else client.data.waiting.push(text);
  },
  close(client) { client.data.upstream?.close(); },
}});
fs.writeFileSync(ready,'http://127.0.0.1:' + server.port);
