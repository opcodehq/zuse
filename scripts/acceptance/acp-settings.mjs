// Catalogue metadata is a fixture. Every configuration, enablement and model probe uses a real engine.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,proxy,url;
const state=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
const configure=value=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(value)});
const click=async(x,y)=>{page.click(x,y);await sleep(350);};
const key=k=>page.run(['press',k]);
const type=v=>page.run(['keyboard','type',v]);
async function until(check,label){for(let n=0;n<120;n++){if(await check())return;await sleep(100);}throw Error(label);}
const snapshot=()=>host.call('GetCustomAcpAgents');
const count=async(method)=>(await state()).events.filter(e=>e.method===method).length;
async function saved(revision){await until(async()=>(await snapshot()).revision>revision,'Engine configuration saved');await sleep(700);}
async function menu(row=0){await click(1098,851+row*48);}
async function choose(index){key('Home');for(let n=0;n<index;n++)key('ArrowDown');key('Enter');await sleep(400);}
try{
 host=await engine('acp-manager');
 const ready=path.join(host.root,'proxy.ready'),log=fs.openSync(path.join(host.root,'proxy.log'),'a');
 proxy=spawn('bun',[fixture('acp-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:['ignore',log,log]});fs.closeSync(log);
 await until(()=>fs.existsSync(ready),'Proxy ready');url=fs.readFileSync(ready,'utf8');
 page=browser('acp-manager');page.open(url,fixture('acp-settings-observer.js'));
 page.wait('window.__acpSettings?.methods.includes("WatchChats")');
 await click(550,891);type('Initial ACP draft.');await click(210,978);await click(80,92);
 page.wait('window.__acpSettings.methods.includes("GetCustomAcpAgents")');
 page.screenshot('acp-manager-browser-empty.png');await click(1086,804);
 page.wait('window.__acpSettings.methods.includes("GetAcpCatalogIcon")');await sleep(400);
 page.screenshot('acp-manager-browser-catalog.png');
 type('No such agent');await sleep(200);page.screenshot('acp-manager-browser-no-matches.png');key('Escape');
 await click(1086,804);page.screenshot('acp-manager-browser-retained-search.png');key('Control+a');key('Backspace');await sleep(300);
 await configure({holdWrites:true});await click(811,270);
 await until(async()=>(await state()).held===1,'Real Add acknowledgement held');
 const first=await snapshot();assert.equal(first.agents.length,1);const id=first.agents[0].id;
 assert.equal(first.agents[0].name,'Catalogue Agent');assert.equal(first.agents[0].args[2],'literal ; $(not-a-shell)');
 assert.equal(first.agents[0].env.PARITY_TOKEN,'fake private value');
 await click(811,270);key('Enter');assert.equal(await count('SetCustomAcpAgent'),1,'Pending Add guarded');
 page.screenshot('acp-manager-browser-pending.png');key('Escape');await click(125,17);
 await click(550,891);key('Control+a');type('Later ACP draft');await configure({holdWrites:false});await sleep(1200);
 type(' stays editable.');key('Control+a');key('Control+c');
 const draft='Later ACP draft stays editable.';
 page.wait(`window.__acpSettings.copied===${JSON.stringify(draft)}`);
 page.screenshot('acp-manager-browser-background-draft.png');
 await click(210,978);await click(80,92);await sleep(600);
 await until(async()=>(await state()).events.some(e=>e.method==='ListModels'&&e.harness===id),'Automatic real model probe');
 page.screenshot('acp-manager-browser-ready.png');
 await menu();page.screenshot('acp-manager-browser-menu.png');await choose(2);await saved(first.revision);
 const duplicated=await snapshot();assert.equal(duplicated.agents.length,2);
 const copy=duplicated.agents.find(a=>a.id!==id);assert.equal(copy.name,'Catalogue Agent copy');assert.deepEqual(copy.env,first.agents[0].env);assert.deepEqual(copy.args,first.agents[0].args);
 page.screenshot('acp-manager-browser-duplicated.png');
 await click(1055,851);await until(async()=>(await host.call('ListHarnesses')).find(a=>a.id===id)?.enabled===false,'Real disable');
 page.screenshot('acp-manager-browser-disabled.png');await click(1055,851);
 await until(async()=>(await host.call('ListHarnesses')).find(a=>a.id===id)?.enabled===true,'Real enable');
 await menu();await choose(1);page.screenshot('acp-manager-browser-edit.png');
 await click(550,248);key('Control+a');type('Renamed Agent');await click(490,499);await click(550,515);type('fallback/model');
 const revision=(await snapshot()).revision;key('Enter');await saved(revision);
 const edited=(await snapshot()).agents.find(a=>a.id===id);assert.equal(edited.name,'Renamed Agent');assert.deepEqual(edited.args,first.agents[0].args);assert.deepEqual(edited.env,first.agents[0].env);assert.deepEqual(edited.modelIds,['fallback/model']);
 await menu();await choose(1);await click(694,449);const clearRevision=(await snapshot()).revision;await click(785,499);await saved(clearRevision);
 assert.deepEqual((await snapshot()).agents.find(a=>a.id===id).env,{});page.screenshot('acp-manager-browser-cleared.png');
 for(let n=0;n<2;n++){const revision=(await snapshot()).revision;await menu();key('End');key('Enter');await saved(revision);}
 assert.equal((await snapshot()).agents.length,0);page.screenshot('acp-manager-browser-removed.png');
 await click(1086,804);page.run(['set','viewport','760','900']);await sleep(300);page.screenshot('acp-manager-browser-compact.png');key('Escape');
 page.run(['set','viewport','1280','1000']);await click(125,17);await click(550,891);key('Control+a');key('Control+c');page.wait(`window.__acpSettings.copied===${JSON.stringify(draft)}`);
 assert.equal((await host.call('WatchChats')).length,0);
 artifact('acp-manager-browser-acceptance.json',{realEngine:true,compiledGpui:true,fixtureCatalogue:true,searchAndReopen:true,registryAdd:true,actualAcknowledgementHeld:true,pendingDuplicateGuard:true,pendingEscapeCloses:true,backgroundSaveKeepsLaterDraftAndFocus:true,realModelProbe:true,enableDisable:true,keyboardActions:true,duplicatePreservesConfiguration:true,editPreservesIdentityArgumentsAndEnvironment:true,explicitCredentialClear:true,remove:true,compact:true,configs:0,chats:0});
 console.log('ACP manager browser acceptance passed.');
}catch(error){try{page?.screenshot('acp-manager-browser-failure.png');}catch{}console.error('ACP manager acceptance failed: '+error.stack);process.exitCode=1;}
finally{page?.close();proxy?.kill();await host?.close();}
