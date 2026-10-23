// Actual compiled browser -> host broker -> isolated API -> durable deletion worker.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,stage='startup';
const file=process.env.ZUSE_CLOUD_UI_FIXTURE_FILE;
try {
  const origin=fs.readFileSync(file,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);
  process.env.ZUSE_CLOUD_API_URL=origin;
  host=await engine('cloud-delete');page=browser('cloud-delete');
  page.open(host.url,fixture('cloud-delete-observer.js'));
  page.wait('window.__cloudArchiveProof?.methods.includes("WatchChats")');await sleep(200);
  page.click(263,964);await sleep(150);page.click(100,242);
  page.wait('window.__cloudArchiveProof?.list?.some(w=>w.workspaceId==="ws-bob"&&w.state==="archived")');await sleep(250);
  stage='confirmation';page.click(1074,228);await sleep(200);
  page.screenshot('cloud-delete-confirmation.png');
  assert.equal(page.evaluate('window.__cloudArchiveProof.deletions.length'),0);
  stage='cancel';page.run(['press','Escape']);await sleep(100);
  assert.equal(page.evaluate('window.__cloudArchiveProof.deletions.length'),0);
  stage='request';page.click(1074,228);page.run(['press','Tab']);page.run(['press','Enter']);
  page.wait('window.__cloudArchiveProof.deletions.length===1');
  page.wait('window.__cloudArchiveProof.list?.some(w=>w.workspaceId==="ws-bob"&&w.statusCode==="cloud_object_purge_failed")');await sleep(200);
  page.screenshot('cloud-delete-retry.png');
  assert.equal(page.evaluate('window.__cloudArchiveProof.list.find(w=>w.workspaceId==="ws-bob").state'),'deleting');
  stage='retry';fs.writeFileSync(file.replace(/\.[^.]+$/,'.retry'),'resume isolated object store\n');
  page.click(1074,228);
  page.wait('window.__cloudArchiveProof.deletions.length===2');
  page.wait('window.__cloudArchiveProof.list?.some(w=>w.workspaceId==="ws-bob"&&w.state==="deleted")');await sleep(250);
  page.screenshot('cloud-delete-completed.png');
  const proof=page.evaluate('window.__cloudArchiveProof');
  assert.equal(proof.deletions[0].state,'deleting');
  assert.equal(proof.deletions[0].deletion.operationId,proof.deletions[1].deletion.operationId);
  assert.ok(!proof.methods.includes('CloudUnarchive'));
  for(const method of ['QueueCommand','QueueMessage','CreateTerminal','CloudResume'])assert.ok(!proof.methods.includes(method));
  const receipt={compiledGpuiBrowser:true,actualHostAndApi:true,confirmationAndCancel:true,cleanupFailureVisible:true,
    retriesSameDeletion:true,confirmedTombstone:true,automaticExecutionCalls:0,liveProvider:false};
  artifact('cloud-delete-browser-acceptance.json',receipt);
  fs.writeFileSync(file.replace(/\.[^.]+$/,'.done'),'verified\n');console.log(JSON.stringify(receipt));
}catch(error){try{page?.screenshot('cloud-delete-failure.png');artifact('cloud-delete-failure.json',{stage,error:error.message,proof:page?.evaluate('window.__cloudArchiveProof')});}catch{}console.error(`Cloud delete acceptance failed at ${stage}: ${error.message}`);process.exitCode=1;}
finally{page?.close();await host?.close();}
