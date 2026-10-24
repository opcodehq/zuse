// Real Git checkout creation and scripted local turns through the compiled composer.
import fs from 'node:fs';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {spawn,execFileSync} from 'node:child_process';
import assert from 'node:assert/strict';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';
let host,proxy,page,url,stage='startup';
const root=path.join(process.cwd(),'.context','creation-workspace-runtime-'+randomUUID());
fs.mkdirSync(path.join(root,'profile'),{recursive:true});
// Disable every external provider, including automatic title fallbacks.
fs.writeFileSync(path.join(root,'profile/harness-prefs.json'),JSON.stringify({enabled:[]}));
const proof=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
const click=async(x,y)=>{page.run(['batch','--bail',`mouse move ${x} ${y}`,'mouse down','mouse up','wait 200']);await sleep(250);};
const press=async key=>{page.run(['press',key]);await sleep(200);};
const shot=name=>page.screenshot('creation-workspace-'+name+'.png');
const waitFor=async check=>{for(let n=0;n<150;n++){if(await check())return;await sleep(100);}assert.fail('Creation acceptance deadline: '+stage);};
const chats=()=>host.call('WatchChats');
const prefs=()=>page.evaluate('JSON.parse(localStorage.getItem("zuse.workspace.preferences.v1"))');
try{
 host=await engine('creation-workspace',{profileRoot:root,env:{ZUSE_MOCK_DELAY_MS:'25'}});
 assert.ok((await host.call('ListHarnesses')).filter(p=>p.id!=='zuse').every(p=>p.enabled===false),'External providers stay disabled in the real engine');
 await host.call('SetWorktreeSettings',{useCustomDirectory:true,customDirectory:path.join(root,'worktrees')});
 const info=await host.call('EngineInfo'),repo=path.join(root,'project');fs.mkdirSync(repo);
 const git=(...args)=>execFileSync('git',['-C',repo,...args],{stdio:'ignore'});
 git('init','-b','main');git('config','user.email','acceptance@example.test');git('config','user.name','Acceptance');fs.writeFileSync(path.join(repo,'README.md'),'Original checkout\n');git('add','README.md');git('commit','-m','Initial project');
 await host.call('Mutate',{op:'createSpace',spaceId:'workspace-project',deviceId:info.deviceId,path:repo,name:'Workspace project',gitDetected:true});
 const ready=path.join(root,'fixture.ready');proxy=spawn('bun',[fixture('creation-workspace.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
 page=browser('creation-workspace');page.open(url,fixture('general-settings-observer.js'));page.wait('window.__generalParity.methods.includes("WatchSpaces")');await sleep(500);
 stage='choose project';await click(460,842);page.run(['keyboard','type','Workspace project']);await press('ArrowDown');await press('Enter');shot('project');
 stage='workspace menu';await click(750,842);shot('menu');
 await press('End');await press('Enter');assert.equal(Object.values(prefs().workspaceByProject)[0],'local');shot('local');
 stage='outside dismissal preserves typing';await click(750,842);await click(550,894);page.run(['keyboard','type','Keep my workspace draft']);await click(900,500);shot('draft');
 const beforeDraft=execFileSync('convert',['.context/creation-workspace-draft.png','-crop','600x38+400+880','-depth','8','rgb:-']);
 await click(750,842);await press('Escape');await click(900,500);shot('dismissed');assert.ok(execFileSync('convert',['.context/creation-workspace-dismissed.png','-crop','600x38+400+880','-depth','8','rgb:-']).equals(beforeDraft),'Workspace dismissal preserves the composer draft');
 stage='remember across reload';page.run(['reload']);page.wait('window.__generalParity.methods.includes("WatchSpaces")');await sleep(500);await click(460,842);page.run(['keyboard','type','Workspace project']);await press('ArrowDown');await press('Enter');assert.equal(Object.values(prefs().workspaceByProject)[0],'local');
 stage='isolated first send';await click(750,842);await press('Home');await press('Enter');assert.equal(Object.values(prefs().workspaceByProject)[0],'worktree');await click(550,894);await press('Control+a');page.run(['keyboard','type','Run in an isolated workspace']);await press('Enter');
 let isolated;await waitFor(async()=>{isolated=(await chats()).find(c=>c.cwd&&c.cwd!==repo);return isolated&&fs.existsSync(path.join(isolated.cwd,'.git'));});
 assert.ok(isolated.cwd.startsWith(path.join(root,'worktrees')+path.sep));assert.ok(fs.statSync(path.join(isolated.cwd,'.git')).isFile());assert.match(isolated.branch,/^zuse\//);assert.equal(fs.readFileSync(path.join(repo,'README.md'),'utf8'),'Original checkout\n');
 await waitFor(async()=>{const messages=await host.call('WatchDocMessages',{chatId:isolated.id});return messages.reset?.some(m=>m.role==='assistant'&&m.status==='complete');});shot('isolated-chat');
 stage='local first send';await press('Control+n');await sleep(400);await click(750,842);await press('End');await press('Enter');await click(550,894);page.run(['keyboard','type','Run in the original checkout']);await press('Enter');
 let local;await waitFor(async()=>{local=(await chats()).find(c=>c.id!==isolated.id&&c.cwd===repo);return local;});
 await waitFor(async()=>{const messages=await host.call('WatchDocMessages',{chatId:local.id});return messages.reset?.some(m=>m.role==='assistant'&&m.status==='complete');});shot('local-chat');
 const wire=await proof(),runs=wire.events.filter(e=>e.method==='QueueCommand');assert.equal(runs.length,2);assert.equal(runs[0].params.command.request.worktree.repoPath,repo);assert.equal(runs[0].params.command.request.worktree.base,'HEAD');assert.equal(runs[0].params.command.request.worktree.spaceId,'workspace-project');assert.equal(runs[1].params.command.request.worktree,null);assert.ok(!wire.events.some(e=>e.blocked));
 const errors=page.run(['errors']);assert.deepEqual(Array.isArray(errors)?errors:errors.errors,[]);
 artifact('creation-workspace-acceptance.json',{compiledBrowser:true,realEngine:true,realGit:true,scriptedAgentOnly:true,providerIdentityMappedToScript:true,externalProvidersDisabled:true,workspaceKeyboardSelection:true,rememberedAfterReload:true,outsideDismissalAndDraft:true,isolated:{id:isolated.id,cwd:isolated.cwd,branch:isolated.branch},local:{id:local.id,cwd:local.cwd},runs:2,wire:wire.events,errors:[]});console.log('Workspace creation passed: keyboard/persistence, draft, real isolated and local sends.');
}catch(error){console.error('Workspace creation failed at '+stage+': '+error.stack);try{shot('failure');artifact('creation-workspace-failure.json',{stage,error:error.message,wire:await proof(),chats:await chats()});}catch{}process.exitCode=1;}
finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}
