// Isolated account/cloud protocol. Local workspace reads use a disposable real engine.
import fs from 'node:fs';
import path from 'node:path';
import {providerSettingsFixture} from './provider-settings.mjs';
import {titleSettingsFixture} from './title-settings.mjs';
const [ipc,ready,assets]=process.argv.slice(2);
const clients=new Set(),events=[];
const providerFixture=providerSettingsFixture(events);
const titleFixture=titleSettingsFixture(events);
let state={user:null,login:null,error:null},hold=false,failNext=false,loginTimer,cloud=false,holdCloud=false;
const delayed=[],delayedWrites=[];
let workspaceSync=null;
const syncReplies=[];
let setup={projects:null,github:null,image:null,imageError:null,projectError:null,failNextBuild:false};
let billing={subscribed:true,capMicros:25000000,creditCents:2500,debitCents:0,creditAvailable:true,status:'active',usedMicros:5000000,overageMicros:0,usageCount:1,summaryError:null,entitlementError:null,creditError:null,failNextCap:false,writeDelay:450,holdWrites:false,workspaces:[]};
const summary=()=>({currency:'USD',status:billing.status,periodStart:1790812800000,periodEnd:1793491200000,basePriceMicros:40000000,includedProviderCostMicros:35000000,providerCostMicros:billing.usedMicros,storageCostMicros:0,includedUsedMicros:Math.min(billing.usedMicros,35000000),includedRemainingMicros:Math.max(0,35000000-billing.usedMicros),overageProviderCostMicros:billing.overageMicros,overageChargeMicros:billing.overageMicros,overageCapMicros:billing.capMicros,markupBasisPoints:500,currentInvoiceEstimateMicros:40000000+billing.overageMicros,usageProvisional:false});
const emit=(client,frame)=>{try{client.send(JSON.stringify(frame));}catch{}};
const writeReply=(client,frame)=>{if(billing.holdWrites)delayedWrites.push([client,frame]);else setTimeout(()=>emit(client,frame),billing.writeDelay);};
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
    if(value.billing)billing={...billing,...value.billing};
    if(value.setup)setup={...setup,...value.setup};
    if(value.providers)providerFixture.configure(value.providers);
    if(value.titles)titleFixture.configure(value.titles);
    if(value.workspaceSync)workspaceSync={hold:false,error:'Sync service is unavailable. Retry when connected.',...workspaceSync,...value.workspaceSync};
    if(workspaceSync&&!workspaceSync.hold)for(const [client,frame]of syncReplies.splice(0))emit(client,frame);
    if(Object.hasOwn(value,'holdCloud'))holdCloud=value.holdCloud;
    if(!holdCloud)for(const [client,frame]of delayed.splice(0))emit(client,frame);
    if(!billing.holdWrites)for(const [client,frame]of delayedWrites.splice(0))emit(client,frame);
    publish();
   }
   return Response.json({state,events,hold,billing,setup,workspaceSync,providers:providerFixture.snapshot(),titles:titleFixture.snapshot()},{headers});
  }
  if(url.pathname==='/api/session/check'||url.pathname==='/api/session')return Response.json({ok:true},{headers});
  if(url.pathname==='/api/rpc'||(url.pathname==='/'&&request.headers.get('upgrade')?.toLowerCase()==='websocket')){
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
   upstream.onmessage=event=>{try{const frame=JSON.parse(event.data);if(!titleFixture.receive(client,frame,emit))emit(client,frame);}catch{}};
   upstream.onclose=()=>client.close();
  },
  message(client,message){
   const text=message.toString(),frame=JSON.parse(text);
   if(frame.method==='SignIn'&&workspaceSync){
    events.push({method:frame.method});const reply={id:frame.id,err:workspaceSync.error,done:true};
    if(workspaceSync.hold)syncReplies.push([client,reply]);else emit(client,reply);return;
   }
   if(frame.method==='WatchDocMessages')events.push({method:frame.method,chatId:frame.params?.chatId});
   if(providerFixture.handle(client,frame,emit))return;
   if(titleFixture.admit(client,frame,emit))return;
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
   if(cloud&&['CloudProjects','CloudGithubCatalog','CloudImage','CloudBillingSummary','CloudBillingUsage','CloudBillingEntitlements','CloudPrepaidBalance','CloudContext','CloudWorkspaces'].includes(frame.method)){
    const owner=state.user?.id??null;
    events.push({id:frame.id,method:frame.method,owner,expectedAccountId:frame.params?.expectedAccountId??null,cursor:frame.params?.cursor??null});
    const project={projectId:'project-'+owner,repositoryIdentity:'github.com/'+owner+'/workspace',repositoryUrl:'https://github.com/'+owner+'/workspace',displayName:'Workspace',defaultBranch:'main',visibility:'private',state:'ready',activeBuilds:{},latestBuilds:{},createdAt:1,updatedAt:1};
    const image={storage:null,state:'not-built',generation:null,providerId:null,runtimeVersion:null,progressPhase:null,errorCode:null,repositories:[],builds:[],builtAt:null,updatedAt:1};
    const offset=frame.params?.cursor==='older'?10:0;
    const usage=Array.from({length:Math.min(10,Math.max(0,billing.usageCount-offset))},(_,i)=>({entryId:'entry-'+owner+'-'+(offset+i),resourceKind:'workspace',usageKind:'compute',resourceId:billing.usageCount===1?'workspace-'+owner:'workspace-'+owner+'-'+(offset+i),provider:'boxd',startedAt:1792663200000,endedAt:1792749600000,vcpuCount:2,memoryMib:4096,providerCostMicros:billing.usageCount===1?5000000:1000000,status:'settled'}));
    if(frame.method==='CloudBillingSummary'&&billing.summaryError){emit(client,{id:frame.id,err:billing.summaryError,done:true});return;}
    if(frame.method==='CloudBillingEntitlements'&&billing.entitlementError){emit(client,{id:frame.id,err:billing.entitlementError,done:true});return;}
    if(frame.method==='CloudPrepaidBalance'&&billing.creditError){emit(client,{id:frame.id,err:billing.creditError,done:true});return;}
    if(frame.method==='CloudImage'&&setup.imageError){emit(client,{id:frame.id,err:setup.imageError,done:true});return;}
    if(frame.method==='CloudProjects'&&setup.projectError){emit(client,{id:frame.id,err:setup.projectError,done:true});return;}
    const data={CloudProjects:{projects:setup.projects??[project]},CloudGithubCatalog:setup.github??{configured:true,canManageInstallations:true,user:null,installations:[],repositories:[]},CloudImage:setup.image??image,CloudBillingSummary:summary(),CloudBillingUsage:{items:usage,nextCursor:billing.usageCount>offset+10?'older':null},CloudBillingEntitlements:{entitlements:billing.subscribed?[{offerId:'cloud-workspace-standard-v1',status:'active',paidThrough:1793491200000}]:[]},CloudPrepaidBalance:{available:billing.creditAvailable,creditCents:billing.creditCents,debitCents:billing.debitCents,currency:'USD'}}[frame.method];
    const value=frame.method==='CloudContext'?{accountId:owner,projects:setup.projects??[]}:frame.method==='CloudWorkspaces'?{accountId:owner,workspaces:billing.workspaces}:{accountId:owner,organizationId:state.orgId??null,data};
    const response={id:frame.id,ok:value};
    if(holdCloud)delayed.push([client,response]);else emit(client,response);return;
   }
   if(cloud&&['CloudProjectConnect','CloudProjectRemove','CloudImageBuild','CloudImageDelete'].includes(frame.method)){
    const owner=state.user?.id??null,p=frame.params??{},organization=state.orgId??null;
    events.push({id:frame.id,method:frame.method,owner,expectedAccountId:p.expectedAccountId,expectedOrganizationId:p.expectedOrganizationId,params:p});
    if(!owner||owner!==p.expectedAccountId||organization!==(p.expectedOrganizationId??null)||(organization&&setup.github?.canManageInstallations===false)){emit(client,{id:frame.id,err:'This account cannot manage cloud settings.',done:true});return;}
    let data;
    if(frame.method==='CloudProjectConnect'){
     const identity=p.repositoryUrl.replace(/^https:\/\//,'').replace(/\.git$/,'');
     data={projectId:'connected-'+identity.replaceAll('/','-'),repositoryIdentity:identity,repositoryUrl:p.repositoryUrl,displayName:identity.replace('github.com/',''),defaultBranch:p.defaultBranch,visibility:p.visibility,state:'ready',activeBuilds:{},latestBuilds:{},createdAt:1792795200000,updatedAt:1792795200000};
     setup.projects=[...(setup.projects??[]).filter(v=>v.projectId!==data.projectId),data];if(setup.image)setup.image.state='outdated';
    }else if(frame.method==='CloudProjectRemove'){
     data=(setup.projects??[]).find(v=>v.projectId===p.projectId);setup.projects=(setup.projects??[]).filter(v=>v.projectId!==p.projectId);if(setup.image)setup.image.state='outdated';
    }else if(frame.method==='CloudImageBuild'){
     const now=1792795200000;const build={buildId:'requested-'+p.idempotencyKey,state:'building',mode:p.mode,active:false,logText:'Preparing repositories and agents…',errorCode:null,providerId:'boxd',runtimeVersion:'runtime-2026-10',progressPhase:'prepare',configurationDigest:'fixture',repositories:(setup.projects??[]).map(p=>({projectId:p.projectId,repositoryIdentity:p.repositoryIdentity,displayName:p.displayName,defaultBranch:p.defaultBranch,sourceCommit:'abc123'})),createdAt:now,updatedAt:now};
     setup.image={...(setup.image??{}),storage:setup.image?.storage??null,state:'building',generation:setup.image?.generation??null,providerId:'boxd',runtimeVersion:'runtime-2026-10',progressPhase:'prepare',errorCode:null,repositories:[],builds:[build,...(setup.image?.builds??[]).filter(b=>b.buildId!==build.buildId)],builtAt:null,updatedAt:now};data=setup.image;
     if(setup.failNextBuild){setup.failNextBuild=false;setup.image.state='not-built';writeReply(client,{id:frame.id,err:'Image request was interrupted. Retry to check the same build.',done:true});return;}
    }else{setup.image={...setup.image,state:'not-built',storage:null};data=setup.image;}
    writeReply(client,{id:frame.id,ok:{accountId:owner,organizationId:organization,data}});return;
   }
   if(cloud&&['CloudBillingPortal','CloudBillingCheckout','CloudPrepaidCheckout','CloudBillingSetCap'].includes(frame.method)){
    const owner=state.user?.id??null,params=frame.params??{};
    events.push({id:frame.id,method:frame.method,owner,expectedAccountId:params.expectedAccountId,amountCents:params.amountCents,overageCapMicros:params.overageCapMicros,idempotencyKey:params.idempotencyKey});
    if(!owner||owner!==params.expectedAccountId){emit(client,{id:frame.id,err:'Your account changed. Reopen this view.',done:true});return;}
    let data;
    if(frame.method==='CloudBillingSetCap'){
     billing.capMicros=params.overageCapMicros;
     if(billing.failNextCap){billing.failNextCap=false;writeReply(client,{id:frame.id,err:'Could not confirm the saved cap. Retry to check the request.',done:true});return;}
     data=summary();
    }else{data={checkoutUrl:'https://billing.example.test/'+({CloudBillingPortal:'portal',CloudBillingCheckout:'subscription',CloudPrepaidCheckout:'credits'}[frame.method])};}
    writeReply(client,{id:frame.id,ok:{accountId:owner,organizationId:state.orgId??null,data}});return;
   }
   if(frame.method==='Plugins'){
    emit(client,{id:frame.id,ok:{kind:'snapshot',tenantId:'personal:'+state.user?.id,tenants:[],catalog:[],connections:[],canManage:true,endpoint:'https://fixture.invalid/mcp'}});return;
   }
   if(['QueueMessage','QueueCommand','CloudCreate','CloudBillingCheckout','CloudBillingPortal','CloudPrepaidCheckout','CloudBillingSetCap'].includes(frame.method)){
    events.push({blocked:frame.method});emit(client,{id:frame.id,err:'Execution is disabled in this fixture',done:true});return;
   }
   if(client.data.ready)client.data.upstream.send(text);else client.data.waiting.push(text);
  },
  close(client){clients.delete(client);client.data.upstream?.close();},
 },
});
fs.writeFileSync(ready,'http://127.0.0.1:'+server.port);
console.log('General settings account fixture ready.');
