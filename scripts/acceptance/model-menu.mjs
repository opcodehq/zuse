// Compiled canvas menus over an isolated catalogue; no provider turn or Cloud compute.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn, execFileSync} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,proxy,page,url,stage='startup';
const state=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
const update=v=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(v)});
const click=async(x,y)=>{page.run(['batch','--bail',`mouse move ${x} ${y}`,'mouse down','mouse up','wait 250']);await sleep(250);};
const press=k=>page.run(['press',k]);
const preferences=()=>page.evaluate('JSON.parse(localStorage.getItem("zuse.workspace.preferences.v1"))');
const shot=n=>page.screenshot('model-menu-'+n+'.png');
const red=(file,x,y)=>Number(execFileSync('convert',['.context/'+file,'-format',`%[fx:int(255*p{${x},${y}}.r)]`,'info:'],{encoding:'utf8'}));
try {
 host=await engine('model-menu');const ready=path.join(host.root,'fixture.ready');
 proxy=spawn('bun',[fixture('general-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(proxy.exitCode,null);await sleep(100);}
 url=fs.readFileSync(ready,'utf8');await update({providers:{}});
 page=browser('model-menu');page.open(url,fixture('general-settings-observer.js'));
 page.wait('window.__generalParity.methods.includes("ListModels")');await click(70,55);
 stage='draft and grouped menu';await click(600,891);page.run(['keyboard','type','Keep this model-menu draft']);await click(1010,946);shot('grouped');
 assert.ok(red('model-menu-grouped.png',900,740)>20&&red('model-menu-grouped.png',1270,740)>20,'380px menu fits the window');
 stage='provider filter preserves draft model';await click(913,711);shot('scoped');assert.equal(preferences().modelConfigHistory?.length??0,0);
 stage='default star is independent of current selection';await click(1222,867);shot('star');assert.equal(preferences().defaultProvider,'codex');assert.equal(preferences().defaultModels.codex,'balanced');assert.equal(preferences().modelConfigHistory.length,0);
 stage='row selection closes';await click(1020,900);shot('selected');assert.equal(preferences().modelConfigHistory[0].provider,'codex');assert.equal(preferences().modelConfigHistory[0].model,'deep');assert.ok(red('model-menu-selected.png',1200,750)<20,'Selection closes the popup');
 stage='search shortcuts and separate default';await click(1010,946);page.run(['keyboard','type','  DEEP  ']);await sleep(300);shot('filtered');press('Control+1');await sleep(350);assert.equal(preferences().modelConfigHistory[0].provider,'claude-code');assert.equal(preferences().modelConfigHistory[0].model,'deep');assert.equal(preferences().defaultModels.codex,'balanced');
 stage='no match Enter and Escape';await click(1010,946);page.run(['keyboard','type','not-a-model']);await sleep(300);press('Enter');shot('empty');assert.equal(preferences().modelConfigHistory[0].provider,'claude-code');press('Escape');await sleep(250);shot('dismissed');
 stage='outside dismissal restores composer input';await click(1010,946);await click(600,891);page.run(['keyboard','type',' stays here']);shot('draft');
 stage='settings picker target';await click(210,978);await click(70,120);await click(1000,228);page.run(['keyboard','type','Codex Deep']);await sleep(250);shot('settings-filtered');press('Enter');await sleep(300);assert.equal(preferences().defaultProvider,'codex');assert.equal(preferences().defaultModels.codex,'deep');shot('settings-selected');
 stage='saved default after reload';page.run(['reload']);page.wait('window.__generalParity.methods.includes("ListModels")');await click(70,55);await click(1010,946);shot('reloaded');assert.equal(preferences().defaultModels.codex,'deep');press('Escape');
 const proof=await state();assert.ok(!proof.events.some(e=>e.blocked));assert.ok(!page.evaluate('window.__generalParity.methods').some(m=>['QueueMessage','QueueCommand','CloudCreate','CloudEnvironment'].includes(m)));assert.equal(page.run(['errors']).errors.length,0);
 artifact('model-menu-acceptance.json',{compiledGpui:true,realDisposableEngine:true,isolatedCatalogue:true,groupedMenu:true,providerFilterDoesNotSelect:true,defaultIndependentFromSelection:true,rowCloses:true,trimmedSubstringSearch:true,keyboardShortcut:true,noMatchDoesNotSend:true,escape:true,outsideDismissal:true,draftRetainedVisually:true,settingsDefaultTarget:true,reloadDefault:true,providerExecution:0,liveProviderAcceptance:false,events:proof.events});
 console.log('Model menu passed: scope, separate default, row/keyboard choice, dismissal, draft, settings and reload.');
} catch(error) {try{shot('failure');artifact('model-menu-failure.json',{stage,error:error.message,preferences:preferences(),fixture:await state()});}catch{}console.error('Model menu failed at '+stage+': '+error.stack);process.exitCode=1;}
finally{page?.close();proxy?.kill();await host?.close();}
