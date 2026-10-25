// General keeps core preferences visible and groups additional working controls.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {pathToFileURL} from 'node:url';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';
import {seedProjectArchives} from './fixtures/project-archive-data.mjs';
export const draft='Keep this draft while changing General preferences.';
export async function seed(host){await host.call('SignOut');return seedProjectArchives(host);}
export async function scenario(ui,host){
 await ui.shot('collapsed');
 await ui.click(600,656);await ui.shot('expanded');
 await ui.secondary(ui.expected);assert.equal(await ui.value(),ui.expected,'Secondary preference saves');
 await ui.click(600,656);await ui.shot('collapsed-saved');
 await ui.key('Space');await ui.shot('keyboard-expanded');
 await ui.secondary(ui.other);assert.equal(await ui.value(),ui.other,'Space opens secondary controls once');
 await ui.click(600,656);await ui.key('Enter');await ui.shot('enter-expanded');
 await ui.secondary(ui.expected);assert.equal(await ui.value(),ui.expected,'Enter opens secondary controls once');
 await ui.click(80,92);await ui.click(80,62);await ui.shot('returned-general');
 await ui.secondary(ui.other);assert.equal(await ui.value(),ui.other,'Controls remain reachable after navigation');
 await ui.click(600,656);await ui.key('Enter');await ui.secondary(ui.expected);
 await ui.compact();await ui.light();await ui.returnDraft();
 assert.equal(await ui.value(),ui.expected,'Exact secondary preference survives returning to the draft');
 const chats=await host.call('WatchChats');assert.equal(chats.length,5);
 const doc=await host.call('WatchDocMessages',{chatId:'live-draft'});assert.equal(doc.reset?.length??doc.length??0,0);
 return {compiledUi:true,realMetadataEngine:true,corePreferencesVisible:true,secondaryPreferenceSaved:true,keyboardDisclosure:true,repeatedActivationOnce:true,retainedAcrossSettingsNavigation:true,compact:true,light:true,exactLiveDraftPreserved:true,noNewChats:true,liveMessagesUnchanged:true};
}
async function main(){let host,page,proxy,url;try{
 host=await engine('general-cleanup-browser',{env:{PATH:'/usr/bin:/bin',ZUSE_NO_LOGIN_SHELL:'1'}});await seed(host);
 const ready=path.join(host.root,'fixture.ready');proxy=spawn('bun',[fixture('general-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(proxy.exitCode,null);await sleep(100);}assert.ok(fs.existsSync(ready));url=fs.readFileSync(ready,'utf8');
 await fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify({cloud:false,providers:{},state:{user:{id:'avery',name:'Avery Parker',email:'avery@example.test'},login:null,error:null}})});
 page=browser('general-cleanup-browser');page.open(url,fixture('cloud-archive-settings-observer.js'));page.wait('window.__uiParity.methods.includes("WatchChats")');await sleep(750);
 const click=async(x,y)=>{page.click(x,y);await sleep(650);},key=async k=>{page.run(['press',k]);await sleep(350);},shot=name=>page.screenshot('general-cleanup-browser-'+name+'.png');
 const prefs=()=>page.evaluate('JSON.parse(localStorage.getItem("zuse.workspace.preferences.v1"))');
 await click(95,184);await click(600,912);page.run(['keyboard','type',draft]);await sleep(400);
 async function copied(){await key('Control+a');await key('Control+c');assert.equal(page.evaluate('window.__archiveDraftCopied'),draft);await key('ArrowRight');}
 await copied();await click(900,400);shot('draft-before');await click(210,978);
 const result=await scenario({click,key,shot,expected:'on',other:'system',value:async()=>prefs().reduceMotion,
  secondary:async value=>{await click(value==='on'?1035:985,780);},
  compact:async()=>{page.run(['set','viewport','760','900']);await sleep(700);shot('compact');page.run(['set','viewport','1280','1000']);await sleep(700);},
  light:async()=>{await click(1036,378);assert.equal(prefs().appearance,'light');shot('light');await click(1087,378);},
  returnDraft:async()=>{await click(110,17);await click(600,912);await copied();await click(900,400);shot('draft-returned');}
 },host);
 assert.ok(!page.evaluate('window.__uiParity.methods').some(m=>['QueueMessage','QueueCommand','CloudCreate'].includes(m)));
 result.errors=page.run(['errors']);assert.deepEqual(result.errors.errors,[]);artifact('general-cleanup-browser-acceptance.json',result);console.log('General cleanup browser acceptance passed.');
}catch(error){console.error(error);try{page?.screenshot('general-cleanup-browser-failure.png');artifact('general-cleanup-browser-failure.json',{error:error.message});}catch{}process.exitCode=1;}finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}}
if(process.argv[1]&&import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href)await main();
