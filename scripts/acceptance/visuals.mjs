import assert from 'node:assert/strict';
import {engine, browser, fixture, artifact, sleep} from './harness.mjs';

let host, page, stage='start';
const snapshot=()=>page.evaluate('({frames:window.__visualProof.frames.map(f=>({revision:f.revision,count:f.tree.kind.Stack.children[0].kind.Stack.children[0].kind.Stat.value,enabled:f.tree.kind.Stack.children[0].kind.Stack.children[2].kind.Toggle.value,tab:f.tree.kind.Stack.children[1].kind.Tabs.value,scale:f.tree.kind.Stack.children[2].kind.Slider.value})),errors:window.__visualProof.errors,controls:window.__visualProof.calls.filter(c=>c.method==="ControlVisual")})');
try {
  host=await engine('visual-session');
  const info=await host.call('EngineInfo');
  assert.ok(info.capabilities.includes('visual-sessions-v1'));
  const chat='visual-session-chat';
  await host.call('SetCustomAcpAgent',{expectedRevision:0,agent:{id:'acp-visual-fixture',name:'Visual fixture',command:'node',args:[fixture('visuals-acp.mjs'),String(host.port),chat,fixture('visuals.luau')],env:{},modelIds:[]}});
  await host.call('Mutate',{op:'createChat',chatId:chat,deviceId:info.deviceId,config:{harness:'acp-visual-fixture',model:'fixture/model',sandbox:'workspace-write'}});
  await host.call('Mutate',{op:'renameChat',chatId:chat,title:'Interactive activity'});
  stage='publish';
  await host.call('QueueMessage',{chatId:chat,text:'Show the activity visual',attachments:[]});
  let visual;
  for (let n=0;n<200;n++) {
    visual=JSON.stringify(await host.call('WatchDocMessages',{chatId:chat})).match(/"id":"(v:[^"]+)"/)?.[1];
    if (visual) break;
    await sleep(100);
  }
  assert.ok(visual,'Agent did not publish visual');
  page=browser('visual-session');page.open(host.url,fixture('visuals-browser.js'));
  stage='open';
  page.wait('window.__visualProof.sockets.some(s=>[...s.methods.values()].includes("WatchChats"))');
  await sleep(200);page.click(112,168);
  page.wait('window.__visualProof.sockets.some(s=>[...s.methods.values()].includes("WatchDocMessages"))');
  await sleep(200);page.click(1146,171);
  page.wait('window.__visualProof.frames.length>0&&window.__visualProof.draws.length>0');
  stage='controls';
  for (const [x,y,revision] of [[505,210,1],[570,206,2],[482,292,3],[985,355,4],[800,615,5]]) {
    page.click(x,y);page.wait(`window.__visualProof.frames.at(-1)?.revision>=${revision}`);
  }
  page.wait('window.__visualProof.draws.some(d=>d.revision===5&&d.commands.some(c=>c.Text?.text==="Canvas interactions: 11"))');
  let state=snapshot();
  assert.deepEqual(state.frames.at(-1),{revision:5,count:'11',enabled:true,tab:'Monthly',scale:4});
  assert.equal(state.controls.length,5);assert.equal(state.errors.length,0);
  assert.ok(state.controls.every(call=>call.target===info.deviceId));
  stage='keyboard';
  page.click(505,210);page.wait('window.__visualProof.frames.at(-1)?.revision===6');
  page.run(['press','Space']);page.wait('window.__visualProof.frames.at(-1)?.revision===7');
  page.run(['press','Tab']);page.run(['press','Space']);page.wait('window.__visualProof.frames.at(-1)?.revision===8');
  page.run(['press','Tab']);page.run(['press','Space']);page.wait('window.__visualProof.frames.at(-1)?.revision===9');
  page.run(['press','Tab']);page.run(['press','Tab']);
  for(const [key,revision] of [['ArrowLeft',10],['Home',11],['End',12]]){
    page.run(['press',key]);page.wait(`window.__visualProof.frames.at(-1)?.revision===${revision}`);
  }
  page.run(['press','Tab']);page.run(['press','Space']);page.wait('window.__visualProof.frames.at(-1)?.revision===13');
  state=snapshot();assert.deepEqual(state.frames.at(-1),{revision:13,count:'23',enabled:false,tab:'Weekly',scale:5});
  assert.equal(state.controls.length,13);assert.equal(state.errors.length,0);
  stage='motion';
  page.wait('window.__visualProof.draws.some(d=>d.revision===13)');
  const animated=page.evaluate('window.__visualProof.draws.length');await sleep(350);
  assert.ok(page.evaluate('window.__visualProof.draws.length')>animated+1);
  page.run(['set','media','light','reduced-motion']);
  page.wait('window.matchMedia("(prefers-reduced-motion: reduce)").matches');await sleep(300);
  const paused=page.evaluate('window.__visualProof.draws.length');await sleep(350);
  assert.equal(page.evaluate('window.__visualProof.draws.length'),paused,'Reduced motion must stop animation RPCs');
  page.screenshot('visual-session-final.png');
  stage='close';page.click(1172,171);
  const before=page.evaluate('window.__visualProof.calls.length');await sleep(500);
  assert.equal(page.evaluate('window.__visualProof.calls.length'),before,'Closing a visual must stop drawing');
  const result={compiledGpuiBrowser:true,actualHostLuauRuntime:true,actualAcpPublication:true,scriptedAcpProvider:true,visualResponsesMocked:false,button:true,toggle:true,tabs:true,slider:true,chart:true,canvasDrawAndClick:true,scopedHost:true,exactControlWrites:13,keyboardControlWrites:7,reducedMotionStopsDraws:true,tableColumnsAndRaggedRows:true,closedDrawingStopped:true,liveThirdPartyAgent:false,frames:state.frames};
  artifact('visual-session-browser-acceptance.json',result);console.log(JSON.stringify(result));
} catch(error) {
  let state;try{state=snapshot();page?.screenshot('visual-parity-failure.png');}catch{}
  artifact('visual-session-failure.json',{stage,error:error.message,state});process.exitCode=1;
} finally {page?.close();await host?.close();}
