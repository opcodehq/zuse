// Hetzner-only operator probe. Ticket arrives on stdin, lives in RAM, is never
// logged/persisted. Stream and terminal contents are reduced to metadata.
import {readFileSync,writeFileSync,constants} from 'node:fs';
let input;
try{input=JSON.parse(readFileSync(0,'utf8'));}catch{console.log(JSON.stringify({metadataOnly:true,ok:false,code:'invalid_probe_input'}));process.exit(1);}
const {grant,sessionId,prefix,action,expectedLabels,forbiddenLabels,expectedMarker,readyFile}=input;
if(grant.protocol!=='zuse-workspace-rust-v1'||grant.rpcProtocol!=='zuse-rpc-v1'||
 grant.wsUrl!==`wss://api-rust-stag.zuse.sh/v1/cloud/workspaces/${grant.workspaceId}/gateway`||
 !/^\.zuse-acceptance\/[a-f0-9]{32}$/.test(prefix)||!['observe','verify'].includes(action))throw Error('invalid_probe_scope');
let next=1,terminalId=null,settled=false,readyResolve,readyReject,frames=0,appends=0,toolCount=0,completed=false;
const calls=new Map(),streams=new Map(),rows=new Map(),start=performance.now();
const methods=new Map();
const report={metadataOnly:true,workspaceId:grant.workspaceId,sessionId,action,realPublicTlsGateway:true};
const socket=new WebSocket(grant.wsUrl,[grant.protocol,grant.credential]);
const ready=new Promise((r,j)=>{readyResolve=r;readyReject=j;});
let failSocket;const fault=new Promise((_,j)=>failSocket=j);fault.catch(()=>{});
const bounded=async(p,ms,code)=>{let t;try{return await Promise.race([p,fault,new Promise((_,j)=>t=setTimeout(()=>j(Error(code)),ms))]);}finally{clearTimeout(t);}};
function call(method,params={}){const id=next++;return bounded(new Promise((r,j)=>{calls.set(id,{r,j});socket.send(JSON.stringify({id,method,params}));}),15000,'rpc_timeout').finally(()=>calls.delete(id));}
function watch(method,params,handler){const id=next++;streams.set(id,handler);methods.set(id,method);socket.send(JSON.stringify({id,method,params}));return id;}
socket.addEventListener('error',()=>{readyReject(Error('gateway_socket_error'));failSocket(Error('gateway_socket_error'));});
socket.addEventListener('close',()=>{if(!settled){readyReject(Error('gateway_closed'));failSocket(Error('gateway_closed'));for(const {j} of calls.values())j(Error('gateway_closed'));}});
socket.addEventListener('message',e=>{
 try{for(const line of String(e.data).split('\n').filter(Boolean)){
  const f=JSON.parse(line);
  if(f._zuseGateway==='availability'){
   if(f.state==='available'&&f.reset===true){report.gatewayReset=true;failSocket(Error('gateway_reconnect_required'));}
   else if(f.state==='available')readyResolve();
   else if(!['pending'].includes(f.state)){failSocket(Error('gateway_reconnect_required'));for(const {j} of calls.values())j(Error('gateway_reconnect_required'));}
   continue;
  }
  const pending=calls.get(f.id);
  if(pending){if(f.err)pending.j(Error('rpc_rejected'));else if(Object.hasOwn(f,'ok'))pending.r(f.ok);}
  if(f.err&&streams.has(f.id)){report.rejectedStream=methods.get(f.id);failSocket(Error('stream_rpc_rejected'));}
  if(f.item&&streams.has(f.id))streams.get(f.id)(f.item);
 }}catch{readyReject(Error('invalid_gateway_frame'));failSocket(Error('invalid_gateway_frame'));}
});
try{
 await bounded(ready,30000,'gateway_ready_timeout');await call('EngineReady');
 report.gatewayReadyMs=performance.now()-start;
 let sessionResolve;const sessionPresent=new Promise(r=>sessionResolve=r);
 watch('WatchChats',{},chats=>{if(Array.isArray(chats)&&chats.some(c=>(c.id??c.chatId??c.chat_id)===sessionId))sessionResolve();});
 await bounded(sessionPresent,30000,'session_identity_timeout');
 report.sessionIdentityObserved=true;
 watch('WatchSessions',{},sessions=>{
  if(!Array.isArray(sessions))return;
  const row=sessions.find(s=>(s.chat_id??s.chatId)===sessionId);
  const turn=row?.last_completed_turn??row?.lastCompletedTurn;
  if(typeof turn==='string')report.lastCompletedTurn=turn;
 });
 if(action==='observe'){
  let resolveComplete;const done=new Promise(r=>resolveComplete=r);
  watch('WatchDocMessages',{chatId:sessionId,openingTail:true},update=>{
   frames++;
   if(Array.isArray(update.reset)){rows.clear();for(const r of update.reset)rows.set(r.id,r);}
   for(const {entry} of update.upsert??[])rows.set(entry.id,entry);
   for(const id of update.remove??[])rows.delete(id);
   for(const a of update.append??[]){appends++;const p=rows.get(a.entry)?.parts?.find(p=>p.id===a.part);if(p&&typeof p.text==='string')p.text+=a.text;}
   const assistants=[...rows.values()].filter(r=>r.role==='assistant');
   toolCount=assistants.flatMap(r=>r.parts??[]).filter(p=>p.call&&p.resolved&&!p.isError).length;
   const full=assistants.flatMap(r=>r.parts??[]).filter(p=>!p.call&&typeof p.text==='string').map(p=>p.text).join('\n');
   if(assistants.some(r=>r.status==='complete')&&full.includes(expectedMarker)&&toolCount>0){completed=true;resolveComplete();}
  });
  if(typeof readyFile!=='string'||!readyFile.startsWith('/etc/zuse/cloud/staging/'))throw Error('private_ready_path_required');
  writeFileSync(readyFile,JSON.stringify({metadataOnly:true,ready:true,sessionId,expectedMarker})+'\n',{mode:0o600,flag:constants.O_WRONLY|constants.O_CREAT|constants.O_EXCL|constants.O_NOFOLLOW});
  await bounded(done,150000,'stream_response_timeout');
  Object.assign(report,{ok:true,canonicalCompleted:completed,resolvedToolCount:toolCount,docFrames:frames,
   appendFrames:appends,observedLiveDeltas:appends>0,userMessageCount:[...rows.values()].filter(r=>r.role==='user').length});
 }else{
  const dir=await call('ListWorkspaceDirectory',{chatId:sessionId,directory:prefix,includeIgnored:true});
  report.filesListed=Array.isArray(dir.entries);
  const edited=await call('ReadWorkspaceFile',{chatId:sessionId,path:prefix+'/edited.txt'});
  report.agentFileRead=typeof edited.text==='string'&&edited.text.length>0&&!edited.truncated;
  const terminal=await call('OpenTerminal',{chatId:sessionId,cols:120,rows:30});terminalId=terminal.id;
  let terminalResolve;const terminalDone=new Promise(r=>terminalResolve=r);let output='';
  watch('SubscribeTerminal',{terminalId,afterSeq:0},item=>{
   if(item.type==='data'){
    output=(output+Buffer.from(item.data,'base64').toString('utf8')).slice(-65536);
    const m=output.match(/(?:^|[\r\n])ZUSE_V2_COUNTS:(\{[^\r\n]*\})/);
    if(m){try{terminalResolve(JSON.parse(m[1]));}catch{}}
   }
  });
  const python=`from pathlib import Path; import json; p=Path(${JSON.stringify(prefix)}); counts={f.stem:len(f.read_text().splitlines()) for f in p.glob('*.log')}; print('ZUSE_'+'V2_COUNTS:'+json.dumps(counts,sort_keys=True))`;
  const command="python3 -c '"+python.replaceAll("'","'\\''")+"'\n";
  await call('WriteTerminal',{terminalId,data:Buffer.from(command).toString('base64')});
  const counts=await bounded(terminalDone,15000,'physical_counter_timeout');
  report.physicalExactlyOnce=expectedLabels.every(k=>counts[k]===1)&&Object.values(counts).every(n=>n===1)&&forbiddenLabels.every(k=>counts[k]===undefined);
  report.removedQueueNeverExecuted=counts.add===undefined&&counts['edited-queued']===undefined;
  report.counterNames=Object.keys(counts).sort();report.counterValues=Object.values(counts);
  await call('CloseTerminal',{terminalId});terminalId=null;
  report.ok=report.filesListed&&report.agentFileRead&&report.physicalExactlyOnce&&report.removedQueueNeverExecuted;
 }
}catch(e){report.ok=false;report.code=/^[a-z_]+$/.test(e.message)?e.message:'gateway_probe_failed';}
finally{
 if(terminalId){try{await call('CloseTerminal',{terminalId});}catch{report.terminalCloseUncertain=true;}}
 report.docFrames=frames;report.appendFrames=appends;report.resolvedToolCount=toolCount;report.canonicalCompleted=completed;report.userMessageCount=[...rows.values()].filter(r=>r.role==='user').length;
 settled=true;socket.close(1000);report.elapsedMs=performance.now()-start;
 console.log(JSON.stringify(report));if(!report.ok)process.exitCode=1;
}
