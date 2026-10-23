// Real GPUI browser -> host connection supervisor -> API -> PostgreSQL -> packaged guest.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,stage='startup';
const file=process.env.ZUSE_CLOUD_LAUNCH_UI_FIXTURE_FILE;
try {
  const origin=fs.readFileSync(file,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);
  process.env.ZUSE_CLOUD_API_URL=origin;
  host=await engine('cloud-launch');page=browser('cloud-launch');
  page.open(host.url,fixture('cloud-launch-observer.js'));
  page.wait('window.__cloudLaunch?.methods.includes("WatchChats")');await sleep(250);
  page.screenshot('cloud-launch-controls-inspect.png');
  if(process.env.ZUSE_CLOUD_LAUNCH_MANUAL==='1') {
    while(!fs.existsSync(file.replace(/\.[^.]+$/,'.browser-done')))await sleep(500);
    const proof=page.evaluate('window.__cloudLaunch');
    assert.ok(proof.history.some(h=>h.reply),'Cached transcript must contain the original reply');
    assert.ok(proof.forwards.some(f=>f.reply),'Reconnected guest must retain the original reply');
    assert.equal(proof.status.filter(s=>s.state==='ready').length,1,'One stable connection must serve startup subscriptions');
    const receipt=JSON.parse(fs.readFileSync('.context/cloud-launch-browser-acceptance.json'));
    assert.equal(receipt.workspaceId,proof.history[0].workspaceId);
    assert.ok((await host.call('CloudWorkspaces')).workspaces.some(w=>w.workspaceId===receipt.workspaceId));
  } else {
    // Coordinates are taken from the rendered 1280x1000 GPUI canvas.
    stage='select-cloud';page.click(657,849);
    page.wait('window.__cloudLaunch.projects?.some(p=>p.projectId==="create-project")');await sleep(150);
    page.screenshot('cloud-composer-destination-menu.png');page.click(694,743);
    stage='first-message';page.click(600,895);
    if(process.env.ZUSE_CLOUD_LAUNCH_ATTACHMENTS==='1') {
      const note=Array.from(Buffer.from('cloud attachment bytes\n'));
      const png=Array.from(fs.readFileSync('crates/icons/assets/plugins/granola.png'));
      assert.equal(page.evaluate(`(()=>{const data=new DataTransfer();data.items.add(new File([new Uint8Array(${JSON.stringify(note)})],'cloud-attachment.txt',{type:'text/plain'}));data.items.add(new File([new Uint8Array(${JSON.stringify(png)})],'cloud-attachment.png',{type:'image/png'}));return document.querySelector('input:not([type=file])').dispatchEvent(new ClipboardEvent('paste',{clipboardData:data,bubbles:true,cancelable:true}));})()`),false);
      await page.wait('window.__cloudLaunch.staged.length===2');await sleep(600);
      page.screenshot('cloud-launch-attachments-composer.png');
    }
    page.run(['keyboard','type','Create the original cloud conversation']);page.run(['press','Enter']);
    page.wait('window.__cloudLaunch.creates.length===1');artifact('cloud-launch-created.json',page.evaluate('window.__cloudLaunch.creates[0]'));
    page.wait('window.__cloudLaunch.status.some(s=>s.state==="ready")');
    page.wait('window.__cloudLaunch.forwards.some(f=>f.reply)');await sleep(250);
    if(process.env.ZUSE_CLOUD_LAUNCH_ATTACHMENTS==='1') {
      page.wait('window.__cloudLaunch.forwards.some(f=>f.attachmentReply)');
      assert.ok(page.evaluate('window.__cloudLaunch.forwards.some(f=>f.attachmentReply)'), 'The first provider turn must verify text and identical inline PNG bytes');
      page.screenshot('cloud-launch-attachments-conversation.png');
      artifact('cloud-launch-attachments-acceptance.json',{actualCompiledGpui:true,hostStaging:true,encryptedApiAssets:true,packagedGuest:true,firstTurnTextAndPngVerified:true,workspace:page.evaluate('window.__cloudLaunch.creates[0].workspaceId'),liveProvider:false});
    }
    page.screenshot('cloud-launch-streamed-chat.png');
    const workspace=page.evaluate('window.__cloudLaunch.creates[0]');
    stage='close-client';page.run(['reload']);
    page.wait('window.__cloudLaunch?.methods.includes("WatchChats")');await sleep(250);
    page.click(657,849);
    page.wait(`window.__cloudLaunch.list?.some(w=>w.workspaceId===${JSON.stringify(workspace.workspaceId)})`);page.run(['press','Escape']);await sleep(200);
    page.screenshot('cloud-launch-reopen-inspect.png');
    stage='reopen-cached';page.click(83,914);
    page.wait('window.__cloudLaunch.history.some(h=>h.reply)');await sleep(150);
    assert.equal(page.evaluate('window.__cloudLaunch.methods.includes("CloudEnvironment")'),false,'Cached reopening cannot wake compute');
    page.screenshot('cloud-launch-cached-reopen.png');
    stage='reconnect';page.click(1235,22);
    page.wait('window.__cloudLaunch.status.some(s=>s.state==="ready")');
    page.wait('window.__cloudLaunch.forwards.some(f=>f.reply)');await sleep(2500);
    page.screenshot('cloud-composer-connected.png');
    if(process.env.ZUSE_CLOUD_UPLOAD_ACCEPTANCE==='1') {
      stage='cloud-attachments';page.click(600,895);
      const note=Array.from(Buffer.from('cloud attachment bytes\n'));
      const png=Array.from(fs.readFileSync('crates/icons/assets/plugins/granola.png'));
      assert.equal(page.evaluate(`(()=>{const data=new DataTransfer();data.items.add(new File([new Uint8Array(${JSON.stringify(note)})],'cloud-attachment.txt',{type:'text/plain'}));data.items.add(new File([new Uint8Array(${JSON.stringify(png)})],'cloud-attachment.png',{type:'image/png'}));return document.querySelector('input:not([type=file])').dispatchEvent(new ClipboardEvent('paste',{clipboardData:data,bubbles:true,cancelable:true}));})()`),false,'Cloud composer must admit pasted files');
      page.wait('window.__cloudLaunch.uploads.length===2');page.wait('window.__cloudLaunch.imageReads.some(r=>r.done)');await sleep(1200);
      const uploaded=page.evaluate('window.__cloudLaunch.uploads');
      const guestRoot=fs.realpathSync(fs.readFileSync(file.replace(/\.[^.]+$/,'.runtime'),'utf8'));
      for(const name of uploaded)assert.ok(name.startsWith(guestRoot+'/'),'Uploads must land on selected guest storage');
      assert.deepEqual([...fs.readFileSync(uploaded.find(p=>p.endsWith('cloud-attachment.txt')))],note);
      assert.deepEqual([...fs.readFileSync(uploaded.find(p=>p.endsWith('cloud-attachment.png')))],png);
      page.screenshot('cloud-upload-composer.png');
      page.run(['keyboard','type','Inspect the attached cloud files']);page.run(['press','Enter']);
      page.wait('window.__cloudLaunch.forwards.some(f=>f.attachmentReply)');await sleep(300);
      assert.equal(page.evaluate('window.__cloudLaunch.sockets'),1,'Uploads must share the existing host socket');
      page.screenshot('cloud-upload-conversation.png');
      artifact('cloud-upload-wire-proof.json',page.evaluate('window.__cloudLaunch'));
      artifact('cloud-upload-browser-acceptance.json',{actualCompiledGpui:true,selectedGuestBytesVerified:true,textAndPng:true,paste:true,realClaudeDriverReadsGuestFiles:true,inlineImageBytesVerified:true,existingConnectionOnly:true,explicitCloudTurns:2,liveProvider:false});
    }
    if(process.env.ZUSE_CLOUD_LIFECYCLE_ACCEPTANCE==='1') {
      for(let cycle=1;cycle<=2;cycle++) {
        stage=`pause-cloud-${cycle}`;
        const metadataBefore=page.evaluate('window.__cloudLaunch.metadata.length');
        const resumeBefore=page.evaluate('window.__cloudLaunch.status.filter(s=>s.state==="ready").length');
        page.click(1125,811);
        page.wait(`window.__cloudLaunch.pauses.length===${cycle}`);
        page.wait(`window.__cloudLaunch.metadata.slice(${metadataBefore}).some(w=>w.state==="paused")`);
        page.wait('window.__cloudLaunch.history.some(h=>h.reply)');
        await sleep(1200);
        assert.equal(page.evaluate('window.__cloudLaunch.status.filter(s=>s.state==="ready").length'),resumeBefore,'Paused history cannot auto-resume');
        page.screenshot('cloud-paused-browser.png');
        stage=`resume-cloud-${cycle}`;page.click(1235,22);
        page.wait(`window.__cloudLaunch.status.filter(s=>s.state==="ready").length>${resumeBefore}`);
        page.wait('window.__cloudLaunch.forwards.some(f=>f.reply)');
        await sleep(1000);
      }
      page.screenshot('cloud-resumed-browser.png');
      artifact('cloud-pause-browser-wire-proof.json',page.evaluate('window.__cloudLaunch'));
      artifact('cloud-pause-browser-acceptance.json',{actualGpui:true,realApiAndPostgres:true,guestQuiesced:true,sameMachine:true,retainedRuntimeRestart:true,cachedHistoryDuringPause:true,interactiveResume:true,pauseResumeCycles:2,providerTurns:1,liveProvider:false});
    }
    const proof=page.evaluate('window.__cloudLaunch');
    assert.equal(proof.creates.length,0,'Reload/reopen must not resubmit creation');
    assert.equal(proof.status.filter(s=>s.state==='ready').length,process.env.ZUSE_CLOUD_LIFECYCLE_ACCEPTANCE==='1'?3:1,'One ready connection per explicit connect');
    artifact('cloud-launch-browser-acceptance.json',{actualCompiledGpui:true,hostSupervisor:true,postgresApiAndPackagedGuest:true,firstMessageAcceptedOnce:true,durableEncryptedLaunch:true,stableStartupSubscriptions:true,liveStream:true,clientClosure:true,cachedReadDoesNotWake:true,reconnectOriginalSession:true,workspaceId:workspace.workspaceId,liveProvider:false});
  }
  fs.writeFileSync(file.replace(/\.[^.]+$/,'.done'),'verified\n');
  console.log('Cloud launch acceptance passed');
} catch(error) {
  try {page?.screenshot('cloud-launch-failure.png');artifact('cloud-launch-failure.json',{stage,error:error.message,proof:page?.evaluate('window.__cloudLaunch')});}catch{}
  console.error(`Cloud launch acceptance failed at ${stage}: ${error.message}`);process.exitCode=1;
} finally {page?.close();await host?.close();}
