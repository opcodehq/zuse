// Compiled provider settings with isolated account/update contracts and real draft storage.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
import {accountRows} from './provider-accounts.mjs';

let host,page,proxy,url,stage='startup';
const state=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
const configure=providers=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify({providers})});
async function until(check,label){for(let n=0;n<150;n++){if(await check())return;await sleep(100);}throw Error(label);}
try{
  host=await engine('provider-options',{env:{PATH:'/usr/bin:/bin',ZUSE_NO_LOGIN_SHELL:'1'}});
  const ready=path.join(host.root,'ready');proxy=spawn('bun',[fixture('general-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
  await until(()=>fs.existsSync(ready),'Fixture ready');url=fs.readFileSync(ready,'utf8');
  await fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify({providers:{accounts:accountRows,loginMode:'browser',pollsBeforeDone:null},state:{user:null,login:null,error:null}})});
  const init=path.join(host.root,'observer.js');fs.writeFileSync(init,fs.readFileSync(fixture('general-settings-observer.js'),'utf8')+'\nconst copy=navigator.clipboard.writeText.bind(navigator.clipboard);navigator.clipboard.writeText=async value=>{window.__optionsCopied=String(value);return copy(value);};');
  page=browser('provider-options');page.open(url,init);page.wait('window.__generalParity?.methods.includes("WatchChats")');
  const click=async(x,y)=>{page.click(x,y);await sleep(500);};
  const key=async value=>{page.run(['press',value]);await sleep(400);};
  const shot=name=>page.screenshot('provider-options-browser-'+name+'.png');
  const events=async method=>(await state()).events.filter(e=>e.method===method);
  const draft='Keep this agent options draft.';
  await click(550,891);page.run(['keyboard','type',draft]);await click(210,978);await click(80,92);page.wait('window.__generalParity.methods.includes("WatchHarnessUpdates")');await sleep(900);await click(600,227);shot('collapsed');
  stage='keyboard disclosure and update policy';await click(650,436);shot('expanded');
  await click(580,478);await until(async()=>(await events('SetHarnessUpdatePolicy')).length===1,'Policy request');
  assert.equal((await events('SetHarnessUpdatePolicy'))[0].params.policy,'auto-when-idle');
  await until(async()=>(await state()).providers.updates[0].policy==='auto-when-idle','Confirmed policy');
  await click(650,436);await key('Enter');shot('reopened');await key('Enter');
  stage='update attention outside options';const updates=(await state()).providers.updates;updates[0]={...updates[0],phase:'available',latestVersion:'1.2.4'};await configure({updates});await sleep(500);shot('attention');
  stage='pending sign-in survives provider collapse';await click(435,387);await until(async()=>(await events('StartAgentLogin')).length===1,'Additional sign-in');await click(600,227);await sleep(500);assert.equal((await events('CancelAgentLogin')).length,0);await click(600,227);shot('signin-reopened');assert.equal((await events('StartAgentLogin')).length,1);await key('Escape');await until(async()=>(await events('CancelAgentLogin')).length===1,'Sign-in cancellation');
  stage='compact';page.run(['set','viewport','760','900']);await sleep(700);shot('compact');page.run(['set','viewport','1280','1000']);await sleep(700);
  await click(125,17);await click(550,891);await key('End');page.run(['keyboard','type',' Preserved.']);await key('Control+a');await key('Control+c');page.wait(`window.__optionsCopied===${JSON.stringify(draft+' Preserved.')}`);
  assert.equal((await host.call('WatchChats')).length,0);const errors=page.run(['errors']).errors;assert.deepEqual(errors,[]);
  artifact('provider-options-browser-acceptance.json',{compiledBrowser:true,isolatedUpdateAndAccountContract:true,accountsFirst:true,collapsedByDefault:true,keyboardDisclosure:true,policyAcknowledged:true,reopeningRetainsPolicy:true,attentionWhileCollapsed:true,pendingLoginSurvivesCollapse:true,compact:true,exactDraftPreserved:true,chats:0,errors});console.log('Provider options browser acceptance passed.');
}catch(error){try{page?.screenshot('provider-options-browser-failure.png');artifact('provider-options-browser-failure.json',{stage,error:error.message,state:await state(),errors:page?.run(['errors'])});}catch{}console.error(error.stack);process.exitCode=1;}
finally{page?.close();proxy?.kill();await host?.close();}
