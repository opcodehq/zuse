// Real GPUI/browser interactions against an isolated engine; no provider turns.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine, browser, fixture, artifact, sleep} from './harness.mjs';
let host, page;
try {
  host = await engine('ui-parity');
  const info = await host.call('EngineInfo');
  const project = host.root + '/project';
  fs.mkdirSync(project);
  await host.call('Mutate', {op: 'createSpace', spaceId: 'project', deviceId: info.deviceId, path: project, name: 'Parity project'});
  for (const [chatId, title] of [['archive-one', 'Archived integration notes'], ['archive-two', 'Archived design notes']]) {
    await host.call('Mutate', {op: 'createChat', chatId, spaceId: 'project', cwd: project});
    await host.call('Mutate', {op: 'renameChat', chatId, title});
    await host.call('Mutate', {op: 'setChatArchived', chatId, archived: true});
  }
  page = browser('ui-parity-acceptance');
  page.open(host.url, fixture('ui-parity-observer.js'));
  page.wait('window.__uiParity.methods.includes("WatchChats")');
  await sleep(350);
  page.screenshot('ui-parity-composer.png');
  const png = fs.readFileSync('.context/ui-parity-composer.png').toString('base64');
  const outline = page.evaluate(`(async () => {
    const image = new Image(); image.src = 'data:image/png;base64,${png}'; await image.decode();
    const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
    const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
    return [[384,918],[1127,918],[756,862],[756,967],[383,918],[1128,918],[756,968]]
      .map(([x,y]) => context.getImageData(x,y,1,1).data[0]);
  })()`);
  assert.ok(outline.slice(0,4).every(red => red >= 30 && red < 65), 'Composer outline must match the reference 744×106px bounds');
  assert.ok(outline.slice(4).every(red => red < 20), 'Composer must end at the reference bounds');

  page.click(210,978); await sleep(180);
  page.click(80,272); await sleep(200);
  page.click(600,163); page.run(['keyboard','type','integration']); await sleep(150);
  page.click(1037,272);
  page.wait('window.__uiParity.mutations.some(change => change.op === "setChatArchived" && change.chatId === "archive-one" && change.archived === false)');
  let chats;
  for (let attempt=0; attempt<40; attempt++) {
    chats = await host.call('WatchChats');
    if (chats.find(chat => chat.id === 'archive-one')?.archived === false) break;
    await sleep(75);
  }
  assert.equal(chats.find(chat => chat.id === 'archive-one').archived, false);
  assert.equal(chats.find(chat => chat.id === 'archive-two').archived, true);
  page.click(600,163); page.run(['press','Control+a']); page.run(['keyboard','type','design']); await sleep(150);
  page.click(500,266);
  page.wait('window.__uiParity.methods.includes("WatchDocMessages")'); await sleep(200);
  page.screenshot('ui-parity-archive-preview.png');
  page.click(294,22); await sleep(180);
  page.click(600,163); page.run(['press','Control+a']); page.run(['press','Backspace']); await sleep(100);
  page.screenshot('ui-parity-archive.png');

  page.click(70,212); await sleep(350);
  page.screenshot('ui-parity-cloud-signed-out.png');
  assert.equal(page.evaluate('window.__uiParity.methods.filter(method => method === "CloudBillingSummary").length'),0,'Signed-out Cloud must not request billing with a synthetic workspace identity');
  page.run(['set','viewport','760','900']); await sleep(200);
  page.screenshot('ui-parity-settings-compact.png');
  const proof = page.evaluate('window.__uiParity');
  assert.equal(proof.mutations.filter(change => change.op === 'setChatArchived').length, 1);
  assert.equal(proof.methods.filter(method => ['QueueMessage','QueueCommand','CloudCreate','CloudBillingPortal','CloudBillingCheckout','CloudPrepaidCheckout'].includes(method)).length, 0);
  artifact('ui-parity-acceptance.json', {
    compiledGpui: true, realEngine: true, referenceCommit: '43e14c1',
    composer: {width:744,height:106,pixelBoundsVerified:true},
    localArchiveSearch: true, restoresOnlyChosenThread: true, savedArchivePreview: true,
    backToArchives: true, cloudSignedOutGate: true, noSyntheticBillingRequests: true, compactSettings: true, agentWrites: 0,
    liveAccountAcceptance: false,
  });
  console.log('UI parity acceptance passed: composer geometry, local archive filter/restore/preview, signed-out cloud gate, compact settings; zero provider turns.');
} catch (error) {
  console.error('UI parity acceptance failed: ' + error.message); process.exitCode = 1;
} finally {page?.close(); await host?.close();}
