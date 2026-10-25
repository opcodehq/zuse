// Real naming preferences with isolated model catalogues and controlled receipts.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn,execFileSync} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,proxy,page,url,stage='startup';console.log('Settings models: startup');
const state=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
const update=value=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(value)});
const click=async(x,y)=>{page.run(['batch','--bail',`mouse move ${x} ${y}`,'mouse down','mouse up','wait 200']);await sleep(200);};
const press=key=>page.run(['press',key]);
const type=value=>page.run(['keyboard','type',value]);
const shot=name=>page.screenshot('settings-model-browser-'+name+'.png');
const draftPaint=name=>execFileSync('convert',['.context/settings-model-browser-'+name+'.png','-crop','650x28+324+892','-depth','8','rgb:-']);
const redText=name=>{const pixels=execFileSync('convert',['.context/settings-model-browser-'+name+'.png','-crop','650x32+374+944','-depth','8','rgb:-']);let count=0;for(let n=0;n<pixels.length;n+=3)if(pixels[n]>100&&pixels[n+1]<pixels[n]*0.75&&pixels[n+2]<pixels[n]*0.9)count++;return count;};
const prefs=()=>page.evaluate('JSON.parse(localStorage.getItem("zuse.workspace.preferences.v1"))');
async function until(check,label){for(let n=0;n<80;n++){if(await check())return;await sleep(100);}throw Error(label);}
const writes=async()=>(await state()).events.filter(e=>e.method==='SetTitleSettings');
const title=async(harness,model)=>until(async()=>{const value=await host.call('GetTitleSettings');return value.harness===harness&&value.model===model;},'Real naming preference');
async function preserved(){const chat=(await host.call('WatchChats')).find(c=>c.id==='settings-existing');assert.deepEqual(chat.config,config,'Settings must preserve the conversation model and options');assert.equal(chat.title,'Keep this conversation');}
const config={harness:'codex',model:'deep',reasoning:'high',modelOptions:{speed:'fast'},sandbox:'workspace-write'};
// The canvas needs positioned wheel input; the CLI targets the hidden input at (0,0).
async function cdp(method,params,pageTarget=false){
 let endpoint=page.run(['get','cdp-url']).cdpUrl;
 if(pageTarget){const address=new URL(endpoint);const targets=await fetch(`http://${address.host}/json/list`,{headers:{Connection:'close'}}).then(r=>r.json());const target=targets.find(target=>target.type==='page'&&target.url===url+'/');assert.ok(target,'Owned browser page');endpoint=target.webSocketDebuggerUrl;}
 const socket=await new Promise((resolve,reject)=>{const socket=new WebSocket(endpoint);socket.onopen=()=>resolve(socket);socket.onerror=()=>reject(Error('Browser input connection'));});
 let timer;try{return await new Promise((resolve,reject)=>{timer=setTimeout(()=>reject(Error('Browser input acknowledgement')),20000);socket.onmessage=event=>{const frame=JSON.parse(event.data);if(frame.id===1){if(frame.error)reject(Error(frame.error.message));else resolve(frame.result);}};socket.send(JSON.stringify({id:1,method,params}));});}finally{clearTimeout(timer);socket.close();}
}
async function canvasWheel(x,y,deltaY){await cdp('Input.dispatchMouseEvent',{type:'mouseWheel',x,y,deltaY,deltaX:0},true);await sleep(700);}
try{
 const root=path.resolve('.context/settings-model-browser-runtime-'+crypto.randomUUID());fs.mkdirSync(root+'/profile',{recursive:true});fs.writeFileSync(root+'/profile/harness-prefs.json',JSON.stringify({enabled:['claude-code','codex']}));
 host=await engine('settings-model-browser',{profileRoot:root});await host.call('SignOut');const info=await host.call('EngineInfo');
 await host.call('Mutate',{op:'createChat',chatId:'settings-existing',deviceId:info.deviceId,config});await host.call('Mutate',{op:'renameChat',chatId:'settings-existing',title:'Keep this conversation'});await host.call('SetTitleSettings',{harness:'codex',model:'deep'});
 const ready=root+'/fixture.ready';proxy=spawn('bun',[fixture('general-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
 const advanced={id:'deep',label:'Codex Deep',reasoningLevels:['low','medium','high'],options:[{id:'speed',label:'Speed',defaultChoice:'standard',choices:[{id:'standard',label:'Standard'},{id:'fast',label:'Fast'}]}]};
 await update({providers:{models:{codex:[{id:'balanced',label:'Codex Balanced',reasoningLevels:[],options:[]},advanced,{id:'default',label:'Codex Automatic',reasoningLevels:[],options:[]}]}},titles:{readError:'Naming settings are temporarily unavailable.'}});
 page=browser('settings-models');page.open(url,fixture('general-settings-observer.js'));page.wait('window.__generalParity.methods.includes("WatchChats")');await sleep(500);await click(100,166);await until(async()=>(await state()).events.some(e=>e.method==='WatchDocMessages'&&e.chatId==='settings-existing'),'Open existing conversation');await click(600,906);type('Keep my browser settings draft.');await click(700,500);shot('draft-before');const beforeDraft=draftPaint('draft-before');await click(210,978);await click(70,120);
 stage='default model selection';console.log('Settings models: default model selection');await click(990,228);shot('defaults');type('  CODEX DEEP  ');await sleep(300);press('Enter');await sleep(500);assert.equal(prefs().defaultProvider,'codex');assert.equal(prefs().defaultModels.codex,'deep');assert.equal(prefs().modelConfigHistory?.length??0,0);await preserved();shot('default-selected');
 stage='thread naming read failure and retry';console.log('Settings models: thread naming read failure and retry');await click(65,62);await click(600,650);await canvasWheel(1000,800,1500);shot('naming-entry');await update({titles:{readError:null}});await click(930,915);await sleep(500);shot('naming-loaded');await title('codex','deep');
 const before=(await writes()).length;
 stage='held naming save';console.log('Settings models: held naming save');await update({titles:{holdWrites:true}});await click(990,892);shot('naming-menu');type('Balanced');press('Control+1');await title('claude-code','balanced');await until(async()=>(await state()).titles.pending===1,'Held naming response');await click(990,892);press('Enter');await click(1050,947);assert.equal((await writes()).length,before+1,'Pending clicks must not repeat the write');await preserved();shot('naming-saving');await update({titles:{holdWrites:false}});await sleep(500);shot('naming-saved');
 stage='rejected naming save';console.log('Settings models: rejected naming save');await update({titles:{rejectWrite:true}});await click(990,892);type('Codex Deep');press('Enter');await sleep(700);await title('claude-code','balanced');assert.equal((await writes()).length,before+2);await canvasWheel(1000,900,500);shot('naming-rejected');assert.ok(redText('naming-rejected')>20,'Rejected save keeps an inline error and the saved choice');
 stage='manual naming refresh';console.log('Settings models: manual naming refresh');await click(930,915);await sleep(500);shot('naming-refreshed');assert.equal(redText('naming-refreshed'),0,'Manual refresh clears a resolved error');
 stage='lost naming receipt';console.log('Settings models: lost naming receipt');await update({titles:{loseReceipt:true}});await click(990,892);type('Codex Deep');press('Enter');await title('codex','deep');await sleep(700);assert.equal((await writes()).length,before+3);shot('naming-reconciled');assert.equal(redText('naming-reconciled'),0,'Read-back confirms the interrupted save');await preserved();
 stage='agent default alias';console.log('Settings models: agent default alias');await click(990,892);type('Automatic');press('Enter');await title('codex',null);shot('naming-automatic');
 stage='follow thread agent';console.log('Settings models: follow thread agent');await click(1050,947);await title(null,null);shot('naming-follow');await preserved();
 stage='exact draft';console.log('Settings models: exact draft');await click(120,17);await click(700,500);shot('draft-final');const afterDraft=draftPaint('draft-final');let changed=0;for(let n=0;n<beforeDraft.length;n++)if(Math.abs(beforeDraft[n]-afterDraft[n])>20)changed++;assert.ok(changed<500,'Rendered draft survives settings without a model or text change');const journal=page.evaluate('Object.keys(localStorage).filter(key=>key.startsWith("zuse.browser-drafts.v1.")).flatMap(key=>JSON.parse(localStorage.getItem(key)).drafts)');const draft=journal.find(draft=>draft.chatId==='settings-existing');assert.equal(draft?.text,'Keep my browser settings draft.');
 assert.equal(prefs().defaultProvider,'codex');assert.equal(prefs().defaultModels.codex,'deep');assert.equal(prefs().modelConfigHistory?.length??0,0);
 const proof=await state();assert.ok(!proof.events.some(event=>event.blocked));assert.equal((await writes()).length,before+5);
 artifact('settings-model-browser-acceptance.json',{compiledBrowser:true,realDisposableEngine:true,isolatedCatalogue:true,defaultSelection:true,searchTrimAndSpaces:true,conversationPreserved:true,positionedCanvasScroll:true,namingReadRetry:true,pendingDuplicateGuard:true,rejectedWriteReadBack:true,lostReceiptReadBackWithoutRepeat:true,agentDefaultNormalized:true,reset:true,exactDraft:draft.text,renderedDraftPreserved:true,events:proof.events});console.log('Settings model browser acceptance passed.');
}catch(error){try{shot('failure');artifact('settings-model-browser-failure.json',{stage,error:error.message,wire:await state()});}catch{}console.error('Settings model browser acceptance failed at '+stage+': '+error.stack);process.exitCode=1;}
finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}
