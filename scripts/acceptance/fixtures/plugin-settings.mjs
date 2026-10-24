// Isolated plugin protocol fixture. Other RPCs use the disposable real engine.
import fs from 'node:fs';
import path from 'node:path';
const [ipc, ready, assets] = process.argv.slice(2);
const clients = new Set(), events = [];
let account = 'plugin-ui-owner', canManage = true, sequence = 1, attempt = null, approved = false;
let removalMode='ok', removalRelease;
let catalog = [
  {id:'figma',name:'Figma',description:'Bring designs and components into your agent workflow.',domain:'figma.com',featured:true,custom:false},
  {id:'workspace-tools',name:'Workspace tools',description:'Tools from your custom MCP server.',domain:'',custom:true,url:'https://tools.example.test/mcp'},
  {id:'metrics',name:'Metrics',description:'Query project metrics with an API key.',domain:'',apiKeyHeader:'X-API-Key',custom:false},
];
let connections = [
  {id:'figma-one',pluginId:'figma',label:'Figma',owner:account,state:'connected',createdAt:Date.UTC(2026,9,20)+0.5,enabled:true},
  {id:'tools-one',pluginId:'workspace-tools',label:'Workspace tools',owner:account,state:'connected',createdAt:Date.UTC(2026,9,20)+0.5,enabled:true},
];
const snapshot = () => ({kind:'snapshot',canManage,tenants:[{id:'personal:'+account,kind:'personal',name:'Personal'}],tenantId:'personal:'+account,endpoint:'https://fixture.invalid/mcp',catalog,connections});
const emit = (client, value) => {try {client.send(JSON.stringify(value));} catch {}};
const accountValue = () => ({user:account?{id:account,email:account+'@example.test',name:'Avery'}:null,login:null,error:null});
const headers = {'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'};
const server = Bun.serve({hostname:'127.0.0.1',port:0,
  async fetch(request, server) {
    const url=new URL(request.url);
    if (url.pathname==='/__fixture') {
      if(request.method==='POST') {
        const value=await request.json();
        if(Object.hasOwn(value,'account')) {
          account=value.account;connections=[];attempt=null;
          for(const client of clients)for(const [id,method]of client.data.handled)if(method==='CloudAccountStatus')emit(client,{id,item:accountValue()});
        }
        if(Object.hasOwn(value,'canManage'))canManage=value.canManage;
        if(Object.hasOwn(value,'approve'))approved=value.approve;
        if(Object.hasOwn(value,'removalMode')){removalMode=value.removalMode;if(removalMode!=='hold'){removalRelease?.();removalRelease=null;}}
      }
      return Response.json({events,snapshot:snapshot(),attempt},{headers});
    }
    if(url.pathname==='/api/session/check'||url.pathname==='/api/session')return Response.json({ok:true},{headers});
    if(url.pathname==='/api/rpc'||(url.pathname==='/'&&request.headers.get('upgrade')?.toLowerCase()==='websocket')) {
      if(server.upgrade(request,{data:{handled:new Map(),waiting:[],upstream:null,ready:false}}))return;
      return new Response('WebSocket required',{status:400});
    }
    const target=path.resolve(assets,'.'+(url.pathname==='/'?'/index.html':url.pathname));
    if(!target.startsWith(path.resolve(assets)+path.sep))return new Response('Not found',{status:404});
    const file=Bun.file(target);if(!await file.exists())return new Response('Not found',{status:404});
    return new Response(file,{headers});
  },
  websocket: {
    open(client) {
      clients.add(client);
      const upstream=new WebSocket('ws://127.0.0.1:'+ipc);client.data.upstream=upstream;
      upstream.onopen=()=>{client.data.ready=true;for(const value of client.data.waiting)upstream.send(value);client.data.waiting=[];};
      upstream.onmessage=event=>{try{emit(client,JSON.parse(event.data));}catch{}};
      upstream.onclose=()=>client.close();
    },
    async message(client, message) {
      const text=message.toString(),frame=JSON.parse(text),p=frame.params??{};
      if(frame.cancel&&client.data.handled.delete(frame.id))return;
      if(frame.method==='CloudAccountStatus') {
        client.data.handled.set(frame.id,frame.method);emit(client,{id:frame.id,item:accountValue()});return;
      }
      if(['QueueMessage','QueueCommand','CloudCreate'].includes(frame.method)) {
        events.push({blocked:frame.method});emit(client,{id:frame.id,err:'Provider execution is disabled in this fixture',done:true});return;
      }
      if(frame.method==='Plugins') {
        events.push({action:p.action,tenantId:p.tenantId,pluginId:p.pluginId,connectionId:p.connectionId,enabled:p.enabled,label:p.label,name:p.name,url:p.url,apiKeyHeader:typeof p.apiKey==='object'?p.apiKey?.header:undefined,apiKeySubmitted:!!p.apiKey});
        let response;
        if(!account) {emit(client,{id:frame.id,err:'Sign in first',done:true});return;}
        if(p.action!=='list'&&!canManage) {emit(client,{id:frame.id,err:'Read-only workspace',done:true});return;}
        if(p.action==='list')response=snapshot();
        else if(p.action==='setEnabled') {connections=connections.map(c=>c.id===p.connectionId?{...c,enabled:p.enabled}:c);response={kind:'ok'};}
        else if(p.action==='disconnect') {connections=connections.filter(c=>c.id!==p.connectionId);response={kind:'ok'};}
        else if(p.action==='removeServer') {
          if(removalMode==='hold')await new Promise(resolve=>{removalRelease=resolve;});
          if(removalMode==='fail'){emit(client,{id:frame.id,err:'The fixture server could not be removed',done:true});return;}
          catalog=catalog.filter(c=>c.id!==p.pluginId);connections=connections.filter(c=>c.pluginId!==p.pluginId);response={kind:'ok'};}
        else if(p.action==='connect') {
          approved=false;
          if(p.pluginId==='metrics') {
            if(p.apiKey!=='fixture-plugin-key') {emit(client,{id:frame.id,err:'Expected the fixture API key',done:true});return;}
            connections.push({id:'metrics-'+(++sequence),pluginId:p.pluginId,label:p.label,owner:account,state:'connected',createdAt:Date.UTC(2026,9,20)+0.5,enabled:true});response={kind:'attempt',id:'attempt-'+sequence,connectionId:'metrics-'+sequence,pluginId:p.pluginId,state:'connected',authorizationUrl:null,expiresAt:Date.now()+60000};
          } else {
            attempt={id:'attempt-'+(++sequence),connectionId:'connection-'+sequence,pluginId:p.pluginId,label:p.label,state:'pending'};
            response={kind:'attempt',...attempt,authorizationUrl:'https://auth.example.test/connect',expiresAt:Date.now()+60000};
          }
        } else if(p.action==='addServer') {
          approved=false;
          if(p.apiKey?.header!=='X-Parity-Key'||p.apiKey?.value!=='fixture-plugin-key') {emit(client,{id:frame.id,err:'Expected the fixture server key',done:true});return;}
          const id='custom-'+(++sequence);
          catalog.push({id,name:p.name,url:p.url,description:'Custom server added in UI acceptance.',domain:'',custom:true,apiKeyHeader:p.apiKey.header});
          attempt={id:'attempt-'+sequence,connectionId:'connection-'+sequence,pluginId:id,label:p.name,state:'pending'};
          response={kind:'attempt',...attempt,authorizationUrl:'https://auth.example.test/connect',expiresAt:Date.now()+60000};
        } else if(p.action==='poll'&&attempt) {
          if(!approved){emit(client,{id:frame.id,ok:{kind:'attempt',...attempt,authorizationUrl:'https://auth.example.test/connect',expiresAt:Date.now()+60000}});return;}
          connections.push({id:attempt.connectionId,pluginId:attempt.pluginId,label:attempt.label,owner:account,state:'connected',createdAt:Date.UTC(2026,9,20)+0.5,enabled:true});
          response={kind:'attempt',...attempt,state:'connected',authorizationUrl:null,expiresAt:Date.now()+60000};attempt=null;
        } else if(p.action==='cancel') {attempt=null;response={kind:'ok'};}
        else {emit(client,{id:frame.id,err:'Unexpected fixture action',done:true});return;}
        emit(client,{id:frame.id,ok:response});return;
      }
      if(frame.method==='CloudAccountSignOut') {
        account=null;connections=[];attempt=null;emit(client,{id:frame.id,ok:{ok:true}});
        for(const [id,method]of client.data.handled)if(method==='CloudAccountStatus')emit(client,{id,item:accountValue()});return;
      }
      if(client.data.ready)client.data.upstream.send(text);else client.data.waiting.push(text);
    },
    close(client) {clients.delete(client);client.data.upstream?.close();},
  },
});
fs.writeFileSync(ready,'http://127.0.0.1:'+server.port);
console.log('Plugin UI protocol fixture ready.');
