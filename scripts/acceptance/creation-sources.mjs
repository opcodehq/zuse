// Compiled browser + actual source/checkout/queue RPCs, with isolated GitHub and agent fixtures.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn,execFileSync} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';
let host,page,proxy,url,stage='start';
const root=path.resolve('.context/creation-sources-runtime-'+randomUUID());
fs.mkdirSync(path.join(root,'profile'),{recursive:true});
fs.writeFileSync(path.join(root,'profile/harness-prefs.json'),JSON.stringify({enabled:[]}));
const bin=path.join(root,'bin');fs.mkdirSync(bin);fs.copyFileSync(fixture('creation-sources-gh.py'),path.join(bin,'gh'));fs.chmodSync(path.join(bin,'gh'),0o755);
const proof=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
const shot=name=>page.screenshot('creation-source-browser-'+name+'.png');
const click=async(x,y)=>{page.run(['batch','--bail',`mouse move ${Math.round(x)} ${Math.round(y)}`,'mouse down','mouse up','wait 150']);await sleep(200);};
const press=async key=>{page.run(['press',key]);await sleep(180);};
let menuTab='prs';
const named=async name=>{
 const tabY=menuTab==='branches'?686:706;
 const positions={
  'Create from…':[1070,842],
  'Clear selected source':[1034,842],
  'Retry source list':[850,790],
  'PRs':[644,tabY],
  'Branches':[715,tabY],
  'Issues':[785,tabY],
 };
 assert.ok(positions[name],'Known physical control: '+name);
 console.log('Click',name,...positions[name]);
 await click(...positions[name]);
 if(name==='Create from…'||['PRs','Branches','Issues'].includes(name))await sleep(700);
 if(['PRs','Branches','Issues'].includes(name))menuTab={PRs:'prs',Branches:'branches',Issues:'issues'}[name];
};
const wait=async check=>{for(let n=0;n<150;n++){if(await check())return;await sleep(100);}assert.fail('Deadline at '+stage);};
const completed=async id=>(await host.call('WatchDocMessages',{chatId:id})).reset?.some(m=>m.role==='assistant'&&m.status==='complete');
try{
 host=await engine('creation-sources',{profileRoot:root,env:{PATH:bin+path.delimiter+process.env.PATH,ZUSE_NO_LOGIN_SHELL:'1',ZUSE_CREATION_SOURCE_FIXTURE:root,ZUSE_MOCK_DELAY_MS:'25'}});
 await host.call('SignOut');await host.call('SetWorktreeSettings',{useCustomDirectory:true,customDirectory:path.join(root,'worktrees')});
 const info=await host.call('EngineInfo'),repo=path.join(root,'project');fs.mkdirSync(repo);
 const git=(...args)=>execFileSync('git',['-C',repo,...args],{encoding:'utf8'}).trim();
 git('init','-b','main');git('config','user.name','Source acceptance');git('config','user.email','source@example.test');
 fs.writeFileSync(path.join(repo,'README.md'),'Primary checkout\n');git('add','README.md');git('commit','-m','Initial');
 git('switch','-c','feature/available');fs.writeFileSync(path.join(repo,'source.txt'),'Selected branch content\n');git('add','source.txt');git('commit','-m','Source change');git('branch','pr-fixture');const sourceHead=git('rev-parse','HEAD');git('switch','main');git('branch','feature/reused');
 const reused=path.join(root,'reused');git('worktree','add',reused,'feature/reused');fs.writeFileSync(path.join(reused,'keep.txt'),'Uncommitted work\n');
 fs.writeFileSync(path.join(repo,'README.md'),'Unsaved primary changes\n');const initialStatus=git('status','--porcelain=v1');
 await host.call('Mutate',{op:'createSpace',spaceId:'source-project',deviceId:info.deviceId,path:repo,name:'Source project',gitDetected:true});
 fs.writeFileSync(path.join(root,'fail-pr-list'),'');
 const ready=path.join(root,'fixture.ready');proxy=spawn('bun',[fixture('creation-workspace.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 await wait(()=>fs.existsSync(ready));url=fs.readFileSync(ready,'utf8');
 page=browser('creation-sources');page.open(url,fixture('general-settings-observer.js'));page.wait('window.__generalParity.methods.includes("WatchSpaces")');await sleep(500);
 stage='choose project';await click(460,842);page.run(['keyboard','type','Source project']);await press('ArrowDown');await press('Enter');
 stage='read failure and retry';await named('Create from…');await wait(async()=>(await proof()).events.some(e=>e.method==='ListCreationSources'));await sleep(300);shot('error');await named('Retry source list');await sleep(350);shot('prs');
 stage='tabs, trimmed author filter, escape';page.run(['keyboard','type',' alice ']);await sleep(250);shot('author');await press('Escape');
 assert.equal((await proof()).events.filter(e=>e.method==='QueueCommand').length,0);
 stage='issue prefill and clear';const beforeReopen=(await proof()).events.filter(e=>e.method==='ListCreationSources').length;await named('Create from…');assert.equal((await proof()).events.filter(e=>e.method==='ListCreationSources').length,beforeReopen+1,'Escape dismissed the previous menu, so one click reopens it');await named('Issues');await sleep(300);page.run(['keyboard','type','93']);await press('Enter');await sleep(350);shot('issue');assert.ok((await proof()).events.some(e=>e.method==='ResolveCreationSource'&&e.params.source.kind==='issue'),'Issue selection reaches the real source adapter');await named('Clear selected source');
 stage='existing branch reuse';await named('Create from…');await named('Branches');await sleep(300);page.run(['keyboard','type','reused']);await press('Enter');await sleep(350);shot('reused');await named('Clear selected source');
 assert.equal(fs.readFileSync(path.join(reused,'keep.txt'),'utf8'),'Uncommitted work\n');
 stage='branch selection';await named('Create from…');await named('Branches');await sleep(300);page.run(['keyboard','type','available']);await press('Enter');await sleep(350);shot('selected');
 assert.equal((await proof()).events.filter(e=>e.method==='QueueCommand').length,0,'Choosing sources does not start a chat');
 stage='first send uses selected checkout';await click(550,894);await press('Control+a');page.run(['keyboard','type','Run against the selected branch']);await press('Enter');
 let chat;await wait(async()=>{chat=(await host.call('WatchChats')).find(c=>c.branch==='feature/available');return chat&&fs.existsSync(path.join(chat.cwd,'.git'));});await wait(()=>completed(chat.id));await sleep(250);shot('reply');
 assert.notEqual(chat.cwd,repo);assert.equal(execFileSync('git',['-C',chat.cwd,'rev-parse','HEAD'],{encoding:'utf8'}).trim(),sourceHead);assert.equal(fs.readFileSync(path.join(chat.cwd,'source.txt'),'utf8'),'Selected branch content\n');
 assert.equal(git('branch','--show-current'),'main');assert.equal(git('status','--porcelain=v1'),initialStatus);assert.equal(fs.readFileSync(path.join(repo,'README.md'),'utf8'),'Unsaved primary changes\n');
 stage='issue context reaches provider';await press('Control+n');await sleep(300);await named('Create from…');await named('Issues');await sleep(300);page.run(['keyboard','type','93']);await press('Enter');await sleep(300);await click(550,894);await press('Enter');
 let issueChat;await wait(async()=>{issueChat=(await host.call('WatchChats')).find(c=>c.id!==chat.id);return !!issueChat;});await wait(()=>completed(issueChat.id));await sleep(250);shot('issue-reply');
 const wire=await proof(),runs=wire.events.filter(e=>e.method==='QueueCommand');assert.equal(runs.length,2);assert.equal(runs[0].params.command.request.worktree,null);assert.deepEqual(runs[0].params.command.request.attachments,[],'Cleared issue context stays out of the branch chat');assert.equal(runs[0].params.command.request.cwd,chat.cwd);assert.ok(runs[1].params.command.request.attachments.some(p=>p.endsWith('issue-93.md')));assert.match(runs[1].params.command.request.prompt,/issue-93\.md/);assert.ok(!wire.events.some(e=>e.blocked));
 const errors=page.run(['errors']);assert.deepEqual(Array.isArray(errors)?errors:errors.errors,[]);
 artifact('creation-source-browser-acceptance.json',{compiledBrowser:true,realEngine:true,realGit:true,githubFixture:true,scriptedAgentOnly:true,readRetry:true,sourceTabs:true,trimmedAuthorSearch:true,issuePrefillAndClear:true,reusedDirtyCheckout:true,choosingDoesNotStartChat:true,sourceFirstSend:true,issueContextFirstSend:true,primaryCheckoutPreserved:true,runs:2,chat,issueChat,wire:wire.events,errors:[]});
 console.log('Source browser acceptance passed: source tabs, retry, reuse, branch first send, issue context and primary checkout preservation.');
}catch(e){console.error('Source browser acceptance failed at '+stage+': '+e.stack);try{shot('failure');artifact('creation-source-browser-failure.json',{stage,error:e.message,snapshot:page.run(['snapshot']),wire:await proof(),root,url});}catch{}process.exitCode=1;if(process.env.ZUSE_ACCEPTANCE_HOLD){console.log('Holding owned fixture for physical inspection:',url,root);while(!fs.existsSync('.context/creation-source-browser.inspect.done'))await sleep(250);}}
finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}
