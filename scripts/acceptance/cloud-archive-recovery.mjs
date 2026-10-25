// Acknowledged Cloud recovery facts with real saved-chat metadata and public UI input.
import fs from 'node:fs';import path from 'node:path';import assert from 'node:assert/strict';import {spawn,execFileSync} from 'node:child_process';import {pathToFileURL} from 'node:url';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';
import {seed as seedMetadata,draft,workspace} from './fixtures/cloud-archive-settings-data.mjs';
export const recovery={workspaceId:'ws-recovery',archived:true,machineRetained:true,backupVerified:false,computeRetired:false,retentionDeadline:1793361600000,operationId:'backup-operation',phase:'uploading',verifiedChunks:0,totalChunks:4};
export const recoveryRow=status=>workspace('ws-recovery','Release recovery','release/recovery','archived',status===null?{}:{archiveStatus:{...recovery,...status}});
export async function seed(host){const data=await seedMetadata(host);data.rows=[recoveryRow({})];return data;}
export async function scenario(ui,url){
 const proof=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
 const configure=p=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(p)});
 const lists=p=>p.events.filter(e=>e.method==='CloudWorkspaces');
 async function until(check,label){for(let n=0;n<100;n++){const p=await proof();if(check(p))return p;await sleep(100);}throw Error(label);}
 const phase=p=>lists(p).at(-1)?.reply?.rows?.[0]?.archiveStatus?.phase;
 async function refresh(row){await configure({rows:[row]});const before=lists(await proof()).length;await ui.click(...ui.points.refresh);await until(p=>lists(p).length>before&&lists(p).at(-1).reply,'Recovery refresh');await sleep(600);}
 async function filter(value){await ui.click(...ui.points.search);await ui.key('Control+a');if(ui.replace)await ui.replace(value);else{await ui.key('Backspace');if(value)await ui.type(value);}await sleep(500);}
 console.log('Recovery: automatic upload, verification and retirement progress');
 await until(p=>phase(p)==='uploading','Initial recovery facts');await ui.shot('uploading');
 await configure({rows:[recoveryRow({phase:'verifying',verifiedChunks:1})]});await until(p=>phase(p)==='verifying','Archived workspaces poll verification');await ui.shot('verifying');
 await configure({rows:[recoveryRow({phase:'retiring',verifiedChunks:4,backupVerified:true})]});await until(p=>phase(p)==='retiring','Verified backup precedes retirement');await ui.shot('retiring');
 const complete=recoveryRow({phase:'retired',verifiedChunks:4,backupVerified:true,computeRetired:true,machineRetained:false});
 await configure({rows:[complete]});await until(p=>phase(p)==='retired','Acknowledged compute retirement');await ui.shot('retired');
 const stopped=lists(await proof()).length;await sleep(2700);assert.equal(lists(await proof()).length,stopped,'Stable recovery stops polling');
 console.log('Recovery: search, unavailable metadata and owner consistency');
 await filter('Backup verified');await ui.shot('verified-search');await filter('no matching recovery');await ui.shot('empty-search');await filter('');
 await refresh(recoveryRow(null));await ui.shot('legacy');await refresh(recoveryRow({workspaceId:'different-workspace',phase:'verifying'}));await ui.shot('mismatched');ui.compare('legacy','mismatched');
 await refresh(recoveryRow({phase:'future-phase'}));await ui.shot('unknown-phase');ui.compare('legacy','unknown-phase');
 await refresh(recoveryRow({phase:'verifying',errorCode:'archive-verify-failed'}));await ui.shot('recovery-error');const failed=lists(await proof()).length;await sleep(2700);assert.equal(lists(await proof()).length,failed,'Failed recovery waits for manual refresh');
 await refresh(complete);await ui.shot('recovered');await ui.compact();await ui.light();
 console.log('Recovery: read-only history, restore retry and exact live draft');
 const beforeRead=(await proof()).events.filter(e=>e.method==='CloudHistory').length;await ui.click(...ui.points.view);await until(p=>p.events.filter(e=>e.method==='CloudHistory').length===beforeRead+1,'Recovery history remains available');await ui.shot('history');await ui.click(...ui.points.back);
 await configure({modes:{restore:'hold'}});await ui.click(...ui.points.restore);await ui.click(...ui.points.restore);let restores=(await proof()).events.filter(e=>e.method==='CloudUnarchive');assert.equal(restores.length,1);await ui.shot('restore-pending');
 await configure({modes:{restore:'fail'}});await sleep(700);await ui.shot('restore-error');await configure({modes:{restore:'ok'}});await ui.click(...ui.points.restoreError);await until(p=>p.events.filter(e=>e.method==='CloudUnarchive').at(-1)?.reply?.state==='paused','Restore completes paused');
 restores=(await proof()).events.filter(e=>e.method==='CloudUnarchive');assert.equal(restores.length,2);assert.equal(restores[0].commandId,restores[1].commandId,'Retry preserves the operation');
 await ui.returnDraft();const end=await proof();assert.equal(end.events.filter(e=>e.blocked||e.rejected).length,0);assert.ok(end.events.every(e=>!e.method||e.expectedAccountId==='archive-owner'));
 return {compiledUi:true,realMetadataEngine:true,recoveryProtocolFixture:true,automaticRecoveryProgress:true,verifiedBackupAndRetirementDistinct:true,absoluteComputeRetentionDeadline:true,stableAndFailedPollingStops:true,statusSearch:true,legacyMetadataFallback:true,mismatchedWorkspaceIgnored:true,unknownPhaseFallback:true,readOnlyHistory:true,restoreRetryOperationRetained:true,restoresPaused:true,compact:true,light:true,exactDraftPreserved:true,providerExecutions:0,fixture:end};
}
async function main(){let host,page,proxy,url;try{
 host=await engine('archive-recovery-browser',{env:{PATH:'/usr/bin:/bin',ZUSE_NO_LOGIN_SHELL:'1'}});const data=await seed(host),ready=path.join(host.root,'fixture.ready'),file=path.join(host.root,'fixture-data.json');fs.writeFileSync(file,JSON.stringify(data));proxy=spawn('bun',[fixture('cloud-archive-settings.mjs'),String(host.port),ready,path.resolve('dist/web'),file],{stdio:'ignore'});while(!fs.existsSync(ready)){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
 page=browser('archive-recovery-browser');page.open(url,fixture('cloud-archive-settings-observer.js'));page.wait('window.__uiParity.methods.includes("WatchChats")');await sleep(800);
 const click=async(x,y)=>{page.click(x,y);await sleep(650);},key=async k=>{page.run(['press',k]);await sleep(300);},type=async s=>{page.run(['keyboard','type',s]);await sleep(350);},shot=name=>page.screenshot('archive-recovery-browser-'+name+'.png');
 await click(95,210);await click(600,912);await type(draft);async function copied(){await key('Control+a');await key('Control+c');assert.equal(page.evaluate('window.__archiveDraftCopied'),draft);await key('ArrowRight');}await copied();await click(900,400);shot('draft-before');await click(210,978);await click(80,272);
 const pixels=name=>execFileSync('convert',['.context/archive-recovery-browser-'+name+'.png','-crop','768x400+360+340','-depth','8','rgb:-'],{maxBuffer:1024*1024});
 const result=await scenario({click,key,type,shot,points:{search:[600,219],refresh:[1080,359],view:[883,566],back:[250,56],restore:[986,566],restoreError:[986,590]},compare:(a,b)=>assert.deepEqual(pixels(a),pixels(b),'Unrelated or unrecognized recovery metadata cannot change the card'),
 compact:async()=>{page.run(['set','viewport','760','900']);await sleep(700);shot('compact');page.run(['set','viewport','1280','1000']);await sleep(700);},
 light:async()=>{await click(80,62);await click(1036,378);await click(80,272);shot('light');},
 returnDraft:async()=>{await click(110,17);await click(600,912);await copied();await click(900,400);shot('draft-returned');}},url);
 assert.equal((await host.call('WatchChats')).length,3);result.errors=page.run(['errors']);assert.deepEqual(result.errors.errors,[]);artifact('archive-recovery-browser-acceptance.json',result);console.log('Cloud archive recovery browser acceptance passed.');
}catch(error){try{page?.screenshot('archive-recovery-browser-failure.png');artifact('archive-recovery-browser-failure.json',{error:error.message,fixture:await fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json())});}catch{}console.error(error);process.exitCode=1;}finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}}
if(process.argv[1]&&import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href)await main();
