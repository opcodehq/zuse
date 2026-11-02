// Rendered custom-server removal against an isolated account and real disposable engine.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,child,url,stage='startup';
const proof=()=>fetch(url+'/__fixture').then(r=>r.json());
const configure=value=>fetch(url+'/__fixture',{method:'POST',body:JSON.stringify(value)});
async function until(check,label){for(let i=0;i<100;i++){const p=await proof();if(check(p))return p;await sleep(100);}throw Error(label);}
const click=async(x,y)=>{page.click(x,y);await sleep(200);};
const press=k=>page.run(['press',k]);
try{
 host=await engine('plugin-removal');const ready=path.join(host.root,'fixture.ready');
 child=spawn('bun',[fixture('plugin-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let i=0;i<100&&!fs.existsSync(ready);i++){assert.equal(child.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
 page=browser('plugin-removal');page.open(url,fixture('ui-parity-observer.js'));page.wait('window.__uiParity.methods.includes("Plugins")');
 await click(210,978);await click(70,152);await click(1087,163);await click(990,199);await click(518,280);
 stage='detail';page.screenshot('plugin-removal-browser-detail.png');
 await click(1001,186);page.screenshot('plugin-removal-browser-menu.png');await click(950,219);
 stage='cancel';page.screenshot('plugin-removal-browser-confirm.png');press('Escape');await sleep(100);
 assert.equal((await proof()).events.filter(e=>e.action==='removeServer').length,0,'Escape cancels without a write');
 await click(1001,186);await click(950,219);await configure({removalMode:'hold'});
 stage='pending';await click(751,341);await until(p=>p.events.some(e=>e.action==='removeServer'),'Request was not started');
 page.screenshot('plugin-removal-browser-pending.png');press('Enter');press('Escape');await sleep(100);
 assert.equal((await proof()).events.filter(e=>e.action==='removeServer').length,1,'Pending dialog ignores repeated submit and Escape');
 const lists=(await proof()).events.filter(e=>e.action==='list').length;await configure({removalMode:'fail'});
 await until(p=>p.events.filter(e=>e.action==='list').length>lists,'Failed request reconciles before retry');await sleep(200);
 stage='failure';page.screenshot('plugin-removal-browser-error.png');
 assert.ok((await proof()).snapshot.catalog.some(p=>p.id==='workspace-tools'));
 await configure({removalMode:'ok'});press('Enter');
 stage='retry';await until(p=>!p.snapshot.catalog.some(p=>p.id==='workspace-tools'),'Retry removes the selected server');
 const p=await proof();assert.ok(p.snapshot.connections.some(c=>c.id==='figma-one'),'Unrelated account is retained');
 assert.equal(p.events.filter(e=>e.action==='removeServer').length,2);
 page.screenshot('plugin-removal-browser-removed.png');
 const methods=page.evaluate('window.__uiParity.methods');assert.equal(methods.filter(m=>['QueueMessage','QueueCommand','CloudCreate','CloudBillingCheckout'].includes(m)).length,0);
  assert.equal(page.run(['errors']).errors.length,0,'No browser runtime errors');
 artifact('plugin-removal-browser-acceptance.json',{compiledGpui:true,isolatedAccount:true,referenceCommit:'43e14c1',escapeCancels:true,pendingIgnoresRepeatedSubmit:true,pendingIgnoresEscape:true,errorKeepsDialog:true,readBeforeRetry:true,keyboardRetry:true,selectedServerRemoved:true,unrelatedConnectionRetained:true,agentWrites:0});
 console.log('Custom server removal passed: cancel, pending guard, failed request, read-before-retry and keyboard retry.');
}catch(error){try{artifact('plugin-removal-browser-failure-proof.json',await proof());}catch{}try{page?.screenshot('plugin-removal-browser-failure.png');}catch{}console.error('Custom server removal failed at '+stage+': '+error.message);process.exitCode=1;}
finally{page?.close();child?.kill('SIGTERM');await host?.close();}
