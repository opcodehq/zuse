// Compiled UI acceptance against isolated account RPC contracts. Never changes a real CLI login.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';

export const accountRows = [
  {id:'aaaaaaaaaaaaaaaa',harness:'claude-code',displayName:'Ava Moon',email:'ava@example.test',planLabel:'Pro',active:true,switchable:true,usageWindows:[{label:'5 hours',usedFraction:0.32,resetsAt:'2026-10-25T01:00:00Z'}]},
  {id:'bbbbbbbbbbbbbbbb',harness:'claude-code',displayName:'Lee Park',email:'lee@example.test',planLabel:'Team',active:false,switchable:true,usageWindows:[],usageError:'Usage is temporarily unavailable.'},
];
export async function accountScenario(ui) {
  const {configure,state,click,key,shot,more,usage}=ui;
  const events=async method=>(await state()).events.filter(e=>e.method===method);
  async function until(check,label){for(let n=0;n<150;n++){if(await check())return;await sleep(100);}throw Error(label);}
  await shot('rows');
  await click(...usage);await shot('usage');await click(...usage);
  await configure({holdAccountActions:true});
  await click(...more[1]);await shot('menu');await key('Home');await key('Enter');
  await until(async()=>(await events('ActivateAgentAccount')).length===1,'Switch requested');
  await key('Enter');await key('Enter');
  assert.equal((await events('ActivateAgentAccount')).length,1,'Pending switch is not duplicated');
  assert.equal((await state()).providers.accounts[0].active,true,'Rows remain authoritative until acknowledgement');
  await configure({holdAccountActions:false});
  await until(async()=>(await state()).providers.accounts.find(a=>a.displayName==='Lee Park').active,'Switched account');
  await sleep(600);await shot('switched');
  const openRemoval=async()=>{await click(...more[0]);await key('End');await key('Enter');};
  await openRemoval();await shot('remove-confirm');await key('Escape');
  assert.equal((await events('ForgetAgentAccount')).length,0,'Escape does not remove');
  await openRemoval();await key('Enter');
  assert.equal((await events('ForgetAgentAccount')).length,0,'Cancel has initial keyboard focus');
  await configure({accountActionError:'The saved login could not be removed.'});
  await openRemoval();await key('Tab');await key('Enter');
  await until(async()=>(await events('ForgetAgentAccount')).length===1,'Removal requested');
  await sleep(700);await shot('remove-error');
  assert.equal((await state()).providers.accounts.length,2,'A refusal preserves both rows');
  await configure({accountActionError:null,holdAccountActions:true});
  await key('Enter');
  await until(async()=>(await events('ForgetAgentAccount')).length===2,'Retry requested');
  await key('Escape');await click(20,600);await key('Enter');
  assert.equal((await events('ForgetAgentAccount')).length,2,'Pending removal is contained and not duplicated');
  assert.equal((await state()).providers.accounts.length,2);
  await shot('remove-pending');await configure({holdAccountActions:false});
  await until(async()=>(await state()).providers.accounts.length===1,'Removal acknowledged');
  await sleep(600);await shot('removed');
  assert.equal((await state()).providers.accounts[0].displayName,'Lee Park');
  assert.equal((await state()).providers.accounts[0].active,true);
  await click(...more[0]);await key('End');await key('Enter');await shot('active-remove-confirm');await key('Escape');
  const proof=await state();
  assert.ok(!proof.events.some(e=>['QueueMessage','QueueCommand','CreateChat','CloudCreate','CloudEnvironment'].includes(e.method)));
  return {sharedAccountRows:true,isolatedAccountContract:true,liveCliCredentialsChanged:false,switch:true,switchPendingDuplicateGuard:true,authoritativeAcknowledgement:true,confirmation:true,cancelInitialFocus:true,escapeCancels:true,failureKeepsRowsAndAllowsRetry:true,pendingRemovalContained:true,inactiveRemovalKeepsActiveLogin:true,activeRemovalCopy:true,usageDetails:true};
}
export async function addAccountScenario({configure,state,click,key,shot,add}) {
  await configure({loginMode:'browser',pollsBeforeDone:null});
  await click(...add);
  async function until(method){for(let n=0;n<150;n++){if((await state()).events.some(e=>e.method===method))return;await sleep(100);}throw Error(method+' was not requested');}
  await until('StartAgentLogin');await shot('add-waiting');await key('Escape');await until('CancelAgentLogin');
  assert.equal((await state()).providers.accounts.length,1,'Cancelling another sign-in preserves the saved account');
  return {addAnotherAccount:true,cancelAdditionalSignIn:true};
}
async function main(){
  let host,page,proxy,url,stage='startup';
  try{
    host=await engine('provider-accounts',{env:{PATH:'/usr/bin:/bin',ZUSE_NO_LOGIN_SHELL:'1'}});
    const ready=path.join(host.root,'fixture.ready');proxy=spawn('bun',[fixture('general-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
    for(let n=0;n<150&&!fs.existsSync(ready);n++)await sleep(100);
    assert.ok(fs.existsSync(ready));url=fs.readFileSync(ready,'utf8');
    const state=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
    const configure=providers=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify({providers})});
    await fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify({providers:{accounts:accountRows},state:{user:null,login:null,error:null}})});
    const init=path.join(host.root,'observer.js');fs.writeFileSync(init,fs.readFileSync(fixture('general-settings-observer.js'),'utf8')+'\nconst copy=navigator.clipboard.writeText.bind(navigator.clipboard);navigator.clipboard.writeText=async value=>{window.__providerCopied=String(value);return copy(value);};');
    page=browser('provider-accounts');page.open(url,init);page.wait('window.__generalParity?.methods.includes("WatchChats")');
    const click=async(x,y)=>{page.click(x,y);await sleep(350);};
    const key=async value=>{page.run(['press',value]);await sleep(250);};
    const draft='Keep this account settings draft.';
    await click(550,891);page.run(['keyboard','type',draft]);await click(210,978);await click(80,92);page.wait('window.__generalParity.methods.includes("ListHarnesses")');await sleep(900);await click(600,227);
    stage='account actions';const proof=await accountScenario({configure,state,click,key,shot:async name=>page.screenshot('provider-accounts-browser-'+name+'.png'),more:[[1097,275],[1097,311]],usage:[435,343]});
    stage='add account';Object.assign(proof,await addAccountScenario({configure,state,click,key,shot:async name=>page.screenshot('provider-accounts-browser-'+name+'.png'),add:[435,351]}));
    stage='compact';page.run(['set','viewport','760','900']);page.screenshot('provider-accounts-browser-compact.png');page.run(['set','viewport','1280','1000']);
    await sleep(700);
    await click(125,17);await click(550,891);await key('End');page.run(['keyboard','type',' Preserved.']);await key('Control+a');await key('Control+c');page.wait(`window.__providerCopied===${JSON.stringify(draft+' Preserved.')}`);
    assert.equal((await host.call('WatchChats')).length,0);
    const errors=page.run(['errors']).errors;assert.deepEqual(errors,[],'No browser runtime errors');
    artifact('provider-accounts-browser-acceptance.json',{...proof,compiledGpui:true,compact:true,exactDraftPreserved:true,chats:0,errors});console.log('Provider accounts browser acceptance passed.');
  }catch(error){try{page?.screenshot('provider-accounts-browser-failure.png');artifact('provider-accounts-browser-failure.json',{stage,error:error.message,state:await fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json()),errors:page?.run(['errors']),console:page?.run(['console'])});}catch{}console.error(error.stack);process.exitCode=1;}
  finally{page?.close();proxy?.kill();await host?.close();}
}
if(process.argv[1]===new URL(import.meta.url).pathname)await main();
