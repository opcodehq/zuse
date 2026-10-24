// Real engine archive metadata and restoration with isolated saved transcript responses.
import fs from 'node:fs';
import path from 'node:path';
import {spawn,execFileSync} from 'node:child_process';
import assert from 'node:assert/strict';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';
import {seedProjectArchives} from './fixtures/project-archive-data.mjs';
let host,page,proxy,url;
const pause=()=>sleep(400);
const click=async(x,y)=>{page.click(x,y);await pause();};
const key=k=>page.run(['press',k]);
const shot=name=>page.screenshot('project-archive-'+name+'.png');
const configure=async patch=>{await fetch(url+'/__fixture',{method:'POST',body:JSON.stringify(patch)});};
const proof=()=>fetch(url+'/__fixture').then(r=>r.json());
const reads=async()=> (await proof()).events.filter(e=>e.method==='WatchDocMessages');
const restores=async()=> (await proof()).events.filter(e=>e.method==='Mutate'&&e.op==='setChatArchived');
const crop=name=>execFileSync('convert',['.context/project-archive-'+name+'.png','-crop','840x500+336+110','-depth','8','rgb:-'],{maxBuffer:2*1024*1024});
try{
 host=await engine('project-archive-acceptance');const data=await seedProjectArchives(host);
 const ready=path.join(host.root,'fixture.ready');proxy=spawn('bun',[fixture('project-archive.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
 page=browser('project-archive-acceptance');page.open(url,fixture('project-archive-observer.js'));page.wait('window.__uiParity.methods.includes("WatchChats")');await sleep(800);
 await click(95,184);await click(600,912);page.run(['keyboard','type','Preserve this live draft.']);await pause();await click(900,500);shot('draft-before');
 const draftBefore=execFileSync('convert',['.context/project-archive-draft-before.png','-crop','600x38+324+890','-depth','8','rgb:-']);
 await click(175,145);shot('project-menu');await click(258,162);shot('list');
 await click(500,102);page.run(['keyboard','type','design']);await pause();shot('filtered');await click(400,203);
 assert.equal((await reads()).at(-1)?.chatId,'arc-design','Filtered row opens the matching root in this project');
 key('Escape');await pause();await click(500,102);key('Control+a');page.run(['keyboard','type','not found']);await pause();shot('no-results');
 const beforeEmpty=(await reads()).length;await click(400,203);assert.equal((await reads()).length,beforeEmpty,'Empty filter has no clickable stale row');
 await click(500,102);key('Control+a');key('Backspace');await pause();await click(400,203);shot('preview');assert.equal((await reads()).at(-1)?.chatId,'arc-release');
 await click(480,81);shot('session');assert.equal((await reads()).at(-1)?.chatId,'arc-release-tab');
 // A cancelled root read may arrive after selecting a different session.
 await configure({holdDocs:['arc-release']});await click(310,81);shot('loading');await click(480,81);shot('before-late');await configure({releaseDocs:true,holdDocs:[]});await pause();shot('after-late');assert.deepEqual(crop('before-late'),crop('after-late'),'Cancelled transcript response must not replace selected session');
 await configure({failDoc:'arc-release'});await click(310,81);shot('read-error');const beforeRetry=(await reads()).length;await configure({failDoc:null});await click(756,545);await pause();shot('read-retry');assert.equal((await reads()).length,beforeRetry+1,'Read failure retry issues a new saved transcript request');
 await click(95,184);await click(900,500);shot('draft-returned');assert.deepEqual(execFileSync('convert',['.context/project-archive-draft-returned.png','-crop','600x38+324+890','-depth','8','rgb:-']),draftBefore,'Archive preview preserves the rendered live composer draft');
 await click(175,233);await click(258,250);shot('other-project');await click(400,203);assert.equal((await reads()).at(-1)?.chatId,'arc-other','Other project opens only its own archive');
 await click(175,145);await click(258,162);await click(400,203);
 await configure({restoreMode:'hold'});const beforeRestore=(await restores()).length;await click(1150,974);await click(1150,974);shot('restore-pending');assert.equal((await restores()).length,beforeRestore+1,'Repeated pending restore is ignored');
 await configure({restoreMode:'fail'});await sleep(600);shot('restore-error');assert.equal((await host.call('WatchChats')).find(c=>c.id==='arc-release').archived,true,'Failed restore retains archive');
 await configure({restoreMode:'ok'});await click(1150,974);await sleep(700);shot('restored');const chats=await host.call('WatchChats');assert.equal(chats.find(c=>c.id==='arc-release').archived,false);assert.equal(chats.find(c=>c.id==='arc-release-tab').archived,false);assert.equal(chats.find(c=>c.id==='arc-design').archived,true);assert.equal(chats.find(c=>c.id==='arc-other').archived,true);
 const final=await proof();assert.equal(final.events.filter(e=>e.blocked).length,0);assert.ok(final.events.filter(e=>e.method==='WatchDocMessages').every(e=>e.targetDeviceId===data.deviceId),'Reads target owning computer');
 artifact('project-archive-acceptance.json',{realEngine:true,compiledBrowser:true,keyboardBackToList:true,filteredProjectList:true,separateReadOnlyPreview:true,sessionSwitch:true,lateReadIgnored:true,liveDraftPreserved:true,restoreFailureRetainsArchive:true,duplicateRestoreGuard:true,restoresOnlyChosenThread:true,providerWrites:0,fixture:final,chats,errors:page.run(['errors'])});console.log('Project archive acceptance passed.');
}catch(error){console.error('Project archive acceptance failed: '+error.message);try{shot('failure');}catch{}process.exitCode=1;}finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}
