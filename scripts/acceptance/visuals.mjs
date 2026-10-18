import fs from 'node:fs';import path from 'node:path';import{spawn,execFileSync}from'node:child_process';import{randomUUID}from'node:crypto';import assert from'node:assert/strict';
const workspace=process.cwd(),root=path.join(workspace,'.context',`visual-session-runtime-${randomUUID()}`);fs.mkdirSync(root,{mode:0o700});
const session=execFileSync('agent-browser',['session','id','--scope','worktree','--prefix','visual-session'],{encoding:'utf8'}).trim();fs.writeFileSync('.context/visual-session-browser-session',session+'\n');
let child,ws,stage='start';const sleep=ms=>new Promise(r=>setTimeout(r,ms));
function browser(args,input){const r=JSON.parse(execFileSync('agent-browser',['--session',session,'--json',...args],{input,encoding:'utf8',maxBuffer:2*1024*1024}));if(!r.success)throw new Error('Browser action failed');return r.data;}
const evaluate=s=>browser(['eval','--stdin'],s).result;
const click=(x,y)=>{browser(['mouse','move',String(x),String(y)]);browser(['mouse','down']);browser(['mouse','up']);};
function rpc(socket){let next=1;const pending=new Map();socket.onmessage=e=>{const f=JSON.parse(e.data),p=pending.get(f.id);if(!p)return;if(f.err){clearTimeout(p.timer);pending.delete(f.id);p.reject(new Error(String(f.err)));}else if(Object.hasOwn(f,'ok')||Object.hasOwn(f,'item')){clearTimeout(p.timer);pending.delete(f.id);if(f.item)socket.send(JSON.stringify({id:f.id,cancel:true}));p.resolve(f.item??f.ok);}};return(method,params={})=>new Promise((resolve,reject)=>{const id=next++;const timer=setTimeout(()=>reject(new Error('RPC timeout')),30000);pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params}));});}
try{
 const log=path.join(root,'server.private.log'),fd=fs.openSync(log,'a',0o600);child=spawn(path.join(workspace,'target/debug/zuse-server'),['--listen=127.0.0.1:0','--ipc-port=0',`--data-dir=${path.join(root,'profile')}`,`--assets=${path.join(workspace,'dist/web')}`],{stdio:['ignore',fd,fd],env:{...process.env,RUST_LOG:'error'}});fs.closeSync(fd);
 let link,port;for(let n=0;n<200;n++){assert.equal(child.exitCode,null);const output=fs.readFileSync(log,'utf8');link=output.match(/Open (http:\/\/[^\s]+)/)?.[1];port=output.match(/Agent tools listen on 127\.0\.0\.1:(\d+)/)?.[1];if(link&&port)break;await sleep(100);}assert.ok(link&&port);
 ws=await new Promise((resolve,reject)=>{const socket=new WebSocket(`ws://127.0.0.1:${port}`);socket.onopen=()=>resolve(socket);socket.onerror=()=>reject(new Error('RPC connect'));});const call=rpc(ws);const info=await call('EngineInfo');assert.ok(info.capabilities.includes('visual-sessions-v1'));
 const chat='visual-session-chat';
 await call('SetCustomAcpAgent',{expectedRevision:0,agent:{id:'acp-visual-fixture',name:'Visual fixture',command:'node',args:[path.join(workspace,'scripts/acceptance/fixtures/visuals-acp.mjs'),String(port),chat,path.join(workspace,'scripts/acceptance/fixtures/visuals.luau')],env:{},modelIds:[]}});
 await call('Mutate',{op:'createChat',chatId:chat,deviceId:info.deviceId,config:{harness:'acp-visual-fixture',model:'fixture/model',sandbox:'workspace-write'}});await call('Mutate',{op:'renameChat',chatId:chat,title:'Interactive activity'});
 stage='publish';await call('QueueMessage',{chatId:chat,text:'Show the activity visual',attachments:[]});
 let visualId;for(let n=0;n<200;n++){const transcript=await call('WatchDocMessages',{chatId:chat});const text=JSON.stringify(transcript);const match=text.match(/"id":"(v:[^"]+)"/);if(match){visualId=match[1];break;}await sleep(100);}assert.ok(visualId,'Agent did not publish visual');
 stage='browser';browser(['--headed','--webgpu','--init-script',path.join(workspace,'scripts/acceptance/fixtures/visuals-browser.js'),'open',link]);browser(['set','viewport','1280','1000']);
 for(let n=0;n<80;n++){if(evaluate('!!document.querySelector("canvas")'))break;await sleep(250);}
 stage='open';browser(['wait','--fn','window.__visualProof.sockets.some(s=>[...s.methods.values()].includes("WatchChats"))']);await sleep(200);
 click(112,168);browser(['wait','--fn','window.__visualProof.sockets.some(s=>[...s.methods.values()].includes("WatchDocMessages"))']);await sleep(200);
 click(1146,171);browser(['wait','--fn','window.__visualProof.frames.length>0&&window.__visualProof.draws.length>0']);
 stage='controls';
 for(const [x,y,revision]of [[505,210,1],[570,206,2],[482,292,3],[985,355,4],[800,615,5]]){click(x,y);browser(['wait','--fn',`window.__visualProof.frames.at(-1)?.revision>=${revision}`]);}
 browser(['wait','--fn','window.__visualProof.draws.some(d=>d.revision===5&&d.commands.some(c=>c.Text?.text==="Canvas interactions: 11"))']);
 const state=evaluate('({frames:window.__visualProof.frames.map(f=>({revision:f.revision,count:f.tree.kind.Stack.children[0].kind.Stack.children[0].kind.Stat.value,enabled:f.tree.kind.Stack.children[0].kind.Stack.children[2].kind.Toggle.value,tab:f.tree.kind.Stack.children[1].kind.Tabs.value,scale:f.tree.kind.Stack.children[2].kind.Slider.value})),draws:window.__visualProof.draws.length,errors:window.__visualProof.errors,controls:window.__visualProof.calls.filter(c=>c.method==="ControlVisual")})');
 assert.deepEqual(state.frames.at(-1),{revision:5,count:'11',enabled:true,tab:'Monthly',scale:4});assert.equal(state.controls.length,5);assert.equal(state.errors.length,0);assert.ok(state.controls.every(c=>c.target===info.deviceId));
 browser(['screenshot',path.join(workspace,'.context/visual-session-final.png')]);
 stage='close';click(1172,171);const before=evaluate('window.__visualProof.calls.length');await sleep(500);assert.equal(evaluate('window.__visualProof.calls.length'),before,'Closing a visual must stop drawing');
 const result={compiledGpuiBrowser:true,actualHostLuauRuntime:true,actualAcpPublication:true,scriptedAcpProvider:true,visualResponsesMocked:false,button:true,toggle:true,tabs:true,slider:true,chart:true,canvasDrawAndClick:true,scopedHost:true,exactControlWrites:5,closedDrawingStopped:true,liveThirdPartyAgent:false,frames:state.frames};
 fs.writeFileSync('.context/visual-session-browser-acceptance.json',JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result));

}catch(error){fs.writeFileSync('.context/visual-session-runtime-error.private.log',String(error.stack),{mode:0o600});console.log(JSON.stringify({stage,error:error.message}));process.exitCode=1;}
finally{try{browser(['close']);}catch{}ws?.close();if(child?.exitCode===null){child.kill('SIGTERM');for(let n=0;n<100&&child.exitCode===null;n++)await sleep(100);}}
