// Actual saved-chat mutations behind both rendered global archive screens.
import fs from 'node:fs';
import path from 'node:path';
import {spawn,execFileSync} from 'node:child_process';
import {pathToFileURL} from 'node:url';
import assert from 'node:assert/strict';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';
import {seedGlobalArchives} from './fixtures/global-archive-data.mjs';
export async function canvasWheel(page,url,deltaY){
 // agent-browser's generic scroll targets the canvas's hidden text input.
 const endpoint=new URL(page.run(['get','cdp-url']).cdpUrl);
 const targets=await fetch(`http://${endpoint.host}/json/list`,{headers:{Connection:'close'}}).then(r=>r.json());
 const target=targets.find(t=>t.type==='page'&&t.url===url+'/');assert.ok(target,'Owned browser canvas');
 const socket=await new Promise((resolve,reject)=>{const s=new WebSocket(target.webSocketDebuggerUrl);s.onopen=()=>resolve(s);s.onerror=()=>reject(Error('Browser wheel connection'));});
 let timer;try{await new Promise((resolve,reject)=>{timer=setTimeout(()=>reject(Error('Browser wheel acknowledgement')),10000);socket.onmessage=event=>{const frame=JSON.parse(event.data);if(frame.id===1){if(frame.error)reject(Error(frame.error.message));else resolve();}};socket.send(JSON.stringify({id:1,method:'Input.dispatchMouseEvent',params:{type:'mouseWheel',x:1000,y:800,deltaY,deltaX:0}}));});}finally{clearTimeout(timer);socket.close();}await sleep(700);
}
export async function scenario(ui,host,url,data){
 const proof=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
 const configure=patch=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(patch)});
 const restores=async()=>(await proof()).events.filter(e=>e.method==='Mutate'&&e.op==='setChatArchived');
 const until=async(check,label)=>{for(let n=0;n<60;n++){if(await check())return;await sleep(100);}throw Error(label);};
 const filter=async query=>{await ui.click(...ui.search);await ui.key('Control+a');if(ui.replace)await ui.replace(query);else{await ui.key('Backspace');if(query)await ui.type(query);}await sleep(400);};
 const restore=async(id)=>{const before=(await restores()).length;await ui.click(...ui.restore);await until(async()=>(await restores()).length===before+1,'Visible archive restore dispatched');const event=(await restores()).at(-1);assert.equal(event.chatId,id,'Search/page restores exactly the visible conversation');assert.equal(event.targetDeviceId,data.deviceId,'Restore targets owning computer');await sleep(500);if(ui.settle)await ui.settle();};
 await configure({restoreMode:'fail'});
 await ui.shot('list');await filter('MIGRATION');await ui.shot('clean-preview-search');await filter('');await restore('library-39');if(ui.checkError){await ui.shot('unfiltered-restore-error');ui.checkError(true,'unfiltered-restore-error');if(ui.dismissError){await ui.dismissError();await ui.shot('dismissed-error');ui.checkError(false,'dismissed-error');}}
 await ui.wheel(5000);await ui.shot('pagination');await ui.click(...ui.next);await sleep(500);await ui.shot('page-two');await restore('arc-release');
 await ui.click(...ui.previous);await sleep(500);await restore('library-39');
 // Return to page two before searching: every changed query must reset the page.
 await ui.wheel(5000);await ui.click(...ui.next);await sleep(500);
 for(const [query,id]of[['MIGRATION','arc-release'],['DESIGN/SETTINGS','arc-design'],['Other project','arc-other'],['library-01','library-01']]){
  await filter(query==='library-01'?'Saved session 01':query);await restore(id);
 }
 await ui.shot('side-chat');
 await filter('No saved conversation matches this');const emptyBefore=(await restores()).length;await ui.shot('empty');await ui.click(...ui.restore);assert.equal((await restores()).length,emptyBefore,'Empty search cannot restore a stale row');
 await filter(data.computer);await restore('library-39');
 await filter(data.project);await restore('library-39');
 await filter('MIGRATION');await ui.shot('preview-search');
 const before=(await restores()).length;await configure({restoreMode:'hold'});await ui.click(...ui.restore);await ui.click(...ui.restore);await ui.shot('pending');assert.equal((await restores()).length,before+1,'Pending restore ignores repeated clicks');
 await configure({restoreMode:'fail'});await sleep(600);assert.equal((await host.call('WatchChats')).find(c=>c.id==='arc-release').archived,true,'Failed restore retains archive');await ui.shot('restore-error');if(ui.checkError)ui.checkError(true);
 await configure({restoreMode:'ok'});await ui.click(...ui.restore);await until(async()=>!(await host.call('WatchChats')).find(c=>c.id==='arc-release').archived,'Successful retry restores root');
 const chats=await host.call('WatchChats');assert.equal(chats.find(c=>c.id==='arc-release-tab').archived,false,'Restore preserves thread sessions');for(const id of ['arc-design','arc-other','library-01','library-39'])assert.equal(chats.find(c=>c.id===id).archived,true,'Other saved conversations remain archived');
 await ui.shot('restored');if(ui.checkError)ui.checkError(false);await filter('Saved session 01');await ui.shot('retained-side-chat');
 if(ui.deleteCancel)await ui.deleteCancel();
 if(ui.menuDismiss)await ui.menuDismiss();
 await ui.returnDraft();
 const final=await proof();assert.equal(final.events.filter(e=>e.blocked).length,0);assert.equal(chats.length,44,'Browsing creates no conversations');
 return {realEngine:true,compiledUi:true,metadataPreviewFixture:true,search:{title:true,preview:true,branch:true,project:true,computer:true,path:true,empty:true},paginationAndQueryReset:true,sideChatRetained:true,ownerTargetedRestore:true,duplicateRestoreGuard:true,failureRetainsArchive:true,retryRestoresChosenThread:true,liveDraftPreserved:true,providerWrites:0,fixture:final};
}
async function main(){
 let host,page,proxy,url;const shot=name=>page.screenshot('global-archive-browser-'+name+'.png');
 try{
  host=await engine('global-archive-browser',{env:{PATH:'/usr/bin:/bin',ZUSE_NO_LOGIN_SHELL:'1'}});const data=await seedGlobalArchives(host),ready=path.join(host.root,'fixture.ready');proxy=spawn('bun',[fixture('project-archive.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
  page=browser('global-archive-browser');page.open(url,fixture('project-archive-observer.js'));page.wait('window.__uiParity.methods.includes("WatchChats")');await sleep(800);
  const click=async(x,y)=>{page.click(x,y);await sleep(500);},key=async k=>{page.run(['press',k]);await sleep(150);},type=async t=>{page.run(['keyboard','type',t]);await sleep(150);};
  await click(95,184);await click(600,912);await type('Preserve this global archive draft.');await click(900,500);shot('draft-before');
  const crop=name=>execFileSync('convert',['.context/global-archive-browser-'+name+'.png','-crop','600x38+324+890','-depth','8','rgb:-']);const draft=crop('draft-before');
  await click(210,978);await click(80,272);await sleep(500);
  const checkError=(present,name=present?'restore-error':'restored')=>{
   const pixels=execFileSync('convert',['.context/global-archive-browser-'+name+'.png','-crop','900x240+330+760','-depth','8','rgb:-'],{maxBuffer:2*1024*1024});
   let red=0;for(let n=0;n<pixels.length;n+=3)if(pixels[n]>180&&pixels[n+1]<pixels[n]*0.8&&pixels[n+2]<pixels[n]*0.8)red++;
   assert.equal(red>30,present,'Archive failure is visible and clears after dismissal or retry');
  };
  const result=await scenario({
   click,key,type,shot,checkError,
   search:[600,214],restore:[1025,290],next:[1090,836],previous:[400,421],
   dismissError:async()=>{await click(1240,950);},
   menuDismiss:async()=>{await click(1100,278);shot('menu');await key('Escape');},
   wheel:dy=>canvasWheel(page,url,dy),
   returnDraft:async()=>{
    await click(110,17);await click(900,500);shot('draft-returned');
    assert.deepEqual(crop('draft-returned'),draft,'Exact live composer draft survives global archive');
   },
  },host,url,data);
  result.errors=page.run(['errors']);assert.deepEqual(result.errors.errors,[]);artifact('global-archive-browser-acceptance.json',result);console.log('Global archive browser acceptance passed.');
 }catch(error){console.error('Global archive browser failed: '+error.message);try{shot('failure');}catch{}process.exitCode=1;}finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}
}
if(process.argv[1]&&import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href)await main();
