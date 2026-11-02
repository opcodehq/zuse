// Compiled GPUI interactions using an isolated plugin protocol and disposable engine.
// No live account, provider turn, external OAuth navigation, or checkout is involved.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,child,url;
async function proof(){return fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());}
async function until(check,label){for(let n=0;n<50;n++){const value=await proof();if(check(value))return value;await sleep(100);}throw Error(label);}
const click=async(x,y)=>{page.click(x,y);await sleep(150);};
const press=key=>page.run(['press',key]);
const type=value=>page.run(['keyboard','type',value]);
async function settings(){await click(210,978);await click(70,152);}
async function browse(){await click(1087,163);await click(990,199);}
try {
  host=await engine('plugin-settings');
  const ready=path.join(host.root,'fixture.ready');
  child=spawn('bun',[fixture('plugin-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:['ignore','pipe','pipe']});
  let output='';child.stdout.on('data',buffer=>output+=buffer);child.stderr.on('data',buffer=>output+=buffer);
  for(let n=0;n<100&&!fs.existsSync(ready);n++){if(child.exitCode!==null)throw Error(output);await sleep(100);}
  assert.ok(fs.existsSync(ready),'Plugin fixture startup');url=fs.readFileSync(ready,'utf8');
  page=browser('plugin-settings');page.open(url,fixture('ui-parity-observer.js'));
  page.wait('window.__uiParity.methods.includes("Plugins")');await sleep(150);await settings();
  page.screenshot('plugin-parity-settings.png');
  await click(1094,257);
  await until(p=>p.snapshot.connections.find(c=>c.id==='figma-one')?.enabled===false,'Toggle the selected connection');
  await click(930,163);type('Workspace');await sleep(100);page.screenshot('plugin-parity-settings-filter.png');
  await click(1087,163);page.screenshot('plugin-parity-add-menu.png');await click(990,227);
  press('Enter');await sleep(100);page.screenshot('plugin-parity-mcp-validation.png');
  press('Escape');await sleep(150);
  assert.equal((await proof()).events.filter(e=>e.action==='addServer').length,0,'Invalid/canceled setup must not write');
  await browse();page.screenshot('plugin-parity-browse.png');
  await click(518,385);page.screenshot('plugin-parity-detail.png');
  // The disabled existing account still reserves the default name Figma.
  await click(548,394);
  await until(p=>p.events.some(e=>e.action==='connect'&&e.pluginId==='figma'&&e.label==='Figma 2'),'Additional connection label');
  page.screenshot('plugin-parity-pending.png');
  // Catalog coordinates follow the framed main pane at 1280×1000.
  // They exercise the canvas controls rather than mutating application state from JavaScript.
  await until(p=>p.events.some(e=>e.action==='poll'),'Automatically poll pending connection');
  await fetch(url+'/__fixture',{method:'POST',body:JSON.stringify({approve:true})});
  await until(p=>p.snapshot.connections.some(c=>c.label==='Figma 2'),'Complete additional connection');
  page.screenshot('plugin-parity-multiple.png');
  await click(1081,400);page.screenshot('plugin-parity-account-menu.png');
  assert.equal((await proof()).events.filter(e=>e.action==='disconnect').length,0,'Opening the account menu must not disconnect');
  press('End');press('Enter');
  await until(p=>!p.snapshot.connections.some(c=>c.label==='Figma 2'),'Disconnect only the selected account');
  assert.equal((await proof()).snapshot.connections.find(c=>c.id==='figma-one').enabled,false);
  await click(431,84);await click(1086,475);
  page.screenshot('plugin-parity-api-key-inline.png');
  type('fixture-plugin-key');page.screenshot('plugin-parity-api-key-masked.png');press('Escape');await sleep(800);page.screenshot('plugin-parity-api-key-cancelled.png');
  assert.equal((await proof()).events.filter(e=>e.action==='connect'&&e.pluginId==='metrics').length,0,'Escape discards the inline secret');
  press('Enter');await sleep(800);page.screenshot('plugin-parity-api-key-reopened.png');type('fixture-plugin-key');press('Enter');
  await until(p=>p.snapshot.connections.some(c=>c.pluginId==='metrics'),'API key connection');
  assert.equal((await proof()).events.filter(e=>e.action==='connect'&&e.pluginId==='metrics').length,1,'Enter submits once');
  await click(431,84);await click(692,150);page.screenshot('plugin-parity-mcp-modal.png');
  const png=fs.readFileSync('.context/plugin-parity-mcp-modal.png').toString('base64');
  const labels=page.evaluate(`(async()=>{
    const image=new Image();image.src='data:image/png;base64,${png}';await image.decode();
    const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
    const context=canvas.getContext('2d');context.drawImage(image,0,0);
    return [[455,550],[590,682],[727,815]].map(([left,right])=>{
      let count=0;for(let y=398;y<411;y++)for(let x=left;x<right;x++){
        const [r,g,b]=context.getImageData(x,y,1,1).data;if(r>85&&g>85&&b>85)count++;
      }return count;
    });
  })()`);
  assert.ok(labels.every(count=>count>20),'All authentication tab labels must be visible');
  // Shift+Tab at the first field wraps to the footer; Tab returns to the first field.
  press('Shift+Tab');press('Tab');type('Parity tools');press('Tab');type('https://parity.example.test/mcp');
  press('Tab');press('End');press('Tab');type('X-Parity-Key');press('Tab');type('fixture-plugin-key');
  page.screenshot('plugin-parity-mcp-filled.png');press('Enter');
  await until(p=>p.events.some(e=>e.action==='addServer'),'Submit remote MCP server');
  const added=(await proof()).events.filter(e=>e.action==='addServer');
  assert.equal(added.length,1);assert.equal(added[0].name,'Parity tools');
  assert.equal(added[0].url,'https://parity.example.test/mcp');assert.equal(added[0].apiKeyHeader,'X-Parity-Key');
  await fetch(url+'/__fixture',{method:'POST',body:JSON.stringify({approve:true})});
  await until(p=>p.snapshot.connections.some(c=>c.label==='Parity tools'),'Complete custom server connection');
  page.screenshot('plugin-parity-server-added.png');
  // Open an ephemeral credential form, then replace its owning account.
  await click(692,150);type('Discarded setup');await sleep(100);
  const before=(await proof()).events.filter(e=>e.action!=='list').length;
  const listsBefore=(await proof()).events.filter(e=>e.action==='list').length;
  await fetch(url+'/__fixture',{method:'POST',body:JSON.stringify({account:'other-plugin-owner',canManage:false})});
  await sleep(250);press('Enter');await sleep(100);
  await until(p=>p.events.filter(e=>e.action==='list').length>listsBefore,'Refresh the new account');
  await click(1086,475);await sleep(100);
  assert.equal((await proof()).events.filter(e=>e.action!=='list').length,before,'Account change discards the form; read-only catalog cannot connect');
  assert.equal((await proof()).snapshot.connections.length,0,'Prior account connections are cleared');
  page.screenshot('plugin-parity-read-only.png');
  await fetch(url+'/__fixture',{method:'POST',body:JSON.stringify({account:null})});await sleep(150);
  page.screenshot('plugin-parity-signed-out.png');
  const result=await proof();assert.equal(result.events.filter(e=>e.blocked).length,0);
  const methods=page.evaluate('window.__uiParity.methods');
  assert.equal(methods.filter(m=>['QueueMessage','QueueCommand','CloudCreate','CloudBillingCheckout','CloudBillingPortal','CloudPrepaidCheckout'].includes(m)).length,0);
  assert.equal(page.run(['errors']).errors.length,0,'No browser runtime errors');
  artifact('plugin-parity-acceptance.json',{
    compiledGpui:true,disposableRealEngine:true,pluginProtocolFixture:true,liveAccountAcceptance:false,
    referenceCommit:'43e14c1',installedFilter:true,selectedToggle:true,separateCatalog:true,
    additionalAccountLabel:'Figma 2',connectPollReconciliation:true,accountMenuKeyboard:true,selectedDisconnect:true,automaticPolling:true,inlineKeyEscapeRestoresTrigger:true,inlineReopenTextEntryWithoutFieldClick:true,
    inlineApiKey:true,apiKeyEnterSubmitsOnce:true,modalEscapeCancels:true,modalTabWrap:true,authenticationTabLabelsVisible:true,
    authenticationEndKey:true,customServerAdded:true,accountChangeDiscardsForm:true,readOnlyBlocksWrites:true,
    agentWrites:0,pluginActions:result.events.filter(e=>e.action!=='list'),
  });
  console.log('Plugin UI acceptance passed: settings/catalog/details, toggle, additional account, keyboard account menu, inline API key, keyboard MCP setup, account change and read-only controls; zero provider turns.');
} catch(error) {
  try {artifact('plugin-inline-browser-failure-proof.json',await proof());page?.screenshot('plugin-parity-failure.png');}catch{}
  console.error('Plugin UI acceptance failed: '+error.message);process.exitCode=1;
} finally {
  page?.close();
  if(child&&child.exitCode===null&&child.signalCode===null){child.kill('SIGTERM');for(let n=0;n<50&&child.exitCode===null&&child.signalCode===null;n++)await sleep(50);}
  await host?.close();
}
