import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,stage='startup';
try {
  host=await engine('account-connection');page=browser('account-connection');
  page.open(host.url,fixture('account-connection.js'));
  page.wait('window.__accountConnectionProof?.calls.some(c=>c.method==="WatchChats")');
  page.click(210,978);await sleep(150);page.click(100,182);
  page.wait('window.__accountConnectionProof?.calls.some(c=>c.method==="ListAccountEnvironments")');
  await sleep(150);stage='open destination';page.click(1070,520);await sleep(500);
  const tabs=page.run(['tab']);artifact('account-connections-tabs.json',tabs);
  assert.equal(tabs.tabs.length,2);
  page.run(['tab',tabs.tabs.at(-1).tabId]);
  // Site-created tabs do not inherit the automation tool's per-page init script.
  // Install only the response boundary; the app's retry still owns reconnection.
  if(!page.evaluate('!!window.__accountConnectionProof'))page.evaluate(fs.readFileSync(fixture('account-connection.js'),'utf8'));
  page.run(['set','viewport','1280','1000']);
  stage='verify destination';
  page.wait('window.__accountConnectionProof?.remembered===1 && window.__accountConnectionProof.calls.some(c=>c.account&&c.method==="WatchChats")');
  assert.equal(page.evaluate('window.opener===null'),true);
  assert.equal(page.evaluate('location.hash'),'');
  const saved=page.evaluate('sessionStorage.getItem("zuse.account-target.v1")');
  assert.equal(JSON.parse(saved).environmentId,'7a5ef180-af4a-44a3-b86b-e1ce925eb827');
  assert.ok(!saved.includes('connectToken')&&!saved.includes('grant-'));
  stage='renew same socket';page.wait('window.__accountConnectionProof.renewals>=1');
  assert.equal(page.evaluate('window.__accountConnectionProof.sockets.filter(s=>s.account).length'),1);
  assert.equal(page.evaluate('window.__accountConnectionProof.calls.filter(c=>c.account&&c.method==="WatchChats").length'),1);
  stage='upload';page.run(['press','Escape']);await sleep(150);
  const content=[104,101,108,108,111,10];
  assert.equal(page.evaluate(`(()=>{const input=document.querySelector('input:not([type=file])');const data=new DataTransfer();data.items.add(new File([new Uint8Array(${JSON.stringify(content)})],'account-notes.txt',{type:'text/plain'}));return input.dispatchEvent(new ClipboardEvent('paste',{clipboardData:data,bubbles:true,cancelable:true}));})()`),false);
  page.wait('!!window.__accountConnectionProof.uploadPath');
  const uploadPath=page.evaluate('window.__accountConnectionProof.uploadPath');
  assert.deepEqual([...fs.readFileSync(uploadPath)],content);
  assert.equal(page.evaluate('window.__accountConnectionProof.calls.filter(c=>!c.account&&(c.method==="UploadChunk"||c.method==="UploadCommit")).length'),0);
  assert.equal(page.evaluate('window.__accountConnectionProof.sockets.filter(s=>s.account&&s.readyState===WebSocket.OPEN).length'),1);
  stage='account retirement';
  page.evaluate('(()=>{const p=window.__accountConnectionProof;for(const socket of p.sockets.filter(s=>!s.account&&s.watch===1)){socket.reply({id:1,item:{user:{id:"another-account"}}});}})()');
  page.wait('window.__accountConnectionProof.sockets.filter(s=>s.account).every(s=>s.readyState===WebSocket.CLOSED)');
  const grants=page.evaluate('window.__accountConnectionProof.grants');await sleep(1500);
  assert.equal(page.evaluate('window.__accountConnectionProof.grants'),grants);
  assert.equal(page.evaluate('window.__accountConnectionProof.calls.filter(c=>c.method==="QueueCommand"||c.method==="QueueMessage").length'),0);
  stage='storage unavailable';page.run(['tab',tabs.tabs[0].tabId]);
  page.evaluate('sessionStorage.removeItem("zuse.account-target.v1")');
  const origin=page.evaluate('location.origin');
  const target={environmentId:'7a5ef180-af4a-44a3-b86b-e1ce925eb827',expectedSubject:'account-fixture-user',label:'Storage fixture computer'};
  page.run(['open',`${origin}/?fixtureNoAccountStorage=1#computer=${encodeURIComponent(JSON.stringify(target))}`]);
  page.wait('window.__accountConnectionProof?.remembered===1');
  assert.equal(page.evaluate('location.hash.startsWith("#computer=")'),true);
  assert.equal(page.evaluate('sessionStorage.getItem("zuse.account-target.v1")'),null);
  page.run(['reload']);page.wait('window.__accountConnectionProof?.remembered===1');
  assert.equal(page.evaluate('window.__accountConnectionProof.calls.some(c=>!c.account&&c.method==="LocalDevice")'),false);
  stage='malformed explicit destination';
  page.run(['open',`${origin}/#computer=invalid-json`]);await sleep(400);
  assert.equal(page.evaluate('location.hash'),'#computer=invalid-json');
  assert.equal(page.evaluate('window.__accountConnectionProof.calls.length'),0);
  page.run(['reload']);await sleep(400);
  assert.equal(page.evaluate('window.__accountConnectionProof.calls.length'),0);
  const result={compiledGpui:true,accountBoundaryFixture:true,realEngineUploads:true,openedOwnTab:true,openerRetired:true,publicTargetOnly:true,renewedWithoutResubscription:true,uploadBytes:content.length,accountChangeClosesSockets:true,storageFailureRetainsExplicitTarget:true,malformedTargetNeverFallsBack:true,agentWrites:0,liveAccountAndDeployedTunnel:false};
  artifact('browser-account-runtime-acceptance.json',result);console.log(JSON.stringify(result));
}catch(error){
  try{artifact('browser-account-failure.json',page.evaluate('(()=>{const p=window.__accountConnectionProof;return {hasProof:!!p,hashCleared:location.hash==="",accountMetadata:sessionStorage.getItem("zuse.account-target.v1"),grants:p?.grants,renewals:p?.renewals,remembered:p?.remembered,calls:p?.calls,sockets:p?.sockets.map(s=>({account:s.account,state:s.readyState,protocol:s.requestedProtocol}))};})()'));}catch{}
  console.error(`Account connection acceptance failed at ${stage}: ${error.message}`);process.exitCode=1;
}
finally{page?.close();await host?.close();}
