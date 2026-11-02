import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
const first='7a5ef180-af4a-44a3-b86b-e1ce925eb827',second='39b98f91-2e8a-4da2-b94c-467ef55114ac';
let host,page,stage='startup';
function attachBoundary(){if(!page.evaluate('!!window.__accountConnectionProof'))page.evaluate(fs.readFileSync(fixture('account-connection.js'),'utf8'));page.run(['set','viewport','1280','1000']);}
async function settings(){page.click(210,978);await sleep(150);page.click(100,182);await sleep(200);}
try{
 host=await engine('nested-account');page=browser('nested-account');page.open(host.url,fixture('account-connection.js'));
 page.wait('window.__accountConnectionProof?.calls.some(c=>c.method==="WatchChats")');
 const root=page.run(['tab']).tabs[0].tabId;const origin=page.evaluate('location.origin');
 await settings();page.wait('window.__accountConnectionProof.calls.some(c=>c.method==="ListAccountEnvironments")');page.click(1070,520);await sleep(400);
 let tabs=page.run(['tab']).tabs;assert.equal(tabs.length,2);page.run(['tab',tabs.at(-1).tabId]);attachBoundary();
 page.wait('window.__accountConnectionProof?.calls.some(c=>c.account&&c.method==="WatchChats")');
 stage='nested connect';await settings();page.wait('window.__accountConnectionProof.calls.some(c=>c.account&&c.method==="ListAccountEnvironments")');
 page.click(1070,650);await sleep(400);tabs=page.run(['tab']).tabs;
 if(tabs.length!==3){page.screenshot('nested-connection-settings-debug.png');throw new Error('Nested Connect did not open a tab');}
 page.run(['tab',tabs.at(-1).tabId]);attachBoundary();
 page.wait(`window.__accountConnectionProof?.calls.some(c=>c.environment==="${second}"&&c.method==="WatchChats")`);
 assert.equal(page.evaluate('window.opener===null && location.hash===""'),true);
 const target=JSON.parse(page.evaluate('sessionStorage.getItem("zuse.account-target.v1")'));
 assert.equal(target.environmentId,second);assert.equal(target.expectedSubject,'second-fixture-user');assert.equal(target.controllers.length,1);
 assert.equal(target.controllers[0].environmentId,first);assert.equal(target.controllers[0].expectedSubject,'account-fixture-user');
 assert.ok(!JSON.stringify(target).includes('connectToken')&&!JSON.stringify(target).includes('grant-'));
 assert.equal(page.evaluate(`window.__accountConnectionProof.minted.some(m=>m.source==="${first}"&&m.environment==="${second}"&&m.subject==="second-fixture-user")`),true);
 assert.equal(page.evaluate(`window.__accountConnectionProof.minted.some(m=>m.source===null&&m.environment==="${second}")`),false);
 stage='nested renewal and upload';page.wait('window.__accountConnectionProof.renewals>=2');
 page.run(['press','Escape']);await sleep(150);
 assert.equal(page.evaluate(`(()=>{const data=new DataTransfer();data.items.add(new File([new Uint8Array([110,101,115,116,101,100])],'nested.txt',{type:'text/plain'}));return document.querySelector('input:not([type=file])').dispatchEvent(new ClipboardEvent('paste',{clipboardData:data,bubbles:true,cancelable:true}));})()`),false);
 page.wait('!!window.__accountConnectionProof.uploadPath');const uploaded=page.evaluate('window.__accountConnectionProof.uploadPath');assert.equal(fs.readFileSync(uploaded,'utf8'),'nested');
 assert.equal(page.evaluate(`window.__accountConnectionProof.calls.filter(c=>['UploadChunk','UploadCommit'].includes(c.method)).every(c=>c.environment==="${second}")`),true);
 stage='ancestor retirement';page.evaluate('(()=>{for(const socket of window.__accountConnectionProof.sockets.filter(s=>!s.account&&s.watch===1))socket.reply({id:1,item:{user:{id:"changed-account"}}});})()');
 page.wait('window.__accountConnectionProof.sockets.filter(s=>s.account).every(s=>s.readyState===WebSocket.CLOSED)');
 const grants=page.evaluate('window.__accountConnectionProof.grants');await sleep(1500);assert.equal(page.evaluate('window.__accountConnectionProof.grants'),grants);
 assert.equal(page.evaluate('window.__accountConnectionProof.calls.filter(c=>["QueueCommand","QueueMessage"].includes(c.method)).length'),0);
 stage='return one controller';await settings();page.click(1070,465);await sleep(400);attachBoundary();
 page.wait(`window.__accountConnectionProof?.calls.some(c=>c.environment==="${first}"&&c.method==="WatchChats")`);
 const parent=JSON.parse(page.evaluate('sessionStorage.getItem("zuse.account-target.v1")'));assert.equal(parent.environmentId,first);assert.ok(!parent.controllers?.length);
 stage='return during authorization';page.run(['tab',root]);
 const direct={environmentId:first,expectedSubject:'account-fixture-user',label:'Pending computer'};
 page.run(['open',`${origin}/?fixtureHoldGrant=1#computer=${encodeURIComponent(JSON.stringify(direct))}`]);page.wait('window.__accountConnectionProof?.held===1');
 await settings();page.click(1070,465);page.wait('window.__accountConnectionProof?.calls.some(c=>!c.account&&c.method==="WatchChats")');
 assert.equal(page.evaluate('sessionStorage.getItem("zuse.account-target.v1")'),null);
 const result={compiledGpui:true,accountBoundaryFixture:true,realEngineUploads:true,nestedTabOpened:true,subjectsBoundPerController:true,publicRecipeOnly:true,bothGrantsRenewed:true,uploadBytes:6,ancestorRetirementStopsDescendants:true,returnRestoresParent:true,returnDuringPendingGrant:true,agentWrites:0,liveAccountAndDeployedTunnel:false};
 artifact('nested-connection-runtime-acceptance.json',result);console.log(JSON.stringify(result));
}catch(error){
 try{artifact('nested-connection-failure.json',page.evaluate('(()=>{const p=window.__accountConnectionProof;return {hasProof:!!p,metadata:sessionStorage.getItem("zuse.account-target.v1"),calls:p?.calls,minted:p?.minted,grants:p?.grants,renewals:p?.renewals,held:p?.held,sockets:p?.sockets.map(s=>({environment:s.environment,state:s.readyState,watch:s.watch}))};})()'));}catch{}
 console.error(`Nested connection acceptance failed at ${stage}: ${error.message}`);process.exitCode=1;
}finally{page?.close();await host?.close();}
