import assert from 'node:assert/strict';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
import {previewPage} from './fixtures/browser-page.mjs';
let host,page,preview,stage='start';
try {
  host=await engine('browser-frame');preview=await previewPage();
  const info=await host.call('EngineInfo');
  await host.call('Mutate',{op:'createChat',chatId:'browser-frame-chat',deviceId:info.deviceId});
  await host.call('Mutate',{op:'renameChat',chatId:'browser-frame-chat',title:'Web preview'});
  page=browser('browser-frame');page.open(host.url,fixture('browser-observer.js'));
  page.wait('window.__browserProof.methods.includes("WatchChats")');await sleep(200);
  page.click(112,168);page.wait('window.__browserProof.methods.includes("WatchDocMessages")');
  stage='pane';page.click(1254,22);await sleep(150);page.click(989,22);
  page.wait('window.__browserProof.methods.includes("WatchPreviews")');
  stage='address';page.click(858,65);page.run(['keyboard','inserttext',preview.url]);page.run(['press','Enter']);
  page.wait('document.querySelector("#zuse-browser-frame")?.style.display==="block"');
  const bounds=page.evaluate('(()=>{const f=document.querySelector("#zuse-browser-frame");const b=f.getBoundingClientRect();return {left:b.left,top:b.top,width:b.width,height:b.height,isolated:f.credentialless};})()');
  assert.ok(bounds.isolated&&bounds.left>600&&bounds.top>=80&&bounds.width>=300&&bounds.height>400);
  stage='page';
  const revision=page.evaluate('window.__browserProof.inspect()');
  page.wait(`window.__browserProof.previewRevision>${revision}&&window.__browserProof.preview?.ready==="true"`);
  const loaded=page.evaluate('window.__browserProof.preview');
  assert.equal(loaded.parentAccess,'blocked');assert.equal(loaded.credentialless,true);
  assert.equal(loaded.fetch,'Fetched from preview');
  const origin=page.evaluate('(()=>{const b=document.querySelector("#zuse-browser-frame").getBoundingClientRect();return {left:b.left,top:b.top};})()');
  page.click(origin.left+loaded.draftPoint[0],origin.top+loaded.draftPoint[1]);
  page.run(['keyboard','inserttext','Keep this preview draft']);
  page.wait('window.__browserProof.preview?.draft==="Keep this preview draft"');
  const button=page.evaluate('(()=>{const b=document.querySelector("#zuse-browser-frame").getBoundingClientRect();const p=window.__browserProof.preview.buttonPoint;return [b.left+p[0],b.top+p[1]];})()');
  page.click(...button);
  page.wait('window.__browserProof.preview?.count===1');
  assert.equal(page.evaluate('window.__browserProof.preview.socket'),'WebSocket connected');
  page.screenshot('browser-frame-final.png');
  stage='hide';page.click(263,964);page.wait('document.querySelector("#zuse-browser-frame")?.style.display==="none"');
  page.run(['press','Escape']);page.wait('document.querySelector("#zuse-browser-frame")?.style.display==="block"');
  const inspected=page.evaluate('window.__browserProof.inspect()');
  page.wait(`window.__browserProof.previewRevision>${inspected}`);
  assert.equal(page.evaluate('window.__browserProof.preview.draft'),'Keep this preview draft');
  stage='retire';page.click(269,22);page.wait('!document.querySelector("#zuse-browser-frame")');
  assert.equal(page.evaluate('window.__browserProof.methods.filter(m=>m==="QueueMessage"||m==="QueueCommand").length'),0);
  await sleep(50);
  assert.ok(preview.requests.some(r=>r.path==='/main.js')&&preview.requests.some(r=>r.path==='/data')&&preview.requests.some(r=>r.path==='/socket'));
  assert.ok(preview.requests.every(r=>!r.applicationCookie));
  const result={compiledGpuiBrowser:true,actualIsolatedFrame:true,addressEnter:true,moduleAndFetch:true,websocket:true,pageInput:true,parentAccessBlocked:true,applicationCookieExcluded:true,settingsHidePreservesDraft:true,threadNavigationRetiresFrame:true,agentWrites:0,previewDiscoverySubscribed:true,pairedPreviewTunnel:false};
  artifact('browser-frame-acceptance.json',result);console.log(JSON.stringify(result));
}catch(error){
  await sleep(50);
  let state;try{state=page?.evaluate('({preview:window.__browserProof?.preview,title:document.title,ready:document.body?.dataset.ready,parentAccess:document.body?.dataset.parentAccess,credentialless:window.credentialless,fetch:document.querySelector("#fetch")?.textContent,socket:document.querySelector("#socket")?.textContent})');}catch{}
  let consoleMessages;try{consoleMessages=JSON.stringify(page?.run(['console'])).replace(/https?:\/\/[^\s"']+/g,'[url]');page?.screenshot('browser-frame-failure.png');}catch{}
  artifact('browser-frame-failure.json',{stage,error:error.message,state,requests:preview?.requests,consoleMessages});process.exitCode=1;
}
finally{page?.close();await preview?.close();await host?.close();}
