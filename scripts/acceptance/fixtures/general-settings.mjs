// Isolated account protocol. All workspace reads use a disposable real engine.
import fs from 'node:fs';
import path from 'node:path';
const [ipc,ready,assets]=process.argv.slice(2);
const clients=new Set(),events=[];
let state={user:null,login:null,error:null},hold=false,failNext=false,loginTimer,cloud=false,holdCloud=false;
const delayed=[];
const emit=(client,frame)=>{try{client.send(JSON.stringify(frame));}catch{}};
const publish=()=>{if(!hold)for(const client of clients)for(const [id,method]of client.data.handled)if(method==='CloudAccountStatus')emit(client,{id,item:state});};
const headers={'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp','Cache-Control':'no-store'};
const server=Bun.serve({hostname:'127.0.0.1',port:0,
 async fetch(request,server){
  const url=new URL(request.url);
  if(url.pathname==='/__fixture'){
   if(request.method==='POST'){
    const value=await request.json();
    if(Object.hasOwn(value,'state'))state=value.state;
    if(Object.hasOwn(value,'hold'))hold=value.hold;
    if(Object.hasOwn(value,'failNext'))failNext=value.failNext;
    if(Object.hasOwn(value,'cloud'))cloud=value.cloud;
    if(Object.hasOwn(value,'holdCloud'))holdCloud=value.holdCloud;
    if(!holdCloud)for(const [client,frame]of delayed.splice(0))emit(client,frame);
    publish();
   }
   return Response.json({state,events,hold},{headers});
  }
  if(url.pathname==='/api/session/check'||url.pathname==='/api/session')return Response.json({ok:true},{headers});
  if(url.pathname==='/api/rpc'){
   if(server.upgrade(request,{data:{handled:new Map(),waiting:[],upstream:null,ready:false}}))return;
   return new Response('WebSocket required',{status:400});
  }
  const target=path.resolve(assets,'.'+(url.pathname==='/'?'/index.html':url.pathname));
  if(!target.startsWith(path.resolve(assets)+path.sep))return new Response('Not found',{status:404});
  const file=Bun.file(target);if(!await file.exists())return new Response('Not found',{status:404});
  return new Response(file,{headers});
 },
 websocket:{
  open(client){
   clients.add(client);const upstream=new WebSocket('ws://127.0.0.1:'+ipc);client.data.upstream=upstream;
   upstream.onopen=()=>{client.data.ready=true;for(const value of client.data.waiting)upstream.send(value);client.data.waiting=[];};
   upstream.onmessage=event=>{try{emit(client,JSON.parse(event.data));}catch{}};
   upstream.onclose=()=>client.close();
  },
  message(client,message){
   const text=message.toString(),frame=JSON.parse(text);
   if(frame.cancel){events.push({cancel:frame.id});if(client.data.handled.delete(frame.id))return;}
   if(frame.method==='CloudAccountStatus'){
    client.data.handled.set(frame.id,frame.method);if(!hold)emit(client,{id:frame.id,item:state});return;
   }
   if(frame.method==='CloudAccountSignIn'){
    events.push({method:frame.method});
    if(failNext){failNext=false;emit(client,{id:frame.id,err:'Account service is unavailable. Check your connection and retry.',done:true});return;}
    clearTimeout(loginTimer);
    loginTimer=setTimeout(()=>{
     state={user:null,login:{url:'https://identity.example.test/device',userCode:'PARITY-CODE',expiresAt:Math.floor(Date.now()/1000)+600},error:null};
     publish();emit(client,{id:frame.id,ok:state.login});
    },cloud?1200:300);return;
   }
   if(frame.method==='CloudAccountSignOut'){
    events.push({method:frame.method});clearTimeout(loginTimer);state={user:null,login:null,error:null};publish();emit(client,{id:frame.id,ok:{ok:true}});return;
   }
   if(cloud&&['CloudProjects','CloudGithub','CloudImage','CloudBillingSummary','CloudBillingUsage','CloudBillingEntitlements','CloudPrepaidBalance','CloudContext','CloudWorkspaces'].includes(frame.method)){
    const owner=state.user?.id??null;
    events.push({id:frame.id,method:frame.method,owner,expectedAccountId:frame.params?.expectedAccountId??null});
    const project={projectId:'project-'+owner,repositoryIdentity:'github.com/'+owner+'/workspace',repositoryUrl:'https://github.com/'+owner+'/workspace',displayName:'Workspace',defaultBranch:'main',visibility:'private',state:'ready',activeBuilds:{},latestBuilds:{},createdAt:1,updatedAt:1};
    const image={storage:null,state:'none',generation:null,providerId:null,runtimeVersion:null,progressPhase:null,errorCode:null,repositories:[],builds:[],builtAt:null,updatedAt:1};
    const summary={currency:'USD',status:'active',periodStart:1790812800000,periodEnd:1793491200000,basePriceMicros:40000000,includedProviderCostMicros:35000000,providerCostMicros:5000000,storageCostMicros:0,includedUsedMicros:5000000,includedRemainingMicros:30000000,overageProviderCostMicros:0,overageChargeMicros:0,overageCapMicros:25000000,markupBasisPoints:500,currentInvoiceEstimateMicros:40000000,usageProvisional:false};
    const data={CloudProjects:{projects:[project]},CloudGithub:{configured:true,canManageInstallations:true,user:null,installations:[],repositories:[]},CloudImage:image,CloudBillingSummary:summary,CloudBillingUsage:{items:[{entryId:'entry-'+owner,resourceKind:'workspace',usageKind:'compute',resourceId:'workspace-'+owner,provider:'boxd',startedAt:1792663200000,endedAt:1792749600000,vcpuCount:2,memoryMib:4096,providerCostMicros:5000000,status:'settled'}],nextCursor:null},CloudBillingEntitlements:{entitlements:[{offerId:'cloud-workspace-standard-v1',status:'active',paidThrough:1793491200000}]},CloudPrepaidBalance:{available:true,creditCents:2500,debitCents:0,currency:'USD'}}[frame.method];
    const value=frame.method==='CloudContext'?{accountId:owner,projects:[]}:frame.method==='CloudWorkspaces'?{accountId:owner,workspaces:[]}:{accountId:owner,organizationId:null,data};
    const response={id:frame.id,ok:value};
    if(holdCloud)delayed.push([client,response]);else emit(client,response);return;
   }
   if(frame.method==='Plugins'){
    emit(client,{id:frame.id,ok:{kind:'snapshot',tenantId:'personal:'+state.user?.id,tenants:[],catalog:[],connections:[],canManage:true,endpoint:'https://fixture.invalid/mcp'}});return;
   }
   if(['QueueMessage','QueueCommand','CloudCreate','CloudBillingCheckout','CloudBillingPortal','CloudPrepaidCheckout'].includes(frame.method)){
    events.push({blocked:frame.method});emit(client,{id:frame.id,err:'Execution is disabled in this fixture',done:true});return;
   }
   if(client.data.ready)client.data.upstream.send(text);else client.data.waiting.push(text);
  },
  close(client){clients.delete(client);client.data.upstream?.close();},
 },
});
fs.writeFileSync(ready,'http://127.0.0.1:'+server.port);
console.log('General settings account fixture ready.');
