// Plugin catalog workflows against isolated account/service replies and a real engine.
import fs from 'node:fs';
import path from 'node:path';
import {spawn,execFileSync} from 'node:child_process';
import {pathToFileURL} from 'node:url';
import assert from 'node:assert/strict';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';
import {canvasWheel} from './global-archive.mjs';
export const draft='Keep this draft through plugin settings.';
export async function scenario(ui,url){
 const proof=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
 const configure=p=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(p)});
 async function until(check,label){for(let n=0;n<90;n++){const p=await proof();if(check(p))return p;await sleep(100);}throw Error(label);}
 const writes=p=>p.events.filter(e=>!['list','signIn'].includes(e.action)).length;
 const metrics=p=>p.snapshot.connections.filter(c=>c.pluginId==='metrics');
 const back=()=>ui.click(...ui.points.back);
 async function search(query){
  await ui.click(...ui.points.search);await ui.key('Control+a');
  if(ui.replace)await ui.replace(query);else{await ui.key('Backspace');if(query)await ui.type(query);}
  await sleep(500);
 }
 console.log("Plugin catalog: accounts and keyboard reopening");
 const start=await proof();
 await ui.shot('catalog');
 await ui.click(...ui.points.manage);await ui.shot('manage-existing');assert.equal(writes(await proof()),writes(start),'Manage opens details without changing an account');await back();
 for(let n=0;n<2;n++){
  await ui.click(...(n===0?ui.points.metrics:ui.points.another));
  await ui.type('discard-this-key');await ui.key('Escape');
  assert.equal(metrics(await proof()).length,n,'Cancel discards the key');
  await ui.key(n===0?'Enter':'Space');
  await ui.type('fixture-plugin-key');await ui.shot('reopened-key-'+n);await ui.key('Enter');
  await until(p=>metrics(p).length===n+1,'Keyboard reopening accepts a key without a field click');
 }
 await ui.shot('two-accounts');await back();await ui.shot('unique-connected-count');
 console.log("Plugin catalog: paging, domain and URL search");
 const initial=(await proof()).snapshot.catalog;
 const samples=Array.from({length:45},(_,n)=>({id:'sample-'+n,name:'Sample tool '+String(n+1).padStart(2,'0'),description:'Catalog paging fixture',domain:'sample-'+(n+1)+'.example.test',custom:false,apiKeyHeader:'X-API-Key'}));
 const custom={id:'custom-query',name:'Private reports',description:'',domain:'',url:'https://tools.example.test/route-unique',custom:true,apiKeyHeader:'X-API-Key'};
 await configure({catalog:[...initial,...samples,custom]});await ui.click(...ui.points.refresh);
 await until(p=>p.snapshot.catalog.length===49,'Extended catalog supplied');await sleep(800);
 await ui.wheel(5000);await ui.shot('first-page-more');await ui.click(...ui.points.more);await sleep(600);await ui.wheel(5000);await ui.shot('expanded-page');
 await ui.click(...ui.points.last);await ui.type('fixture-plugin-key');await ui.key('Enter');
 await until(p=>p.events.some(e=>e.action==='connect'&&e.pluginId==='sample-44'),'Show more exposes the last catalog entry');await back();
 for(const [query,id]of [['SAMPLE-44.EXAMPLE.TEST','sample-43'],['ROUTE-UNIQUE','custom-query']]){
  await search(query);await ui.shot(id+'-search');await ui.click(...ui.points.single);
  await ui.type('fixture-plugin-key');await ui.key('Enter');
  await until(p=>p.events.some(e=>e.action==='connect'&&e.pluginId===id),'Search matches domain or server URL');await back();
 }
 await search('');await ui.key('Tab');await ui.wheel(5000);await ui.shot('query-reset-page');ui.checkMoreReset();
 console.log("Plugin catalog: signed-out and loading states");
 // Signed-out and load failure states must not offer the bundled fallback catalog.
 await ui.wheel(-5000);await sleep(500);await configure({account:null,signInMode:'hold'});await sleep(800);await ui.shot('signed-out');
 const beforeSign=await proof();await ui.click(...ui.points.metrics);assert.equal(writes(await proof()),writes(beforeSign),'No signed-out catalog action remains clickable');
 await ui.click(...ui.points.signIn);await ui.click(...ui.points.signIn);await until(p=>p.events.some(e=>e.action==='signIn'),'Sign-in action');
 assert.equal((await proof()).events.filter(e=>e.action==='signIn').length,1,'A pending sign-in dispatches once');
 const lists=(await proof()).events.filter(e=>e.action==='list').length;
 await configure({account:'catalog-next-owner',listMode:'hold',signInMode:'ok'});
 await until(p=>p.events.filter(e=>e.action==='list').length===lists+1,'New account starts one catalog read');await sleep(500);await ui.shot('loading');
 await ui.click(...ui.points.refresh);await ui.click(...ui.points.refresh);
 assert.equal((await proof()).events.filter(e=>e.action==='list').length,lists+1,'Pending refreshes do not duplicate the read');
 await configure({listMode:'fail'});await sleep(700);await ui.shot('load-error');await configure({listMode:'ok'});await ui.click(...ui.points.refresh);
 await until(p=>p.events.filter(e=>e.action==='list').length===lists+2,'Catalog failure can be retried');await sleep(700);await ui.shot('recovered');
 await ui.returnDraft();const end=await proof();assert.equal(end.events.filter(e=>e.blocked).length,0);
 return {compiledUi:true,realEngine:true,pluginProtocolFixture:true,keyboardReopenWithoutFieldClick:true,cancelDiscardsSecret:true,twoAccountsOnePlugin:true,customGroups:true,pagination:true,queryResetsPagination:true,domainAndUrlSearch:true,signedOutGating:true,onePendingSignIn:true,loadFailureRetry:true,onePendingRefresh:true,liveDraftPreserved:true,providerWrites:0,fixture:end};
}
async function main(){
 let host,page,proxy,url;const shot=name=>page.screenshot('plugin-cleanup-browser-'+name+'.png');
 try{
  host=await engine('plugin-cleanup-browser',{env:{PATH:'/usr/bin:/bin',ZUSE_NO_LOGIN_SHELL:'1'}});const info=await host.call('EngineInfo');
  await host.call('Mutate',{op:'createChat',chatId:'plugin-draft',deviceId:info.deviceId,cwd:host.root});await host.call('Mutate',{op:'renameChat',chatId:'plugin-draft',title:'Keep plugin draft'});
  const ready=path.join(host.root,'fixture.ready');proxy=spawn('bun',[fixture('plugin-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
  for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
  page=browser('plugin-cleanup-browser');page.open(url,fixture('plugin-catalog-observer.js'));page.wait('window.__uiParity.methods.includes("Plugins")');await sleep(700);
  const click=async(x,y)=>{page.click(x,y);await sleep(700);},key=async k=>{page.run(['press',k]);await sleep(350);},type=async s=>{page.run(['keyboard','type',s]);await sleep(400);};
  await click(95,210);page.wait('window.__uiParity.methods.includes("WatchDocMessages")');await sleep(800);await click(600,912);await type(draft);
  async function copied(){await key('Control+a');await key('Control+c');assert.equal(page.evaluate('window.__pluginCopied'),draft,'Exact real composer draft');await key('ArrowRight');}
  await copied();await click(900,400);shot('draft-before');await click(80,108);await sleep(700);
  const pixels=name=>execFileSync('convert',['.context/plugin-cleanup-browser-'+name+'.png','-crop','100x34+404+934','-depth','8','rgb:-']);
  const result=await scenario({click,key,type,shot,
   points:{back:[431,84],manage:[726,282],metrics:[1086,475],another:[520,357],search:[510,150],refresh:[770,150],more:[444,950],last:[726,938],single:[726,282],signIn:[434,302]},
   wheel:dy=>canvasWheel(page,url,dy),checkMoreReset:()=>assert.deepEqual(pixels('query-reset-page'),pixels('first-page-more'),'Clearing search resets the page limit'),
   returnDraft:async()=>{await click(95,210);await sleep(700);await click(600,912);await copied();await click(900,400);shot('draft-returned');},
  },url);
  assert.equal((await host.call('WatchChats')).length,1,'Plugins never create a conversation');result.errors=page.run(['errors']);assert.deepEqual(result.errors.errors,[]);artifact('plugin-cleanup-browser-acceptance.json',result);console.log('Plugin catalog browser acceptance passed.');
 }catch(error){try{artifact('plugin-cleanup-browser-failure-proof.json',await fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json()));shot('failure');}catch{}console.error('Plugin catalog browser failed: '+error.message);process.exitCode=1;}
 finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}
}
if(process.argv[1]&&import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href)await main();
