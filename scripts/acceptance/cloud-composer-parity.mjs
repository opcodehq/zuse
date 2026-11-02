// Compiled GPUI -> isolated API/PostgreSQL -> packaged guest. The fixture must
// enable ZUSE_CLOUD_LIFECYCLE_ACCEPTANCE=1 and ZUSE_CLOUD_UPLOAD_ACCEPTANCE=1
// (the latter expects the two explicitly submitted provider turns in this flow).
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {engine, browser, fixture, artifact, sleep} from './harness.mjs';
const file=process.env.ZUSE_CLOUD_LAUNCH_UI_FIXTURE_FILE;
assert.ok(file,'Set the isolated cloud launch fixture ready file');
process.env.ZUSE_CLOUD_API_URL=fs.readFileSync(file,'utf8').trim();
let host,page;
try {
  host=await engine('cloud-composer-parity');page=browser('cloud-composer-parity');
  page.open(host.url,fixture('cloud-launch-observer.js'));
  page.wait('window.__cloudLaunch?.methods.includes("WatchChats")');
  page.click(640,849);page.wait('window.__cloudLaunch.projects?.some(p=>p.projectId==="create-project")');
  page.click(690,756);page.click(600,895);
  page.run(['keyboard','type','Create the original cloud conversation']);page.run(['press','Enter']);
  page.wait('window.__cloudLaunch.creates.length===1');
  page.wait('window.__cloudLaunch.forwards.some(f=>f.reply)');await sleep(600);
  page.screenshot('cloud-composer-parity-running.png');
  const pause=async cycle=>{
    const metadata=page.evaluate('window.__cloudLaunch.metadata.length');
    page.click(1222,18);await sleep(200);page.click(1000,359);
    page.wait(`window.__cloudLaunch.pauses.length===${cycle}`);
    page.wait(`window.__cloudLaunch.metadata.slice(${metadata}).some(w=>w.state==="paused")`);
    page.wait('window.__cloudLaunch.history.some(h=>h.reply)');
    const connections=page.evaluate('window.__cloudLaunch.methods.filter(m=>m==="CloudEnvironment").length');
    await sleep(600);
    assert.equal(page.evaluate('window.__cloudLaunch.methods.filter(m=>m==="CloudEnvironment").length'),connections,'Reading paused history must not wake compute');
  };
  await pause(1);page.screenshot('cloud-composer-parity-paused.png');
  page.click(1162,850);page.wait('window.__cloudLaunch.status.filter(s=>s.state==="ready").length===2');
  await sleep(500);await pause(2);
  page.click(600,895);page.run(['keyboard','type','Continue the original conversation after pause']);
  assert.equal(page.evaluate('window.__cloudLaunch.status.filter(s=>s.state==="ready").length'),2,'Editing a draft must not resume compute');
  page.click(1214,965);page.wait('window.__cloudLaunch.status.filter(s=>s.state==="ready").length===3');
  const root=fs.readFileSync(file.replace(/\.[^.]+$/,'.runtime'),'utf8').trim();
  for(let n=0;n<100;n++){
    if(fs.readFileSync(root+'/provider-observed','utf8').trim().split('\n').length===2)break;
    await sleep(100);
  }
  assert.equal(fs.readFileSync(root+'/provider-observed','utf8').trim().split('\n').length,2,'Paused submission must execute exactly once');
  page.screenshot('cloud-composer-parity-resumed.png');
  const proof=page.evaluate('window.__cloudLaunch');assert.equal(proof.creates.length,1);
  artifact('cloud-composer-parity-acceptance.json',{compiledGpui:true,menuPause:true,pausedHistoryDoesNotWake:true,pausedComposerPreserved:true,explicitResume:true,pausedSubmit:true,providerTurns:2,workspaces:1,liveProvider:false});
  artifact('cloud-composer-parity-wire-proof.json',proof);
  fs.writeFileSync(file.replace(/\.[^.]+$/,'.done'),'Cloud composer parity acceptance passed\n');
  console.log('Cloud composer parity acceptance passed');
} finally {page?.close();await host?.close();}
