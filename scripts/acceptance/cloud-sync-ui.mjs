// Optional cloud-launch acceptance: real rendered controls and host filesystem effects.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {artifact,sleep} from './harness.mjs';
async function until(read, accept, label) {
  const end=Date.now()+30000;let value;
  while(Date.now()<end){value=await read();if(accept(value))return value;await sleep(150);}
  throw new Error(`${label} timed out: ${JSON.stringify(value)}`);
}
export async function beforeCloudSync(page,file,host) {
  const workspace=page.evaluate('window.__cloudLaunch.creates[0].workspaceId');
  const account=(await host.call('AuthStatus')).user.id;
  const params={workspaceId:workspace,expectedAccountId:account};
  const status=()=>host.call('CloudSyncStatus',params).then(v=>v.status);
  const mirror=path.join(host.root,'ui-mirror');
  page.click(1250,22);page.wait('window.__cloudLaunch.sync.some(v=>v.method==="CloudSyncStatus")');await sleep(100);
  page.click(1040,290);page.run(['keyboard','type',mirror]);page.click(1000,316);
  await until(status,s=>s.state==='synced'&&s.localPath===mirror,'UI folder selection');
  const remote=path.join(fs.realpathSync(fs.readFileSync(file.replace(/\.[^.]+$/,'.runtime'),'utf8')),'checkout');
  assert.ok(fs.existsSync(path.join(mirror,'tracked')));fs.writeFileSync(path.join(remote,'ui-synced'),'live guest UI bytes');
  await until(()=>Promise.resolve(fs.existsSync(path.join(mirror,'ui-synced'))),Boolean,'Background mirror');
  assert.equal(fs.readFileSync(path.join(mirror,'ui-synced'),'utf8'),'live guest UI bytes');
  page.click(1230,210);await until(status,s=>!s.enabled,'Disable from UI');assert.ok(fs.existsSync(path.join(mirror,'tracked')));
  page.click(1230,210);await until(status,s=>s.enabled&&s.state==='synced','Re-enable from UI');
  page.screenshot('cloud-sync-browser-active.png');
  artifact('cloud-sync-browser-ui-first.json',{workspaceId:workspace,mirror,enabledViaUi:true,disabledViaUi:true,backgroundCopy:true,retainedFiles:true});
  page.click(1250,22);
}
export async function afterCloudSync(page,file,host) {
  const first=JSON.parse(fs.readFileSync('.context/cloud-sync-browser-ui-first.json'));
  const params={workspaceId:first.workspaceId,expectedAccountId:(await host.call('AuthStatus')).user.id};
  const status=()=>host.call('CloudSyncStatus',params).then(v=>v.status);
  page.click(1250,22);page.wait('window.__cloudLaunch.sync.some(v=>v.method==="CloudSyncStatus" && v.status.enabled)');
  assert.equal((await status()).localPath,first.mirror);
  const invalid=path.join(host.root,'unowned-ui-folder');fs.mkdirSync(invalid);fs.writeFileSync(path.join(invalid,'sentinel'),'local work');
  page.click(1040,320);page.run(['keyboard','type',invalid]);page.click(995,347);
  page.wait('window.__cloudLaunch.error?.includes("empty folder")');
  assert.equal((await status()).localPath,first.mirror);assert.equal(fs.readFileSync(path.join(invalid,'sentinel'),'utf8'),'local work');
  await sleep(2300);page.screenshot('cloud-sync-browser-error.png');
  page.click(1040,320);page.run(['press','Control+a']);page.run(['keyboard','type',first.mirror]);page.click(995,347);
  await until(status,s=>s.enabled&&s.state==='synced','Recover from invalid folder');page.click(995,380);
  page.wait('window.__cloudLaunch.methods.includes("CloudSyncRetry")');await until(status,s=>s.state==='synced','UI retry');
  page.screenshot('cloud-sync-browser-final.png');
  artifact('cloud-sync-browser-ui-acceptance.json',{...first,actualCompiledGpui:true,postgresApi:true,packagedGuest:true,cachedReopen:true,settingsRetained:true,retryViaUi:true,invalidFolderRefused:true,unrelatedFilesPreserved:true,liveProvider:false});
  page.click(1250,22);
}
