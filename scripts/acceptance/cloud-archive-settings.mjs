// Public UI interactions with a real metadata engine and isolated Cloud/account replies.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {pathToFileURL} from 'node:url';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';
import {seed,draft,initialRows,cleanupRow,workspace} from './fixtures/cloud-archive-settings-data.mjs';
export async function cleanupFailureScenario(ui,url){
 const proof=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
 const configure=p=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(p)});
 const cleanups=p=>p.events.filter(e=>e.method==='CloudCleanupMirror');
 async function until(check,label){for(let n=0;n<100;n++){const p=await proof();if(check(p))return p;await sleep(100);}throw Error(label);}
 await configure({rows:[cleanupRow],modes:{cleanup:'hold'}});await ui.click(...ui.points.refresh);await sleep(700);
 const before=cleanups(await proof()).length;
 await ui.click(...ui.points.cleanup);await ui.click(...ui.points.cleanup);
 assert.equal(cleanups(await proof()).length,before+1,'One local cleanup remains pending');
 await configure({modes:{cleanup:'fail'}});await sleep(700);await ui.shot('cleanup-retry-error');
 await configure({modes:{cleanup:'ok'}});await ui.click(...ui.points.cleanupError);
 const end=await until(p=>cleanups(p).at(-1)?.reply,'Failed cleanup can be retried from its card');
 assert.equal(cleanups(end).length,before+2);assert.equal(cleanups(end).at(-1).commandId,cleanups(end).at(-2).commandId,'Cleanup retry retains its operation');
 await sleep(650);await ui.shot('cleanup-retry-complete');return {cleanupFailureRetry:true,onePendingCleanup:true,distinctErrorElements:true};
}
export async function scenario(ui,url){
 const proof=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
 const configure=p=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(p)});
 const events=(p,method)=>p.events.filter(e=>e.method===method);
 async function until(check,label){for(let n=0;n<100;n++){const p=await proof();if(check(p))return p;await sleep(100);}throw Error(label);}
 const click=name=>ui.click(...ui.points[name]);
 async function refresh(rows){if(rows)await configure({rows});const before=events(await proof(),'CloudWorkspaces').length;await click('refresh');await until(p=>events(p,'CloudWorkspaces').length>before,'Refresh dispatched');await sleep(650);}
 async function search(value){await click('search');await ui.key('Control+a');if(ui.replace)await ui.replace(value);else{await ui.key('Backspace');if(value)await ui.type(value);}await sleep(600);}
 console.log('Cloud archives: account scope, automatic progress and one pending refresh');
 await until(p=>events(p,'CloudWorkspaces').some(e=>e.reply),'Initial archives loaded');await ui.shot('list');
 const first=await proof();assert.equal(first.account,'archive-owner');assert.ok(events(first,'CloudWorkspaces').every(e=>e.expectedAccountId==='archive-owner'&&e.expectedOrganizationId===null));
 const restores=events(first,'CloudUnarchive').length;await click('secondRestore');assert.equal(events(await proof(),'CloudUnarchive').length,restores,'Archiving chat cannot restore early');
 const beforePoll=events(await proof(),'CloudWorkspaces').length;await configure({modes:{list:'hold'}});
 await until(p=>events(p,'CloudWorkspaces').length>beforePoll,'Background progress poll');const held=events(await proof(),'CloudWorkspaces').length;
 await click('refresh');await click('refresh');assert.equal(events(await proof(),'CloudWorkspaces').length,held,'A pending background read cannot be refreshed twice');await ui.shot('poll-held');
 const complete=initialRows.map(row=>row.workspaceId==='ws-pending'?{...row,state:'archived',statusCode:'archived'}:row);
 await configure({rows:complete,modes:{list:'ok'}});
 await until(p=>events(p,'CloudWorkspaces').some(e=>e.reply?.rows?.some(row=>row.workspaceId==='ws-pending'&&row.state==='archived')),'Automatic poll receives completed archive');await sleep(650);await ui.shot('progress-complete');
 console.log('Cloud archives: restore failure, duplicate guard and retry');
 await configure({modes:{restore:'hold'}});await click('secondRestore');await click('secondRestore');
 await until(p=>events(p,'CloudUnarchive').length===restores+1,'One restore request');await ui.shot('restore-pending');
 await configure({modes:{restore:'fail'}});await sleep(700);assert.equal((await proof()).rows.find(r=>r.workspaceId==='ws-pending').state,'archived');await ui.shot('restore-error');
 await configure({modes:{restore:'ok'}});await click('secondRestoreError');
 await until(p=>p.rows.find(r=>r.workspaceId==='ws-pending').state==='paused','Restore leaves compute paused');await sleep(700);await ui.shot('restored');
 const restoreEvents=events(await proof(),'CloudUnarchive');assert.equal(restoreEvents.length,restores+2);assert.equal(restoreEvents.at(-1).commandId,restoreEvents.at(-2).commandId,'Restore retry reuses its operation');
 console.log('Cloud archives: saved history, rejected account envelope and session switching');
 await configure({modes:{read:'fail'}});await click('view');await until(p=>events(p,'CloudHistory').at(-1)?.failed,'Saved history read failure');await sleep(650);await ui.shot('history-error');
 await configure({modes:{read:'ok'},responseOwner:'unrelated-account'});await click('viewError');await sleep(700);await ui.shot('history-account-rejected');
 await configure({responseOwner:null});await click('viewError');await until(p=>events(p,'CloudHistory').at(-1)?.reply?.accountId==='archive-owner','History retry belongs to current account');await sleep(850);await ui.shot('history');
 await click('secondSession');await until(p=>events(p,'CloudHistory').at(-1)?.reply?.sessionId==='arc-release-tab','Second saved conversation');await sleep(700);await ui.shot('second-session');
 await click('firstSession');await until(p=>events(p,'CloudHistory').at(-1)?.reply?.sessionId==='arc-release','Root conversation remains reachable');await sleep(700);await ui.shot('root-session-returned');await click('historyBack');await sleep(700);
 console.log('Cloud archives: confirmed deletion and local cleanup retry');
 await refresh([complete[0],cleanupRow]);let deletes=events(await proof(),'CloudDelete').length;
 await click('delete');await ui.shot('delete-confirmation');await ui.key('Escape');assert.equal(events(await proof(),'CloudDelete').length,deletes,'Escape cancels deletion');
 await click('delete');await ui.key('Tab');await ui.key('Enter');await until(p=>events(p,'CloudDelete').length===deletes+1,'Keyboard confirmation deletes once');await sleep(700);await ui.shot('delete-retry');
 await click('retryDelete');await until(p=>events(p,'CloudDelete').length===deletes+2,'Deletion retry');const deleteEvents=events(await proof(),'CloudDelete');assert.equal(deleteEvents.at(-1).commandId,deleteEvents.at(-2).commandId,'Deletion retry retains its operation');
 const deleted={...complete[0],state:'deleted',desiredState:'deleted',statusCode:'deleted'};await configure({rows:[deleted,cleanupRow]});
 await until(p=>events(p,'CloudWorkspaces').some(e=>e.reply?.rows?.some(r=>r.workspaceId==='ws-release'&&r.state==='deleted')),'Deletion progress is polled');await sleep(650);await ui.shot('cleanup-error');
 await click('cleanup');await until(p=>events(p,'CloudCleanupMirror').some(e=>e.reply),'Cleanup retry acknowledged');await sleep(700);await ui.shot('cleanup-complete');await cleanupFailureScenario(ui,url);
 console.log('Cloud archives: shared title, branch and status search');
 await refresh([...complete,cleanupRow]);for(const value of ['release/v3','feature/search','cleanup','absent-result']){await search(value);await ui.shot('search-'+value.replaceAll('/','-'));}await search('');
 console.log('Cloud archives: account replacement retires a pending read');
 await refresh([complete[0]]);await configure({modes:{read:'hold'}});const reads=events(await proof(),'CloudHistory').length;await click('view');await until(p=>events(p,'CloudHistory').length===reads+1,'Old-account history is pending');
 const next=workspace('ws-private','Second account archive','private/account');await configure({account:'archive-owner-two',rows:[next]});
 await until(p=>events(p,'CloudWorkspaces').some(e=>e.reply?.accountId==='archive-owner-two'),'Changed account loads its own archives');await configure({modes:{read:'ok'}});await sleep(750);await ui.shot('old-read-retired');
 await click('view');await until(p=>events(p,'CloudHistory').at(-1)?.workspaceId==='ws-private'&&events(p,'CloudHistory').at(-1)?.reply?.accountId==='archive-owner-two','Old history never replaces the new list');await sleep(700);await click('historyBack');await sleep(700);
 await configure({orgId:'organization-fixture'});await sleep(650);await ui.shot('personal-account');await configure({account:null,orgId:null});await sleep(650);await ui.shot('signed-out');
 const before=events(await proof(),'CloudDelete').length;await click('delete');assert.equal(events(await proof(),'CloudDelete').length,before,'Signed-out catalog has no stale delete action');
 await configure({account:'archive-owner',rows:[complete[0]],modes:{list:'hold'}});const waiting=await until(p=>events(p,'CloudWorkspaces').at(-1)?.expectedAccountId==='archive-owner'&&!events(p,'CloudWorkspaces').at(-1)?.reply,'Signed-in account starts one read');
 const pending=events(waiting,'CloudWorkspaces').length;await click('refresh');await click('refresh');assert.equal(events(await proof(),'CloudWorkspaces').length,pending,'Account loading ignores duplicate refreshes');await ui.shot('loading');
 await configure({modes:{list:'fail'}});await sleep(650);await ui.shot('load-error');await configure({modes:{list:'ok'}});await refresh();await ui.shot('recovered');
 await ui.compact();await ui.returnDraft();const end=await proof();assert.equal(end.events.filter(e=>e.blocked||e.rejected).length,0);
 return {compiledUi:true,realMetadataEngine:true,cloudProtocolFixture:true,generalAccountScope:true,workspaceSyncAccountSeparate:true,onePendingRead:true,automaticArchiveProgress:true,restoreFailureRetry:true,restoreRemainsPaused:true,verifiedAccountEnvelope:true,savedSessionSwitchAndReturn:true,deleteConfirmationAndCancel:true,sameDeletionOperation:true,localCleanupRetry:true,titleBranchStatusSearch:true,oldAccountReadRetired:true,signedOutAndOrganizationGates:true,loadFailureRetry:true,compact:true,exactDraftPreserved:true,providerExecutions:0,fixture:end};
}
async function main(){
 let host,page,proxy,url;
 try{
  host=await engine('cloud-archive-settings-browser',{env:{PATH:'/usr/bin:/bin',ZUSE_NO_LOGIN_SHELL:'1'}});const data=await seed(host),ready=path.join(host.root,'fixture.ready'),file=path.join(host.root,'fixture-data.json');fs.writeFileSync(file,JSON.stringify(data));
  proxy=spawn('bun',[fixture('cloud-archive-settings.mjs'),String(host.port),ready,path.resolve('dist/web'),file],{stdio:'ignore'});while(!fs.existsSync(ready)){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
  page=browser('cloud-archive-settings-browser');page.open(url,fixture('cloud-archive-settings-observer.js'));page.wait('window.__uiParity.methods.includes("WatchChats")');await sleep(800);
  const click=async(x,y)=>{page.click(x,y);await sleep(650);},key=async k=>{page.run(['press',k]);await sleep(300);},type=async s=>{page.run(['keyboard','type',s]);await sleep(350);},shot=name=>page.screenshot('cloud-archive-cleanup-browser-'+name+'.png');
  await click(95,210);page.wait('window.__uiParity.methods.includes("WatchDocMessages")');await sleep(700);await click(600,912);await type(draft);
  async function copied(){await key('Control+a');await key('Control+c');assert.equal(page.evaluate('window.__archiveDraftCopied'),draft,'Exact real composer draft');await key('ArrowRight');}
  await copied();await click(900,400);shot('draft-before');await click(210,978);await click(80,272);await sleep(700);
  const result=await scenario({click,key,type,shot,
   points:{search:[600,219],refresh:[1080,359],secondRestore:[986,612],secondRestoreError:[986,636],view:[883,482],viewError:[883,506],firstSession:[280,98],secondSession:[410,98],historyBack:[250,56],delete:[1075,482],retryDelete:[1055,482],cleanup:[1030,530],cleanupError:[1030,554]},
   compact:async()=>{page.run(['set','viewport','760','900']);await sleep(800);shot('compact');page.run(['set','viewport','1280','1000']);await sleep(700);},
   returnDraft:async()=>{await click(110,17);await sleep(700);await click(600,912);await copied();await click(900,400);shot('draft-returned');},
  },url);
  result.errors=page.run(['errors']);assert.deepEqual(result.errors.errors,[]);assert.equal((await host.call('WatchChats')).length,3);artifact('cloud-archive-cleanup-browser-acceptance.json',result);console.log('Cloud archive settings browser acceptance passed.');
 }catch(error){try{page?.screenshot('cloud-archive-cleanup-browser-failure.png');artifact('cloud-archive-cleanup-browser-failure.json',{error:error.message,fixture:await fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json())});}catch{}console.error(error);process.exitCode=1;}
 finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}
}
if(process.argv[1]&&import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href)await main();
