// Identical cache-cleared/cached navigation tests for production T3 and Zuse.
// Node 22+, CHROME_BIN, T3_BENCH_AUTH_FILE (own disposable server cookies).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {engine, sleep} from './harness.mjs';

assert(process.env.CHROME_BIN && process.env.T3_BENCH_AUTH_FILE);
const root = path.resolve('.context/startup-comparison-' + randomUUID());
fs.mkdirSync(root, {recursive:true});
const t3 = JSON.parse(fs.readFileSync(process.env.T3_BENCH_AUTH_FILE));
const runs = Number(process.env.BENCH_RUNS ?? 3);
const fast = process.env.BENCH_FAST === '1';
const warmOnly = process.env.BENCH_MODE === 'warm-only';
const apps=(process.env.BENCH_APPS ?? 't3,zuse').split(',');
assert(Number.isInteger(runs) && runs > 0 && apps.every(app=>['t3','zuse'].includes(app)));
const throttle = {offline:false, latency:fast ? 10 : 400, downloadThroughput:fast ? 10000000 : 50000,
  uploadThroughput:fast ? 10000000 : 25000, connectionType:'cellular3g'};
const report = {createdAt:new Date().toISOString(), profile:throttle,
  method:'Authenticated empty workspace; same browser, viewport and network profile; fresh browser profile per sample. Auth cookies and completed onboarding pre-established. HTTP bytes captured through ready plus one second. Fully warmed cache is settled before cached navigation.',
  t3Version:'0.1.7', warmOnly, samples:[]};
const output = path.resolve(process.env.BENCH_OUTPUT ?? '.context/t3-benchmark/comparison.json');
let host;

async function until(check, label, timeout=30000) {
  const end=Date.now()+timeout;
  while(Date.now()<end) {if(await check()) return; await sleep(100);}
  throw Error(label + ' timed out');
}

const observer = `(() => {
  const p=window.__startupBench={documentId:Math.random(),rx:0,decodedWsBytes:0,shell:false,requests:{}};
  const Native=WebSocket;
  window.WebSocket=class extends Native {
    send(value){try{const f=JSON.parse(value);if(f.method)p.requests[f.id]=f.method;}catch{}super.send(value);}
    constructor(...args){super(...args);this.addEventListener('message', e=>{
      p.rx++;p.decodedWsBytes+=typeof e.data==='string'?new TextEncoder().encode(e.data).length:(e.data.byteLength??e.data.size??0);
      if(typeof e.data==='string')for(const line of e.data.split('\\n'))try{
        const f=JSON.parse(line);if(p.requests[f.id]==='WatchChats'&&Array.isArray(f.item))p.shell=true;
      }catch{}
    });}
  };
})();`;

async function sample(app, run) {
  const profile=path.join(root, app+'-'+run);
  const log=fs.openSync(profile+'.chromium.log','w');
  const chrome=spawn(process.env.CHROME_BIN,['--no-sandbox','--enable-unsafe-webgpu','--remote-debugging-port=0',
    '--user-data-dir='+profile,'about:blank'],{stdio:['ignore',log,log]});
  fs.closeSync(log);
  let socket, session, serial=0;
  const pending=new Map(), events=[];
  const inflight=new Set();let lastHttpActivity=0;
  const command=(method,params={},target=session)=>new Promise((resolve,reject)=>{
    const id=++serial;
    const timer=setTimeout(()=>{pending.delete(id);reject(Error(method+' timeout'));},20000);
    pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params,...(target?{sessionId:target}:{})}));
  });
  const evaluate=async expression=>{
    const r=await command('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});
    if(r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails));return r.result.value;
  };
  try {
    await until(()=>fs.existsSync(path.join(profile,'DevToolsActivePort')),'Chromium');
    const [port,endpoint]=fs.readFileSync(path.join(profile,'DevToolsActivePort'),'utf8').trim().split('\n');
    socket=new WebSocket('ws://127.0.0.1:'+port+endpoint);
    await new Promise((resolve,reject)=>{socket.onopen=resolve;socket.onerror=reject;});
    socket.onmessage=e=>{
      const m=JSON.parse(e.data);if(m.id){const p=pending.get(m.id);if(!p)return;pending.delete(m.id);clearTimeout(p.timer);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);}
      else if(m.sessionId===session) {events.push(m); if(m.method==='Network.requestWillBeSent' && m.params.request.url.startsWith('http')) {inflight.add(m.params.requestId);lastHttpActivity=Date.now();} if(['Network.loadingFinished','Network.loadingFailed'].includes(m.method)) {inflight.delete(m.params.requestId);lastHttpActivity=Date.now();}}
    };
    const {targetId}=await command('Target.createTarget',{url:'about:blank'});
    ({sessionId:session}=await command('Target.attachToTarget',{targetId,flatten:true}));
    await command('Page.enable');await command('Network.enable');await command('Page.bringToFront');
    const dpr=await evaluate('devicePixelRatio');
    await command('Emulation.setDeviceMetricsOverride',{width:1280,height:1000,deviceScaleFactor:dpr,mobile:false});
    await command('Emulation.setFocusEmulationEnabled',{enabled:true});
    const origin=app==='t3'?t3.origin:new URL(host.url).origin;
    let cookies;
    if(app==='t3') cookies=t3.cookies.map(cookie=>{
      const first=cookie.split(';')[0],at=first.indexOf('=');
      return {name:first.slice(0,at),value:first.slice(at+1),url:origin,httpOnly:true,sameSite:'Lax'};
    });
    else cookies=[{name:'zuse_session',value:new URLSearchParams(new URL(host.url).hash.slice(1)).get('token'),url:origin,httpOnly:true,sameSite:'Lax'}];
    await command('Network.setCookies',{cookies});
    await command('Page.addScriptToEvaluateOnNewDocument',{source:(app==='t3' ? `localStorage.setItem('t3code:client-settings:v1', JSON.stringify({onboardingCompletedAt:'2026-11-08T00:00:00.000Z'}));` : '') + observer});
    await command('Network.emulateNetworkConditions',throttle);
    await command('Network.clearBrowserCache');
    const ready=app==='zuse'
      ? 'window.__startupBench?.shell && !!document.querySelector("canvas") && !document.querySelector("#boot")'
      : process.env.T3_BENCH_READY ?? 'window.__startupBench?.rx > 0 && location.pathname === "/" && !document.querySelector("[role=dialog]") && document.body.innerText.includes("What should we work on?")';
    const results={app,run};
    if(warmOnly) {
      await command('Network.emulateNetworkConditions',{offline:false,latency:0,downloadThroughput:-1,uploadThroughput:-1});
      await command('Page.navigate',{url:origin});
      await until(async()=>await evaluate(ready),'Unthrottled cache preparation');
      await until(()=>inflight.size===0 && Date.now()-lastHttpActivity>1500,'HTTP cache preparation settles');
      await command('Network.emulateNetworkConditions',throttle);
    }
    for(const phase of (warmOnly?['warm']:['cold','warm'])) {
      events.length=0;
      const previous=phase==='warm'?await evaluate('window.__startupBench.documentId'):null;
      const started=Date.now();
      if(phase==='cold') await command('Page.navigate',{url:origin});else await command('Page.reload');
      await until(async()=>await evaluate(`window.__startupBench?.documentId !== ${JSON.stringify(previous)} && (${ready})`),app+' '+phase+' ready',fast?30000:300000);
      await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
      const readyMs=Date.now()-started;
      await sleep(1000);
      const responses=events.filter(e=>e.method==='Network.responseReceived');
      const requests=new Map(responses.map(e=>[e.params.requestId,e.params.response]));
      const finishes=events.filter(e=>e.method==='Network.loadingFinished');
      const sameOrigin=([,r])=>new URL(r.url).origin===origin;
      const assets=[...requests].filter(sameOrigin).map(([id,r])=>({
        path:new URL(r.url).pathname,status:r.status,mime:r.mimeType,
        encoding:Object.entries(r.headers).find(([k])=>k.toLowerCase()==='content-encoding')?.[1]??'identity',
        cached:!!(r.fromDiskCache||r.fromPrefetchCache),
        wireBytes:finishes.find(e=>e.params.requestId===id)?.params.encodedDataLength??null,
      }));
      results[phase]={readyMs,wireBytes:finishes.reduce((sum,e)=>sum+e.params.encodedDataLength,0),applicationWireBytes:assets.reduce((sum,a)=>sum+(a.wireBytes??0),0),requestCount:assets.length,
        decodedWsBytes:await evaluate('window.__startupBench.decodedWsBytes'),assets,
        readiness:ready};
      const screenshot=await command('Page.captureScreenshot',{format:'png'});
      fs.writeFileSync(path.join(root,app+'-'+run+'-'+phase+'.png'),Buffer.from(screenshot.data,'base64'));
      console.log(JSON.stringify({app,run,phase,readyMs,wireBytes:results[phase].wireBytes}));
      if(phase==='cold') await until(()=>inflight.size===0 && Date.now()-lastHttpActivity>1500,'HTTP cache settles before reload');
    }
    return results;
  } catch(error) {
    try {
      const screenshot=await command('Page.captureScreenshot',{format:'png'});
      fs.writeFileSync(path.join(root,app+'-'+run+'-failure.png'),Buffer.from(screenshot.data,'base64'));
      const diagnostic=await evaluate('({body:document.body.innerText.slice(0,3500),proof:window.__startupBench})');
      fs.writeFileSync(path.join(root,app+'-'+run+'-diagnostic.json'),JSON.stringify(diagnostic,null,2));
    } catch {}
    throw error;
  } finally {
    socket?.close();for(const request of pending.values())clearTimeout(request.timer);
    chrome.kill('SIGTERM');
    await new Promise(resolve=>{if(chrome.exitCode!==null||chrome.signalCode!==null)resolve();else chrome.once('exit',resolve);});
  }
}

try {
  if(apps.includes('zuse')) host=await engine('startup-comparison',{env:{PATH:'/usr/bin:/bin',ZUSE_NO_LOGIN_SHELL:'1'}});
  // Alternate which app runs first to reduce ordering bias.
  for(let run=1;run<=runs;run++) for(const app of (run%2?['t3','zuse']:['zuse','t3']).filter(app=>apps.includes(app))) {
    console.error('Benchmark',app,'sample',run);
    report.samples.push(await sample(app,run));
    fs.writeFileSync(output,JSON.stringify(report,null,2)+'\n');
  }
} catch(error) {
  report.failure=error.message;console.error(error.message);process.exitCode=1;
} finally {
  await host?.close();report.artifacts=root;
  fs.writeFileSync(output,JSON.stringify(report,null,2)+'\n');
}
