import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {sleep} from '../harness.mjs';

export async function until(check,label,timeout=30000){const end=Date.now()+timeout;while(Date.now()<end){if(await check())return;await sleep(75);}throw Error(label+' timed out');}

export async function cdpBrowser(root,{headless=false}={}){
  const executable=process.env.CHROME_BIN;assert(executable,'Set CHROME_BIN');
  const profile=path.join(root,'chrome');const fd=fs.openSync(path.join(root,'chrome.log'),'w');
  const child=spawn(executable,[...(headless?['--headless=new']:[]),'--no-sandbox','--enable-unsafe-webgpu','--remote-debugging-port=0','--user-data-dir='+profile,'about:blank'],{stdio:['ignore',fd,fd]});fs.closeSync(fd);
  let socket,session,serial=0;const pending=new Map(),events=[];
  const call=(method,params={},target=session)=>new Promise((resolve,reject)=>{const id=++serial;const timer=setTimeout(()=>{pending.delete(id);reject(Error(method+' timed out'));},30000);pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params,...(target?{sessionId:target}:{})}));});
  const close=async()=>{socket?.close();for(const p of pending.values())clearTimeout(p.timer);child.kill('SIGTERM');await new Promise(resolve=>{if(child.exitCode!==null||child.signalCode!==null)resolve();else child.once('exit',resolve);});};
  try{
    await until(()=>fs.existsSync(path.join(profile,'DevToolsActivePort')),'Chromium startup');
    const [port,endpoint]=fs.readFileSync(path.join(profile,'DevToolsActivePort'),'utf8').trim().split('\n');
    socket=new WebSocket('ws://127.0.0.1:'+port+endpoint);await new Promise((resolve,reject)=>{socket.onopen=resolve;socket.onerror=reject;});
    socket.onmessage=event=>{const m=JSON.parse(event.data);if(m.id){const p=pending.get(m.id);if(!p)return;pending.delete(m.id);clearTimeout(p.timer);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);}else if(m.sessionId===session)events.push(m);};
    const {targetId}=await call('Target.createTarget',{url:'about:blank'});({sessionId:session}=await call('Target.attachToTarget',{targetId,flatten:true}));
    await call('Page.enable');await call('Runtime.enable');await call('Network.enable');await call('Page.bringToFront');await call('Emulation.setFocusEmulationEnabled',{enabled:true});
    const evaluate=async expression=>{const r=await call('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});if(r.exceptionDetails)throw Error(JSON.stringify(r.exceptionDetails));return r.result.value;};
    const dpr=await evaluate('devicePixelRatio');await call('Emulation.setDeviceMetricsOverride',{width:1280,height:1000,deviceScaleFactor:dpr,mobile:false});
    const click=async(x,y)=>{await call('Input.dispatchMouseEvent',{type:'mouseMoved',x,y});for(const type of ['mousePressed','mouseReleased'])await call('Input.dispatchMouseEvent',{type,x,y,button:'left',clickCount:1});};
    const screenshot=async name=>{const image=await call('Page.captureScreenshot',{format:'png'});fs.writeFileSync(path.join(root,name+'.png'),Buffer.from(image.data,'base64'));};
    const type=async text=>{await call('Input.insertText',{text});for(const type of ['keyDown','keyUp'])await call('Input.dispatchKeyEvent',{type,key:'Enter',code:'Enter',windowsVirtualKeyCode:13});};
    return {call,evaluate,click,type,screenshot,close,events};
  }catch(error){await close();throw error;}
}
