// Run against the optional isolated PostgreSQL/guest archive fixture in tests/postgres.rs.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,stage='startup';
try {
  const fixtureFile=process.env.ZUSE_CLOUD_UI_FIXTURE_FILE;
  assert.ok(fixtureFile,'Set ZUSE_CLOUD_UI_FIXTURE_FILE to the ready local archive fixture');
  const origin=fs.readFileSync(fixtureFile,'utf8').trim();
  assert.ok(/^http:\/\/127\.0\.0\.1:\d+$/.test(origin),'Acceptance only uses an isolated loopback API');
  process.env.ZUSE_CLOUD_API_URL=origin;
  host=await engine('cloud-archive');
  page=browser('cloud-archive');page.open(host.url,fixture('cloud-archive-observer.js'));
  page.wait('window.__cloudArchiveProof?.methods.includes("WatchChats")');
  await sleep(200);
  stage='archive-list';page.click(210,978);await sleep(150);page.click(100,242);
  page.wait('window.__cloudArchiveProof?.list?.some(w=>w.workspaceId==="ws-bob"&&w.state==="archived")');
  await sleep(150);
  page.screenshot('cloud-archived-list.png');
  stage='saved-history';page.click(520,223);
  page.wait('window.__cloudArchiveProof?.history?.fixtureReply===true');
  await sleep(200);
  const history=page.evaluate('window.__cloudArchiveProof.history');
  assert.equal(history.workspaceId,'ws-bob');assert.equal(history.sessionId,'session-bob');
  assert.equal(history.archived,true);assert.equal(history.paused,true);assert.ok(history.entries>0);
  page.screenshot('cloud-archived-history.png');
  stage='lazy-unarchive';page.click(1195,975);
  page.wait('window.__cloudArchiveProof?.unarchive?.state==="paused"');
  const proof=page.evaluate('window.__cloudArchiveProof');
  for(const method of ['QueueCommand','QueueMessage','CreateTerminal','OpenTerminal','CloudResume']) {
    assert.ok(!proof.methods.includes(method),`Archive review must not execute ${method}`);
  }
  const rows=(await host.call('CloudWorkspaces')).workspaces;
  const original=rows.find(w=>w.workspaceId==='ws-bob');
  assert.equal(original.state,'paused');assert.equal(original.desiredState,'paused');
  assert.equal(original.initialSessionId,'session-bob');
  const receipt={compiledGpuiBrowser:true,actualHostBroker:true,actualGuestArchive:true,
    encryptedCachedHistory:true,archivedReadOnly:true,unarchiveRemainsPaused:true,
    originalWorkspaceAndSession:true,automaticExecutionCalls:0,liveProvider:false};
  artifact('cloud-archive-browser-acceptance.json',receipt);
  const ready=path.parse(fixtureFile);
  fs.writeFileSync(path.join(ready.dir,ready.name+'.done'),'verified\n');
  console.log(JSON.stringify(receipt));
} catch(error) {
  try {page?.screenshot('cloud-archive-failure.png');artifact('cloud-archive-browser-failure.json',{stage,error:error.message,proof:page?.evaluate('window.__cloudArchiveProof')});}catch{}
  console.error(`Cloud archive acceptance failed at ${stage}: ${error.message}`);process.exitCode=1;
} finally {page?.close();await host?.close();}
