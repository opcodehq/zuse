// Matched shipped T3 and compiled Zuse UI flows; disposable profiles, no model API.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {randomUUID,createHash} from 'node:crypto';
import {sleep} from './harness.mjs';
import {matchedChatData} from './fixtures/matched-chat-data.mjs';
import {cdpBrowser,until} from './fixtures/cdp-browser.mjs';
import {socketDelayProxy} from './fixtures/socket-delay-proxy.mjs';

const runs=Number(process.env.BENCH_RUNS??3),fast=process.env.BENCH_FAST==='1';
const rows=Number(process.env.BENCH_HISTORY_ROWS??1500);
const apps=(process.env.BENCH_APPS??'t3,zuse').split(',');
const root=path.resolve('.context/chat-comparison-'+randomUUID());fs.mkdirSync(root,{recursive:true});
const output=path.resolve(process.env.BENCH_OUTPUT??'.context/t3-benchmark/chat-results.json');
const profile={latencyMs:fast?0:400,downloadBytesPerSecond:fast?10000000:50000,uploadBytesPerSecond:fast?10000000:25000,socketRoundTripMs:fast?0:400};
const report={createdAt:new Date().toISOString(),profile,artifacts:root,t3Version:'0.1.7',t3Commit:'322f6544dfad0a3c8a0c8cbecf2c7f7874b14268',method:'Both real browser UIs, identical 1500-message canonical text/roles, same local Claude-protocol streaming fixture, 40 chunks at 50 ms. Application shell loaded without throttling; chat actions under bandwidth limits and explicit WebSocket delay. Initial visible window sizes follow each product defaults. History measurement is receipt plus two animation frames, not a cross-renderer paint proof.',samples:[]};
report.networkLimiter='Aggregate encoded TCP bytes at loopback proxy, including native permessage-deflate; no browser-level bandwidth throttle';
report.streamFixture={chunks:40,chunkIntervalMs:50,completedParagraphEveryChunks:5,promptEcho:true,syntheticToolOutputs:3,outputBytesEach:32794};
report.zuseBuild={serverProfile:'dev (debug)',browserProfile:'web-release',serverBytes:fs.statSync('target/debug/zuse-cli').size,serverModifiedAt:fs.statSync('target/debug/zuse-cli').mtime.toISOString(),wasm:fs.readdirSync('dist/web').filter(name=>name.endsWith('.wasm')).map(name=>({name,sha256:createHash('sha256').update(fs.readFileSync(path.join('dist/web',name))).digest('hex')}))};
const codec=fs.readFileSync('apps/web/src/rpc_compression.js','utf8').replaceAll('export ','');
const observer='(()=>{'+codec+String.raw`
 const p=window.__chatBench={stage:'startup',calls:[],completedCalls:[],received:[],http:[],snapshots:[],pages:[],opens:0,closes:[],lastOpenAt:0,methods:{},packets:[],firstTextAt:null,decodeErrors:[]};
 const sockets=[];Object.defineProperty(p,'sockets',{enumerable:false,value:sockets});
 const encode=new TextEncoder();
 function receive(text,source,request,receivedAt,receivedStage,encodedBytes){
  const at=receivedAt??performance.now(),stage=receivedStage??p.stage,bytes=encode.encode(text).length;
  p.received.push({at,bytes,source,stage,request});
  const snapshot=text.includes('"reset":')||text.includes('"historyResumed":true')||text.includes('"projection":');
  for(const match of text.matchAll(/Network packet ([0-9]{4})[.]/g)){p.packets.push({index:Number(match[1]),at,stage,source,snapshot});if(!snapshot&&(stage==='stream'||stage==='stop-start'))p.firstTextAt??=at;}
  try{for(const f of text.split('\n').filter(Boolean).flatMap(line=>[].concat(JSON.parse(line)))){
   if(Object.hasOwn(f,'ok'))p.completedCalls.push(f.id);
   const values=f.values??[f.item??f.ok??f];
   for(const v of values){if(!v)continue;
    const snapshot=v.snapshot??v;
    if(Array.isArray(v.items)&&'nextCursor' in v)p.pages.push({at,bytes,source,stage,rows:v.items.length,hasMore:v.hasMoreHistory,cursor:v.nextCursor});
    if(((v.reset||v.historyResumed)&&p.methods[f.id]==='WatchDocMessages')||snapshot.projection?.thread){
     const entries=v.reset??snapshot.projection?.messages??[];
     p.snapshots.push({at,bytes,encodedBytes,source,stage,rows:v.historyRows??entries.length,resumed:!!v.historyResumed,hasMore:v.historyHasMore??snapshot.hasMoreHistory,total:v.historyTotal,nextAnchor:v.historyNextAnchor,cursor:snapshot.historyCursor,sequence:snapshot.snapshotSequence});
    }
   }
  }}catch{}
 }
 const Native=WebSocket;
 let decoded=Promise.resolve();
 window.WebSocket=class extends Native{
  constructor(...args){super(...args);const decoder=createRpcDecoder();sockets.push(this);this.addEventListener('open',()=>{p.opens++;p.lastOpenAt=performance.now();});this.addEventListener('close',e=>p.closes.push({at:performance.now(),code:e.code,reason:e.reason,clean:e.wasClean,stage:p.stage}));this.addEventListener('message',e=>{const at=performance.now(),stage=p.stage;decoded=decoded.then(async()=>{let text=e.data,encodedBytes;if(typeof text!=='string'){const bytes=new Uint8Array(e.data instanceof Blob?await e.data.arrayBuffer():e.data);encodedBytes=bytes.length;text=(bytes[0]===31&&bytes[1]===139)||(bytes[0]===90&&bytes[1]===68)?await decodeRpcFrame(bytes,16*1024*1024,decoder):new TextDecoder().decode(bytes);}receive(text,'ws',undefined,at,stage,encodedBytes);}).catch(error=>p.decodeErrors.push(String(error)));});}
  send(value){try{for(const f of [].concat(JSON.parse(value))){const method=f.method??f.tag;if(method){p.methods[f.id]=method;p.calls.push({id:f.id,method,params:f.params??f.payload,at:performance.now(),stage:p.stage});}}}catch{}super.send(value);}
 };
 const nativeFetch=window.fetch;
 window.fetch=async function(...args){const url=typeof args[0]==='string'?args[0]:args[0].url??String(args[0]);const started=performance.now();const stage=p.stage;const r=await nativeFetch.apply(this,args);if(url.includes('/api/orchestration/'))r.clone().text().then(text=>{p.http.push({url:new URL(url,location.href).pathname,stage,started,at:performance.now(),bytes:encode.encode(text).length,status:r.status});receive(text,'http',url.split('?')[0]);});return r;};
})();`;

function providerEvents(file){return fs.existsSync(file)?fs.readFileSync(file,'utf8').trim().split('\n').filter(Boolean).map(JSON.parse):[];}
async function measure(app,run,data,sampleRoot){
 const proxy=await socketDelayProxy(app==='t3'?data.t3.origin:data.zuse.url,profile.socketRoundTripMs);
 let browser;
 try{browser=await cdpBrowser(sampleRoot);}catch(error){await proxy.close();throw error;}
 const {call,evaluate,click,type,screenshot}=browser;
 let stage='startup';const result={app,run};
 const proof=()=>evaluate('window.__chatBench');
 const setStage=async value=>{stage=value;console.error(app,run,value);proxy.setStage(value);await evaluate('window.__chatBench.stage='+JSON.stringify(value));};
 const evidence=()=>{const p=fs.existsSync(path.join(app==='t3'?data.t3.root:data.zuse.root,'provider-seen.jsonl'));return p?providerEvents(path.join(app==='t3'?data.t3.root:data.zuse.root,'provider-seen.jsonl')):[];};
 try{
  await call('Page.addScriptToEvaluateOnNewDocument',{source:(app==='t3'?`localStorage.setItem('t3code:client-settings:v1',JSON.stringify({onboardingCompletedAt:'2026-11-09T00:00:00.000Z'}));`:'')+observer});
  if(app==='t3')await call('Network.setCookies',{cookies:data.t3.cookies.map(cookie=>{const first=cookie.split(';')[0],at=first.indexOf('=');return {name:first.slice(0,at),value:first.slice(at+1),url:new URL(proxy.url).origin,httpOnly:true,sameSite:'Lax'};})});
  await call('Page.navigate',{url:proxy.url});
  await until(async()=>app==='t3'?await evaluate('document.body?.innerText.includes("3G matched chat")'):(await proof())?.calls.some(c=>c.method==='WatchChats'),app+' app shell',90000);
  await sleep(1200);
  fs.writeFileSync(path.join(sampleRoot,'initial-dom.txt'),await evaluate('document.body.innerText'));
  // The proxy meters compressed wire bytes. Browser-level WS throttling
  // can otherwise charge decoded payloads and penalize compression.
  proxy.setNetworkProfile(profile);
  await setStage('open');
  const started=await evaluate('performance.now()');
  if(app==='zuse')await click(100,168);
  else {
    assert(await evaluate(`(()=>{const row=document.querySelector('[data-thread-item]');const title=[...(row?.querySelectorAll('span')??[])].find(e=>e.textContent.trim()==='3G matched chat');const item=title??row?.querySelector('[role="button"]');if(!item)return false;item.click();return true})()`),'T3 thread navigation exists');
  }
  await until(async()=>(await proof()).snapshots.some(s=>s.stage==='open'),app+' initial chat snapshot',45000);
  await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
  const first=(await proof()).snapshots.find(s=>s.stage==='open');
  result.chatOpenMs=first.at-started;result.chatOpenWithFrameWaitMs=await evaluate('performance.now()')-started;result.initialHistory=first;
  assert(first.rows>0&&first.rows<rows,'Large history arrives as a bounded window');
  await sleep(500);await screenshot('history');
  await setStage('older');
  if(app==='zuse'){
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',x:760,y:250});
    await call('Input.dispatchMouseEvent',{type:'mouseWheel',x:760,y:250,deltaX:0,deltaY:-1000000});
    await sleep(750);
  }else await evaluate(`document.querySelector('button[aria-label="Load earlier turns"]')?.scrollIntoView({block:'center'})`);
  const olderStart=await evaluate('performance.now()');
  if(app==='zuse')await click(760,115);
  else assert(await evaluate(`(()=>{const button=document.querySelector('button[aria-label="Load earlier turns"]');if(!button)return false;button.click();return true})()`),'T3 earlier history control');
  await until(async()=>{const p=await proof();return app==='zuse'?p.snapshots.some(s=>s.stage==='older'):p.pages.some(s=>s.stage==='older');},app+' older history');
  const olderProof=await proof();const older=(app==='zuse'?olderProof.snapshots:olderProof.pages).find(s=>s.stage==='older');
  result.olderHistoryMs=older.at-olderStart;result.olderHistory=older;
  await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
  // Exercise the real provider launch and incremental rendering through each composer.
  await setStage('stream');await evaluate('window.__chatBench.firstTextAt=null');
  if(app==='zuse')await click(600,900);else assert(await evaluate(`(()=>{const editor=document.querySelector('[contenteditable="true"][role="textbox"],.tiptap[contenteditable="true"],[contenteditable="true"]');if(!editor)return false;editor.focus();return true})()`),'T3 composer exists');
  const sendStart=await evaluate('performance.now()');await type('BENCHMARK stream');
  await until(async()=>(await proof()).firstTextAt!==null,app+' first stream append',45000);
  result.sendToFirstAppendMs=(await proof()).firstTextAt-sendStart;
  await until(async()=>(await proof()).packets.some(p=>p.stage==='stream'&&p.index===39),app+' complete fixture text');
  await sleep(1000);
  const afterStream=await proof();
  assert.equal(new Set(afterStream.packets.filter(p=>p.stage==='stream').map(p=>p.index)).size,40,'All fixture chunks reached the browser');
  result.streamDecodedWsBytes=afterStream.received.filter(f=>f.stage==='stream'&&f.source==='ws').reduce((n,f)=>n+f.bytes,0);
  result.streamWsFrames=afterStream.received.filter(f=>f.stage==='stream'&&f.source==='ws').length;
  await screenshot('stream-complete');
  await setStage('tools');
  if(app==='zuse')await click(600,900);else await evaluate(`document.querySelector('[contenteditable="true"]').focus()`);
  await type('BENCHMARK tools');
  await until(async()=>(await proof()).packets.some(p=>p.stage==='tools'&&p.index===39),app+' tool fixture complete');
  await sleep(1000);
  const toolProof=await proof();
  result.toolTraceDecodedWsBytes=toolProof.received.filter(f=>f.stage==='tools'&&f.source==='ws').reduce((n,f)=>n+f.bytes,0);
  const outputs=evidence().filter(e=>e.event==='tool-output');assert.equal(outputs.length,3,'Three distinct synthetic outputs');
  result.toolOutputOriginalBytes=outputs.reduce((n,e)=>n+e.bytes,0);
  if(app==='zuse'){
    const transcript=await data.zuse.call('WatchDocMessages',{chatId:data.chatId});
    const tools=transcript.reset.flatMap(entry=>entry.parts).filter(part=>part.kind==='tool'&&part.outputRef);
    assert.equal(tools.length,3,'Zuse stores on-demand output refs');
    for(const tool of tools){
      const full=await data.zuse.call('FetchToolBlob',{blobRef:tool.outputRef,offset:0,limit:65536});
      const digest=createHash('sha256').update(full.text).digest('hex');
      assert(outputs.some(output=>output.sha256===digest),'Zuse output byte integrity');
      assert.equal(Buffer.byteLength(full.text),outputs[0].bytes,'Zuse full output length');
      tool.verifiedDigest=digest;
    }
    assert.equal(new Set(tools.map(tool=>tool.verifiedDigest)).size,3,'Distinct tool outputs keep separate payloads');
    result.toolDetailsOnDemandBytes=outputs[0].bytes;
    for(const tool of tools)delete tool.verifiedDigest;
    result.routineToolOutputBytes=Buffer.byteLength(JSON.stringify(tools));
  }else{
    const projection=await data.t3.call('orchestration.getThreadProjection',{threadId:data.threadId});
    const tools=projection.turnItems.filter(item=>item.type==='command_execution');assert.equal(tools.length,3,'T3 retains command items');
    assert(tools.every(item=>!item.output),'T3 omits full output from ordinary projection');
    const digests=[];
    for(const tool of tools){
      const full=await data.t3.call('orchestration.getTurnItem',{threadId:data.threadId,itemId:tool.id});
      const digest=createHash('sha256').update(full.item.output).digest('hex');
      assert(outputs.some(output=>output.sha256===digest),'T3 output byte integrity');
      assert.equal(Buffer.byteLength(full.item.output),outputs[0].bytes,'T3 full output length');
      digests.push(digest);
    }
    assert.equal(new Set(digests).size,3,'Distinct T3 tool outputs keep separate payloads');
    result.toolDetailsOnDemandBytes=outputs[0].bytes;
    result.routineToolOutputBytes=Buffer.byteLength(JSON.stringify(tools));
  }
  result.toolDetailsPreserved=true;
  result.toolOutputsVerified=3;
  await setStage('stop-start');await evaluate('window.__chatBench.firstTextAt=null');
  if(app==='zuse')await click(600,900);else await evaluate(`document.querySelector('[contenteditable="true"]').focus()`);
  await type('BENCHMARK stop');
  await until(async()=>(await proof()).firstTextAt!==null,app+' second fixture streams');
  // Reconnect an accepted turn, not an in-flight prompt acknowledgment.
  // Otherwise preserved uncertain drafts move the Stop control beside Send.
  if(app==='zuse')await until(async()=>{const p=await proof();const commands=p.calls.filter(c=>c.stage==='stop-start'&&['QueueCommand','QueueMessage'].includes(c.method));return commands.length>0&&commands.every(c=>p.completedCalls.includes(c.id));},'Zuse prompt accepted before disconnect');
  await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
  // Reconnect while the same long-running fixture is still emitting.
  await setStage('reconnect');const before=await proof();const reconnectStart=await evaluate('performance.now()');
  await evaluate('window.__chatBench.sockets.forEach(socket=>socket.close())');
  await until(async()=>(await proof()).opens>before.opens,app+' reconnect socket');
  const reopened=(await proof()).lastOpenAt;
  if(app==='zuse')await until(async()=>(await proof()).snapshots.some(p=>p.stage==='reconnect'&&p.at>=reopened),app+' reconnect authoritative history');
  await until(async()=>(await proof()).packets.some(p=>p.stage==='reconnect'&&!p.snapshot&&p.at>=reopened),app+' live text resumes',45000);
  result.runningAgentReconnectMs=(await proof()).packets.find(p=>p.stage==='reconnect'&&!p.snapshot&&p.at>=reopened).at-reconnectStart;
  await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
  const executions=evidence().filter(e=>e.event==='run');assert.equal(executions.length,3,'Reconnect must not execute an agent twice');
  result.noDuplicateExecution=true;
  if(app==='t3')await until(async()=>await evaluate(`(()=>{const button=document.querySelector('button[aria-label="Stop generation"]');return !!button&&!button.disabled&&button.getAttribute('aria-disabled')!=='true'})()`),'T3 Stop control enabled',15000);
  await setStage('stop');const stopStart=Date.now();
  if(app==='zuse')await click(1178,960);
  else assert(await evaluate(`(()=>{const button=document.querySelector('button[aria-label="Stop generation"]');if(!button)return false;button.click();return true})()`),'T3 Stop generation control exists');
  await until(()=>evidence().some(e=>e.event==='interrupt'),app+' provider interrupted',15000);
  result.stopAcknowledgedMs=Date.now()-stopStart;
  await sleep(500);
  result.providerExecutionCount=evidence().filter(e=>e.event==='run').length;
  result.providerInterruptCount=evidence().filter(e=>e.event==='interrupt').length;
  result.checks={history:true,olderHistory:true,allTextChunks:true,stream:true,toolOutputOmission:true,fullToolOutputRetrievable:true,runningAgentReconnect:true,noReplay:true,stop:true};
  const final=await proof();assert.equal(final.decodeErrors.length,0,'Wire observer decodes every frame');fs.writeFileSync(path.join(sampleRoot,'wire-observation.json'),JSON.stringify(final,null,2));
  result.phases=Object.fromEntries(['open','older','stream','tools','stop-start','reconnect','stop'].map(phase=>[phase,{decodedWsBytes:final.received.filter(f=>f.stage===phase&&f.source==='ws').reduce((n,f)=>n+f.bytes,0),decodedHttpBytes:final.http.filter(f=>f.stage===phase).reduce((n,f)=>n+f.bytes,0),wsFrames:final.received.filter(f=>f.stage===phase&&f.source==='ws').length,httpResponses:final.http.filter(f=>f.stage===phase).length}]));
  for(const [phase,value] of Object.entries(result.phases)){
    value.encodedWsDownloadBytes=proxy.traffic.filter(f=>f.stage===phase&&f.direction==='download').reduce((n,f)=>n+f.bytes,0);
    value.encodedWsUploadBytes=proxy.traffic.filter(f=>f.stage===phase&&f.direction==='upload').reduce((n,f)=>n+f.bytes,0);
  }
  result.webSocketDeflate=proxy.webSocketDeflate;
  fs.writeFileSync(path.join(sampleRoot,'socket-wire-counts.json'),JSON.stringify(proxy.traffic,null,2));
  result.httpTransfers=browser.events.filter(e=>e.method==='Network.loadingFinished').map(e=>{const response=browser.events.find(r=>r.method==='Network.responseReceived'&&r.params.requestId===e.params.requestId)?.params.response;return response?{url:new URL(response.url).pathname,encodedBytes:e.params.encodedDataLength,encoding:response.headers['content-encoding']??response.headers['Content-Encoding']??'identity'}:null;}).filter(Boolean).filter(r=>r.url.includes('/api/orchestration/'));
  result.artifacts=sampleRoot;await screenshot('finished');
  return result;
 }catch(error){result.failure={stage,message:error.message};try{await screenshot('failure');fs.writeFileSync(path.join(sampleRoot,'failure-proof.json'),JSON.stringify({proof:await proof(),body:await evaluate('document.body.innerText'),sockets:await evaluate('window.__chatBench.sockets.map(s=>({origin:new URL(s.url).origin,state:s.readyState}))'),buttons:await evaluate('[...document.querySelectorAll("button")].map(e=>({text:e.innerText,aria:e.getAttribute("aria-label"),title:e.title,disabled:e.disabled}))')},null,2));fs.writeFileSync(path.join(sampleRoot,'browser-errors.private.json'),JSON.stringify(browser.events.filter(e=>e.method==='Runtime.exceptionThrown'||e.method==='Runtime.consoleAPICalled'||e.method==='Network.webSocketFrameError')), {mode:0o600});}catch{}fs.writeFileSync(path.join(sampleRoot,'failure.json'),JSON.stringify(result,null,2));throw error;
 }finally{await browser.close();await proxy.close();}
}

try{
 for(let run=1;run<=runs;run++){
  const fixtureRoot=path.join(root,'fixture-'+run);fs.mkdirSync(fixtureRoot,{recursive:true});let data;
  try{data=await matchedChatData(fixtureRoot,rows);report.fixture=data.fixture;
   for(const app of (run%2?['t3','zuse']:['zuse','t3']).filter(app=>apps.includes(app))){const sampleRoot=path.join(root,app+'-'+run);fs.mkdirSync(sampleRoot);console.error('Chat benchmark',app,'sample',run);const result=await measure(app,run,data,sampleRoot);report.samples.push(result);fs.writeFileSync(output,JSON.stringify(report,null,2));console.log(JSON.stringify(result));}
  }finally{await data?.t3.close();await data?.zuse.close();}
 }
}catch(error){report.failure=error.message;console.error(error);process.exitCode=1;}finally{fs.writeFileSync(output,JSON.stringify(report,null,2)+'\n');}
