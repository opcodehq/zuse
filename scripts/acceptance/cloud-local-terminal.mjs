// Local synced-checkout and cloud shells retain independent catalogs and real processes.
import assert from 'node:assert/strict';import fs from 'node:fs';import path from 'node:path';import {randomUUID} from 'node:crypto';
import {artifact,sleep} from './harness.mjs';
const output=environment=>`window.__cloudLaunch.tools.filter(t=>t.environment===${JSON.stringify(environment)} && t.method==="TerminalData").map(t=>atob(t.data)).join("")`;
async function until(read,accept,label,timeout=30000){const end=Date.now()+timeout;let value;while(Date.now()<end){try{value=await read();if(accept(value))return value;}catch(e){value=e.message;}await sleep(150);}throw new Error(`${label}: ${JSON.stringify(value)}`);}
async function command(page,text,expected,environment){page.click(1050,973);page.run(['keyboard','type',text]);page.click(1257,975);page.wait(`${output(environment)}.includes(${JSON.stringify(expected)})`);}
export async function beforeLocalTerminals(page,file,host){
  const mirror=JSON.parse(fs.readFileSync('.context/cloud-sync-browser-ui-first.json')).mirror;
  page.click(700,500);await sleep(250);page.click(1254,66);await sleep(300);page.click(1122,66);page.wait('window.__cloudLaunch.tools.some(t=>t.environment==="cloud" && (t.method==="OpenTerminal" || (t.method==="ListTerminals" && t.result.length>0)))');await sleep(150);
  page.screenshot('cloud-local-terminal-select-inspect.png');
  // Selector positions belong to the existing 380px right panel.
  page.click(990,106);page.wait('window.__cloudLaunch.tools.some(t=>t.environment==="local" && t.method==="OpenTerminal")');
  await command(page,`printf 'host-only bytes' > local-shell-proof; printf '%s' "$$" > local-shell-pid; export ZUSE_LOCAL_SHELL=retained; printf 'LOCAL:%s:%s\\n' "$ZUSE_LOCAL_SHELL" "$PWD"`,'LOCAL:retained:','local');
  assert.equal(fs.readFileSync(path.join(mirror,'local-shell-proof'),'utf8'),'host-only bytes');
  const remote=path.join(fs.realpathSync(fs.readFileSync(file.replace(/\.[^.]+$/,'.runtime'),'utf8')),'checkout');assert.ok(!fs.existsSync(path.join(remote,'local-shell-proof')));
  const proof=page.evaluate('window.__cloudLaunch');const session=proof.tools.find(t=>t.environment==='local'&&t.method==='OpenTerminal').result;
  assert.equal(fs.realpathSync(session.cwd),fs.realpathSync(mirror));
  page.click(908,106);await command(page,`printf 'guest-only bytes' > cloud-shell-proof; printf 'CLOUD-SELECTED\\n'`,'CLOUD-SELECTED','cloud');
  assert.equal(fs.readFileSync(path.join(remote,'cloud-shell-proof'),'utf8'),'guest-only bytes');
  artifact('cloud-local-terminal-original.json',{session,mirror,pid:fs.readFileSync(path.join(mirror,'local-shell-pid'),'utf8')});
  page.click(990,106);page.screenshot('cloud-local-terminal-browser.png');
}
export async function afterLocalTerminals(page,file,host){
  const original=JSON.parse(fs.readFileSync('.context/cloud-local-terminal-original.json'));
  page.click(700,500);await sleep(250);page.click(1254,66);await sleep(300);page.click(1122,66);page.wait('window.__cloudLaunch.tools.some(t=>t.environment==="cloud"&&t.method==="ListTerminals")');
  page.click(990,106);page.wait('window.__cloudLaunch.tools.some(t=>t.environment==="local"&&t.method==="ListTerminals")');
  assert.deepEqual(page.evaluate('window.__cloudLaunch.tools.find(t=>t.environment==="local"&&t.method==="ListTerminals").result'),[original.session]);
  assert.equal(page.evaluate('window.__cloudLaunch.tools.filter(t=>t.environment==="local"&&t.method==="OpenTerminal").length'),0);
  await command(page,`printf 'LOCAL-REOPEN:%s:%s\\n' "$ZUSE_LOCAL_SHELL" "$$"`,`LOCAL-REOPEN:retained:${original.pid}`,'local');
  page.screenshot('cloud-local-terminal-browser-reopened.png');
  artifact('cloud-local-terminal-browser-acceptance.json',{actualGpui:true,hostLocalAndGuestSeparated:true,originalShell:original.session.id,samePid:original.pid,clientClosure:true,noExtraLocalShell:true,liveProvider:false});
}
export async function acceptPausedLocalTerminals(page,file,host){
  const original=JSON.parse(fs.readFileSync('.context/cloud-local-terminal-original.json'));
  const workspace=JSON.parse(fs.readFileSync('.context/cloud-sync-browser-ui-first.json')).workspaceId;
  const account=(await host.call('AuthStatus')).user.id;const row=(await host.call('CloudWorkspace',{workspaceId:workspace,expectedAccountId:account})).workspace;
  const scope={workspaceId:workspace,expectedAccountId:account,chatId:row.initialSessionId,requestId:99};
  const local=async(method,params={},extra={})=>(await host.call('CloudLocalTerminal',{...scope,method,params,...extra})).ok;
  await assert.rejects(local('WriteTerminal',{terminalId:original.session.id,data:Buffer.from('false\r').toString('base64')},{chatId:'other-conversation'}),/does not belong/);
  await assert.rejects(local('ListTerminals',{}, {expectedAccountId:'another-account'}),/account changed/);
  await host.call('CloudPause',{workspaceId:workspace,expectedAccountId:account,commandId:randomUUID()});
  await until(()=>host.call('CloudWorkspace',{workspaceId:workspace,expectedAccountId:account}),r=>r.workspace.state==='paused','Cloud pause');
  const trace=process.env.ZUSE_CLOUD_TEST_HTTP_TRACE;const before=trace&&fs.readFileSync(trace,'utf8').length;
  const second=await local('OpenTerminal',{cols:80,rows:24,cwd:'/must-not-be-used'});assert.equal(second.cwd,original.mirror);
  await local('WriteTerminal',{terminalId:second.id,data:Buffer.from(`printf 'local while paused' > local-paused-proof; printf '%s' "$$" > local-paused-pid\r`).toString('base64')});
  await until(()=>Promise.resolve(fs.existsSync(path.join(original.mirror,'local-paused-proof'))),Boolean,'Local command while paused');
  const pid=Number(fs.readFileSync(path.join(original.mirror,'local-paused-pid'),'utf8'));
  if(trace){assert.ok(!/^POST .*\/(resume|gateway\/ticket)(?:\n|$)/m.test(fs.readFileSync(trace,'utf8').slice(before)),'Local shells must not wake compute');}
  const origin=fs.readFileSync(file,'utf8').trim();const response=await fetch(`${origin}/v1/cloud/workspaces/${workspace}/archive`,{method:'POST',headers:{authorization:'Bearer dev-user','content-type':'application/json'},body:JSON.stringify({workspaceId:workspace,commandId:randomUUID()})});assert.equal(response.status,200);
  await until(()=>host.call('CloudWorkspaces'),r=>r.workspaces.some(w=>w.workspaceId===workspace&&w.localMirrorCleanup?.state==='complete'),'Archive cleanup');
  assert.ok(!fs.existsSync(path.join(original.mirror,'tracked')));assert.equal(fs.readFileSync(path.join(original.mirror,'local-paused-proof'),'utf8'),'local while paused');
  await until(()=>Promise.resolve().then(()=>{try{process.kill(pid,0);return false;}catch(e){if(e.code==='ESRCH')return true;throw e;}}),Boolean,'Local shell stopped before archive reclamation');
  await assert.rejects(local('WriteTerminal',{terminalId:second.id,data:Buffer.from('false\r').toString('base64')}),/Unarchive/);
  artifact('cloud-local-terminal-lifecycle-acceptance.json',{pausedComputeStayedPaused:true,explicitCwdIgnored:true,wrongConversationRejected:true,wrongAccountRejected:true,archiveStopsShell:true,ownedFilesReclaimed:true,localOnlyFilesPreserved:true,liveProvider:false});
}
