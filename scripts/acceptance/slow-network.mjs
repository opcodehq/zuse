// Node 22+. Uses a disposable real engine, the compiled UI and a local provider.
// CHROME_BIN must name Chromium/Chrome with WebGPU support.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {engine, artifact, sleep} from './harness.mjs';
import {importChatData} from './fixtures/import-chat-data.mjs';
import {socketDelayProxy} from './fixtures/socket-delay-proxy.mjs';

assert(globalThis.WebSocket, 'Use Node.js 22 or newer');
assert(process.env.CHROME_BIN, 'Set CHROME_BIN to a Chromium executable');
const root = path.resolve('.context/slow-network-' + randomUUID());
fs.mkdirSync(path.join(root, 'profile'), {recursive: true});
fs.writeFileSync(path.join(root, 'profile/harness-prefs.json'), JSON.stringify({enabled: ['claude-code']}));
const data = importChatData(root);
const historyRows=Number(process.env.ZUSE_NETWORK_HISTORY_ROWS ?? 0);
const runtimeOnly=process.env.ZUSE_RUNTIME_ONLY === '1';
if(historyRows) {
  const rows=fs.readFileSync(data.source,'utf8').split('\n').map(JSON.parse);
  const extra=Array.from({length:historyRows},(_,i)=>{const text='History row '+i+' '+('Historical context 🦀 '.repeat(50));return {type:i%2?'assistant':'user',uuid:'history-'+i,timestamp:new Date().toISOString(),sessionId:'original-claude-session',cwd:data.project,message:{role:i%2?'assistant':'user',content:i%2?[{type:'text',text}]:text}};});
  rows.splice(rows.length-1,0,...extra);fs.writeFileSync(data.source,rows.map(JSON.stringify).join('\n'));
}
const provider = path.resolve('scripts/acceptance/fixtures/slow-network-provider.py');
fs.chmodSync(provider, 0o755);
const profile = {latencyMs: Number(process.env.ZUSE_NETWORK_LATENCY_MS ?? 400), downloadBytesPerSecond: Number(process.env.ZUSE_NETWORK_DOWNLOAD_BPS ?? 50000), uploadBytesPerSecond: Number(process.env.ZUSE_NETWORK_UPLOAD_BPS ?? 25000)};
const socketRoundTripMs=Number(process.env.ZUSE_NETWORK_SOCKET_RTT_MS ?? 0);
const results = {profile:{...profile,socketRoundTripMs}, appShellUnthrottled:runtimeOnly, historyRows, checks: {}};
const output=process.env.ZUSE_NETWORK_OUTPUT ?? (runtimeOnly?'chat-runtime-3g.json':'slow-network-acceptance.json');
let host, proxy, chrome, cdp, session, stage = 'startup';
const pending = new Map();
let nextId = 0;
const events = [];
function call(method, params = {}, sessionId = session) {
  return new Promise((resolve, reject) => {
    const id = ++nextId;
    const timer = setTimeout(() => {pending.delete(id); reject(new Error(method + ' timed out'));}, 20000);
    pending.set(id, {resolve, reject, timer});
    cdp.send(JSON.stringify({id, method, params, ...(sessionId ? {sessionId} : {})}));
  });
}
const evaluate = async expression => {
  const result = await call('Runtime.evaluate', {expression, returnByValue: true, awaitPromise: true});
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
  return result.result.value;
};
async function until(check, label, timeout = 30000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await check()) return;
    await sleep(100);
  }
  throw new Error(label + ' timed out');
}
const click = async (x, y) => {
  await call('Input.dispatchMouseEvent', {type:'mouseMoved', x, y});
  for (const type of ['mousePressed', 'mouseReleased']) {
    await call('Input.dispatchMouseEvent', {type, x, y, button: 'left', clickCount: 1});
  }
};
const proof = () => evaluate('window.__networkProof');
const codec=fs.readFileSync('apps/web/src/rpc_compression.js','utf8').replaceAll('export ','');
const observer = '(() => {'+codec+`
  localStorage.setItem('zuse.workspace.preferences.v1', JSON.stringify({appearance:'dark'}));
  const Native = WebSocket;
  const p = window.__networkProof = {documentId:Math.random(), calls:[], replies:[], opens:0, bytes:0, appends:0, completed:0, dropProbes:false, droppedProbes:0, transcripts:[], sockets:[]};
  let decoded=Promise.resolve();
  window.WebSocket = class extends Native {
    constructor(...args) {
      super(...args); const decoder=createRpcDecoder();p.sockets.push(this);
      this.addEventListener('open', () => p.opens++);
      this.addEventListener('message', event => { decoded=decoded.then(async()=>{
        const payload=typeof event.data==='string'?event.data:await decodeRpcFrame(new Uint8Array(event.data),16*1024*1024,decoder);
        p.bytes += new TextEncoder().encode(payload).length;
        for (const line of payload.split('\\n')) try {
          const f = JSON.parse(line);
          if (f.item?.append?.length) {p.appends++;p.firstAppendAt ??= performance.now();}
          if((f.item?.reset||f.item?.historyResumed)&&p.calls.some(c=>c.id===f.id&&c.method==='WatchDocMessages')) p.transcripts.push({id:f.id,rows:f.item.historyRows??f.item.reset.length,hasMore:!!f.item.historyHasMore,total:f.item.historyTotal,bytes:new TextEncoder().encode(line).length,at:performance.now()});
          for(const row of f.item?.upsert ?? []) if(row.entry?.role==='assistant' && row.entry.status==='complete') p.completed++;
          p.replies.push({id:f.id, error:f.err, ok:Object.hasOwn(f,'ok'), item:!!f.item,at:performance.now()});
        } catch {}
      });});
    }
    send(text) {const f=JSON.parse(text); if(f.method) p.calls.push({id:f.id,method:f.method,params:f.params,at:performance.now()}); if(p.dropProbes && f.id===0 && f.method==='EngineInfo') {p.droppedProbes++;return;} if(f.method==='QueueCommand' && f.params?.command?.kind==='run' && f.params.command.request.harness!=='claude-code') throw new Error('Network acceptance only permits the isolated provider'); super.send(text);}
  };
  // Socket objects are retained for explicit link fault injection, not serialized.
  Object.defineProperty(p, 'sockets', {enumerable:false, value:p.sockets});
})();`;

try {
  host = await engine('slow-network', {profileRoot: root, env: {...data.env, CLAUDE_CODE_EXECUTABLE: provider}});
  if(socketRoundTripMs) proxy=await socketDelayProxy(host.url,socketRoundTripMs);
  const rows = await host.call('ListExternalChats');
  const source = rows.find(row => row.provider === 'claude-code');
  assert(source, 'Isolated provider history discovered');
  const imported = await host.call('ImportExternalChat', {id: source.id, requestId: randomUUID()});
  const chatId = imported.chat.id;
  if(historyRows) {const full=await host.call('WatchDocMessages',{chatId});results.legacyFullHistoryBytes=Buffer.byteLength(JSON.stringify(full));results.legacyFullHistoryRows=full.reset.length;}
  await host.call('Mutate', {op:'setChatConfig', chatId, expectedConfig:{deviceId:imported.chat.deviceId,config:imported.chat.config}, config:{...imported.chat.config,harness:'claude-code',model:'claude-sonnet-4-6'}});
  const chromeProfile = path.join(root, 'chromium');
  const descriptor = fs.openSync(path.join(root, 'chromium.log'), 'w');
  chrome = spawn(process.env.CHROME_BIN, [...(process.env.ZUSE_NETWORK_HEADLESS === '1' ? ['--headless=new'] : []), '--no-sandbox', '--enable-unsafe-webgpu',
    '--remote-debugging-port=0',
    '--user-data-dir=' + chromeProfile, 'about:blank'], {stdio: ['ignore', descriptor, descriptor]});
  fs.closeSync(descriptor);
  await until(() => fs.existsSync(path.join(chromeProfile, 'DevToolsActivePort')), 'Chromium startup');
  const [port, endpoint] = fs.readFileSync(path.join(chromeProfile, 'DevToolsActivePort'), 'utf8').trim().split('\n');
  cdp = new WebSocket(`ws://127.0.0.1:${port}${endpoint}`);
  await new Promise((resolve, reject) => {cdp.onopen=resolve; cdp.onerror=reject;});
  cdp.onmessage = event => {
    const message = JSON.parse(event.data);
    if (message.id) {
      const request = pending.get(message.id);
      if (!request) return;
      pending.delete(message.id); clearTimeout(request.timer);
      message.error ? request.reject(new Error(JSON.stringify(message.error))) : request.resolve(message.result);
    } else if (message.sessionId === session) events.push(message);
  };
  const {targetId} = await call('Target.createTarget', {url:'about:blank'}, undefined);
  ({sessionId: session} = await call('Target.attachToTarget', {targetId, flatten:true}, undefined));
  await call('Page.enable'); await call('Network.enable');
  await call('Page.bringToFront');
  await call('Emulation.setFocusEmulationEnabled', {enabled:true});
  const deviceScaleFactor = await evaluate('window.devicePixelRatio');
  await call('Emulation.setDeviceMetricsOverride', {width:1280, height:1000, deviceScaleFactor, mobile:false});
  await call('Page.addScriptToEvaluateOnNewDocument', {source:observer});
  const throttle = {offline:false, latency:profile.latencyMs, downloadThroughput:profile.downloadBytesPerSecond, uploadThroughput:profile.uploadBytesPerSecond, connectionType:'cellular3g'};
  await call('Network.emulateNetworkConditions', runtimeOnly ? {...throttle,latency:0,downloadThroughput:-1,uploadThroughput:-1} : throttle);
  await call('Network.clearBrowserCache');
  stage = 'cold load'; console.error(stage);
  const coldStart = Date.now();
  await call('Page.navigate', {url:proxy?.url ?? host.url});
  await until(async () => (await proof())?.calls.some(c => c.method === 'WatchChats'), 'Cold browser initialization', 300000);
  results.coldLoadMs = Date.now() - coldStart;
  const coldWasm = events.find(e => e.method === 'Network.responseReceived' && new URL(e.params.response.url).pathname.endsWith('.wasm'))?.params.response;
  assert(coldWasm, 'Browser requested the production WASM');
  const encoding = Object.entries(coldWasm.headers).find(([key]) => key.toLowerCase() === 'content-encoding')?.[1];
  assert.equal(encoding, 'br', 'Cold load negotiates Brotli');
  results.coldWasmEncoding = encoding;
  results.checks.coldLoad = true;
  await sleep(1000);
  if(runtimeOnly) await call('Network.emulateNetworkConditions',throttle);
  const chatOpenStart=Date.now();
  await click(100, 168);
  await until(async () => (await proof()).calls.some(c => c.method === 'WatchDocMessages' && c.params.chatId === chatId), 'Open imported chat');
  await until(async ()=>(await proof()).transcripts.length>0,'Recent history arrives');
  const initialHistory=(await proof()).transcripts[0];
  await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
  results.chatOpenMs=Date.now()-chatOpenStart;results.initialHistory=initialHistory;
  results.initialAgentCatalogCalls=(await proof()).calls.filter(c=>c.method==='ListModels').length;
  const socketProof=await proof();
  const identityCall=socketProof.calls.find(c=>c.method==='LocalDevice');
  results.socketRpcRoundTripMs=socketProof.replies.find(r=>r.id===identityCall.id).at-identityCall.at;
  if(socketRoundTripMs) assert(results.socketRpcRoundTripMs>=socketRoundTripMs*0.9,'Socket propagation delay is active');
  if(historyRows) {assert(initialHistory.rows<=32 && initialHistory.hasMore);assert(initialHistory.bytes*20 < results.legacyFullHistoryBytes);}
  results.checks.openHistory = true;
  if(historyRows) {
    stage='older history';console.error(stage);
    const count=(await proof()).transcripts.length;
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',x:760,y:250});
    await call('Input.dispatchMouseEvent',{type:'mouseWheel',x:760,y:250,deltaX:0,deltaY:-1000000});
    await sleep(750);
    const top=await call('Page.captureScreenshot',{format:'png'});
    fs.writeFileSync('.context/chat-runtime-history.png',Buffer.from(top.data,'base64'));
    const earlierStart=Date.now();
    await click(760,115);
    await until(async ()=>(await proof()).transcripts.length>count,'Earlier messages load');
    results.expandedHistory=(await proof()).transcripts.at(-1);
    results.olderHistoryMs=Date.now()-earlierStart;
    assert(results.expandedHistory.rows>initialHistory.rows);
    results.checks.olderHistory=true;
  }

  stage = 'send and stream'; console.error(stage);
  await click(600, 900);
  await call('Input.insertText', {text:'Send a network acceptance response'});
  for (const type of ['keyDown','keyUp']) await call('Input.dispatchKeyEvent', {type,key:'Enter',code:'Enter',windowsVirtualKeyCode:13});
  await until(async () => (await proof()).calls.some(c => c.method === 'QueueCommand'), 'Composer sends run');
  await until(async () => (await proof()).appends > 1, 'Incremental transcript appends');
  const streamingProof=await proof();
  results.sendToFirstAppendMs=streamingProof.firstAppendAt-streamingProof.calls.find(c=>c.method==='QueueCommand'&&c.params.command.kind==='run').at;
  results.checks.sendAndStream = true;
  await until(async () => (await proof()).completed > 0, 'Completed streamed response');
  stage = 'stop'; console.error(stage);
  const appendsBeforeStop = (await proof()).appends;
  await click(600, 900);
  await call('Input.insertText', {text:'stop this response'});
  for (const type of ['keyDown','keyUp']) await call('Input.dispatchKeyEvent', {type,key:'Enter',code:'Enter',windowsVirtualKeyCode:13});
  await until(async () => (await proof()).appends > appendsBeforeStop, 'Second response streams before Stop');
  if(process.env.ZUSE_RUNTIME_ACTIVE_RECONNECT === '1') {
    stage='running agent reconnect';console.error(stage);
    const active=await proof();
    const started=Date.now();
    const runCount=active.calls.filter(c=>c.method==='QueueCommand'&&c.params.command.kind==='run').length;
    await evaluate('window.__networkProof.sockets.forEach(socket=>socket.close())');
    await until(async ()=>(await proof()).opens>active.opens,'Running agent reconnects');
    await until(async ()=>(await proof()).transcripts.length>active.transcripts.length,'Running agent restores history');
    const appendsAfterReset=(await proof()).appends;
    await until(async ()=>(await proof()).appends>appendsAfterReset,'Running agent resumes live text');
    assert.equal((await proof()).calls.filter(c=>c.method==='QueueCommand'&&c.params.command.kind==='run').length,runCount,'Running agent is not started twice');
    results.runningAgentReconnectMs=Date.now()-started;
    results.checks.runningAgentReconnect=true;
    stage='stop';console.error(stage);
  }
  const stopStart = Date.now();
  await click(1178, 960);
  await until(async () => (await proof()).calls.some(c => c.method === 'QueueCommand' && c.params.command.kind === 'interrupt'), 'Stop button sends interrupt');
  await until(() => fs.existsSync(data.seen) && fs.readFileSync(data.seen,'utf8').includes('"event": "interrupt"'), 'Execution host interrupts provider');
  results.stopAcknowledgedMs = Date.now() - stopStart;
  await sleep(1000);
  const stoppedAppends = (await proof()).appends;
  await sleep(500);
  assert.equal((await proof()).appends, stoppedAppends, 'Stopped response no longer appends text');
  results.checks.stop = true;
  results.rpcBytesThroughStop = (await proof()).bytes;
  stage = 'upload'; console.error(stage);
  results.upload = await evaluate(`(async () => {
    const bytes = new Uint8Array(32768); for(let i=0;i<bytes.length;i++) bytes[i]=i%251;
    const start=performance.now(); const r=await fetch('/api/attachments?name=network-test.bin', {method:'POST',body:bytes});
    return {status:r.status, elapsedMs:performance.now()-start, body:await r.json()};
  })()`);
  assert.equal(results.upload.status, 200);
  assert.deepEqual(fs.readFileSync(results.upload.body.path), Buffer.from(Uint8Array.from({length:32768}, (_,i)=>i%251)), 'Attachment bytes survive upload');
  results.checks.upload = true;
  if (profile.uploadBytesPerSecond === 25000) assert(results.upload.elapsedMs > 500, 'Upload throttling is active');
  stage = 'reconnect'; console.error(stage);
  const before = await proof();
  const runsBefore = before.calls.filter(c => c.method === 'QueueCommand').length;
  const reconnectStart=Date.now();
  if(process.env.ZUSE_RUNTIME_BLACKHOLE === '1') await evaluate('window.__networkProof.dropProbes=true');
  else await evaluate('window.__networkProof.sockets.forEach(socket => socket.close())');
  await until(async () => (await proof()).opens > before.opens, 'Reconnect opens socket',process.env.ZUSE_RUNTIME_BLACKHOLE === '1'?100000:30000);
  await until(async () => (await proof()).calls.filter(c => c.method === 'WatchDocMessages').length > before.calls.filter(c => c.method === 'WatchDocMessages').length, 'Reconnect restores selected history');
  assert.equal((await proof()).calls.filter(c => c.method === 'QueueCommand').length, runsBefore, 'Reconnect never replays run');
  await until(async () => (await proof()).transcripts.length > before.transcripts.length,'Reconnect history arrives');
  results.reconnectHistory=(await proof()).transcripts.at(-1);
  results.reconnectMs=Date.now()-reconnectStart;
  if(historyRows) assert(results.reconnectHistory.bytes*20 < results.legacyFullHistoryBytes);
  if(process.env.ZUSE_RUNTIME_BLACKHOLE === '1') {assert((await proof()).droppedProbes>0);results.checks.silentConnectionRecovery=true;}
  results.checks.reconnectWithoutReplay = true;
  stage = 'cached reload'; console.error(stage);
  events.length = 0;
  const documentId = (await proof()).documentId;
  const reloadStart = Date.now();
  await call('Page.reload');
  await until(async () => (await proof())?.documentId !== documentId && (await proof())?.calls.some(c => c.method === 'WatchChats'), 'Cached reload', 30000);
  results.cachedReloadMs = Date.now() - reloadStart;
  const wasm = events.filter(e => e.method === 'Network.responseReceived' && new URL(e.params.response.url).pathname.endsWith('.wasm'));
  assert(wasm.length && wasm.every(e => e.params.response.fromDiskCache || e.params.response.fromPrefetchCache), 'Reload reuses WASM from cache');
  results.checks.cachedReload = true;
  results.cachedWasmNetworkBytes = events.filter(e => e.method === 'Network.loadingFinished' && wasm.some(w => w.params.requestId === e.params.requestId)).reduce((sum,e)=>sum+e.params.encodedDataLength,0);
  stage = 'startup retry'; console.error(stage);
  await call('Network.setBlockedURLs', {urls:['*.wasm']});
  await call('Page.reload');
  await until(async () => await evaluate('document.querySelector("#boot button")?.textContent === "Retry"'), 'Failed startup offers Retry');
  await call('Network.setBlockedURLs', {urls:[]});
  const failedDocument = (await proof()).documentId;
  await evaluate('document.querySelector("#boot button").click()');
  await until(async () => (await proof())?.documentId !== failedDocument && (await proof())?.calls.some(c => c.method === 'WatchChats'), 'Startup retry recovers', 30000);
  results.checks.startupRetryRecovery = true;
  const screenshot = await call('Page.captureScreenshot', {format:'png'});
  fs.writeFileSync('.context/slow-network-final.png', Buffer.from(screenshot.data,'base64'));
  const files = fs.readdirSync('dist/web').filter(name => name.endsWith('.wasm'));
  results.wasm = files.map(name => ({name, rawBytes:fs.statSync(path.join('dist/web',name)).size, gzipBytes:fs.statSync(path.join('dist/web',name+'.gz')).size, brotliBytes:fs.statSync(path.join('dist/web',name+'.br')).size}));
  artifact(output, results);
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  results.failure = {stage, message:error.message};
  if (cdp && session) try {
    const screenshot = await call('Page.captureScreenshot', {format:'png'});
    fs.writeFileSync('.context/slow-network-failure.png', Buffer.from(screenshot.data,'base64'));
    results.proof = await proof();
  } catch {}
  artifact(output, results);
  console.error(stage + ': ' + error.message); process.exitCode=1;
} finally {
  cdp?.close();
  for (const request of pending.values()) clearTimeout(request.timer);
  chrome?.kill('SIGTERM');
  await proxy?.close();
  await host?.close();
}
