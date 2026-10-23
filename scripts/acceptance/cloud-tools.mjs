// Real GPUI panels against the selected cloud guest, including client detachment.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {artifact,sleep} from './harness.mjs';
const output = 'window.__cloudLaunch.tools.filter(t=>t.method==="TerminalData").map(t=>atob(t.data)).join("")';
function checkout(file) {
  return path.join(fs.realpathSync(fs.readFileSync(file.replace(/\.[^.]+$/,'.runtime'),'utf8')),'checkout');
}
async function terminal(page) {
  page.click(1254,66);await sleep(250);page.click(1202,66);
  page.wait('window.__cloudLaunch.tools.some(t=>t.method==="ListTerminals")');await sleep(250);
}
async function command(page,text,expected) {
  page.click(1050,973);page.run(['keyboard','type',text]);page.click(1257,975);
  page.wait(`${output}.includes(${JSON.stringify(expected)})`);
}
export async function beforeCloudTools(page,file) {
  await terminal(page);
  page.wait('window.__cloudLaunch.tools.some(t=>t.method==="OpenTerminal")');
  await command(page,`printf 'cloud file from retained shell\\n' > cloudtool.txt; printf '%s\\n' "$$" > terminalpid; export ZUSE_TERMINAL_PROOF=retained; printf 'CLOUDTOOLS:%s:%s\\n' "$ZUSE_TERMINAL_PROOF" "$PWD"`,'CLOUDTOOLS:retained:');
  const root=checkout(file);
  assert.equal(fs.readFileSync(path.join(root,'cloudtool.txt'),'utf8'),'cloud file from retained shell\n');
  const proof=page.evaluate('window.__cloudLaunch');
  const session=proof.tools.find(t=>t.method==='OpenTerminal').result;
  assert.equal(fs.realpathSync(session.cwd),root,'The terminal must run in the selected guest checkout');
  page.click(995,66);page.wait('window.__cloudLaunch.tools.filter(t=>t.method==="ListWorkspaceDirectory").length>=2');await sleep(250);
  page.click(1040,146);page.wait('window.__cloudLaunch.tools.some(t=>t.method==="ReadWorkspaceFile")');await sleep(250);
  page.click(1050,136);page.run(['press','Control+a']);page.run(['keyboard','type','saved through cloud file panel\n']);page.run(['press','Control+s']);
  page.wait('window.__cloudLaunch.tools.some(t=>t.method==="WriteWorkspaceFile" && t.result.status==="written")');
  assert.equal(fs.readFileSync(path.join(root,'cloudtool.txt'),'utf8'),'saved through cloud file panel\n');
  page.click(998,66);page.wait('window.__cloudLaunch.tools.some(t=>t.method==="GetCheckoutDiff")');await sleep(250);
  assert.ok(page.evaluate('window.__cloudLaunch.tools.find(t=>t.method==="GetCheckoutDiff").result.files.some(f=>JSON.stringify(f).includes("cloudtool.txt"))'));
  page.screenshot('cloud-tools-git-browser.png');
  artifact('cloud-tools-before-reload.json',page.evaluate('window.__cloudLaunch'));
  artifact('cloud-tools-original-terminal.json',{session,pid:fs.readFileSync(path.join(root,'terminalpid'),'utf8').trim()});
}
export async function afterCloudTools(page,file) {
  await terminal(page);
  const original=JSON.parse(fs.readFileSync('.context/cloud-tools-original-terminal.json','utf8'));
  const proof=page.evaluate('window.__cloudLaunch');
  assert.deepEqual(proof.tools.find(t=>t.method==='ListTerminals').result,[original.session]);
  assert.equal(proof.tools.filter(t=>t.method==='OpenTerminal').length,0,'Reopening must adopt the server-owned shell');
  await command(page,`printf 'REOPEN:%s:%s\\n' "$ZUSE_TERMINAL_PROOF" "$$"; printf '%s\\n' "$$" >> terminalpid`,`REOPEN:retained:${original.pid}`);
  assert.deepEqual(fs.readFileSync(path.join(checkout(file),'terminalpid'),'utf8').trim().split('\n'),[original.pid,original.pid]);
  page.screenshot('cloud-tools-retained-browser.png');
  artifact('cloud-tools-after-reload.json',page.evaluate('window.__cloudLaunch'));
  artifact('cloud-tools-browser-acceptance.json',{actualGpui:true,guestCheckoutVerified:true,fileReadAndSave:true,gitDiff:true,terminalCatalog:true,originalTerminalId:original.session.id,sameShellPid:original.pid,clientClosure:true,noExtraShellOnReopen:true,liveProvider:false});
}
