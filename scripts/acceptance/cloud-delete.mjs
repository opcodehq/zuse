// Actual compiled browser -> host broker -> isolated API -> durable deletion worker.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,mirror,stage='startup';
const file=process.env.ZUSE_CLOUD_UI_FIXTURE_FILE;
try {
  const origin=fs.readFileSync(file,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);
  process.env.ZUSE_CLOUD_API_URL=origin;
  if(process.env.ZUSE_CLOUD_MIRROR_ACCEPTANCE==='1') {
    const base=fs.mkdtempSync(path.join(process.cwd(),'.context/cloud-mirror-'));
    const root=path.join(base,'repo/feature/bob');fs.mkdirSync(root,{recursive:true});
    const entries=['clean','edited'].map(name=>{
      fs.writeFileSync(path.join(root,name),'cloud snapshot',{mode:0o644});
      return {path:name,mode:0o644,size:14,hash:createHash('sha256').update('cloud snapshot').digest('hex')};
    });
    fs.writeFileSync(path.join(root,'edited'),'newer local work');
    fs.writeFileSync(path.join(root,'.env'),'local secret fixture');
    fs.writeFileSync(path.join(root,'extra'),'unrelated local file');
    fs.mkdirSync(path.join(root,'.git'));fs.writeFileSync(path.join(root,'.git/HEAD'),'ref: refs/heads/local\n');
    const marker=path.join(root,'.zuse-sync.json');
    fs.writeFileSync(marker,JSON.stringify({version:1,workspaceId:'ws-bob',files:entries,pending:entries}));
    const cache=root+'.zuse-sync-cache';fs.mkdirSync(cache);fs.writeFileSync(path.join(cache,'.zuse-sync.json'),JSON.stringify({version:1,workspaceId:'ws-bob',files:[]}));
    fs.writeFileSync(path.join(cache,'received.ndjson'),'private download receipt');
    mirror={base,root,marker,cache,entries};process.env.ZUSE_CLOUD_MIRROR_ROOT=base;
  }
  host=await engine('cloud-delete');page=browser('cloud-delete');
  page.open(host.url,fixture('cloud-delete-observer.js'));
  page.wait('window.__cloudArchiveProof?.methods.includes("WatchChats")');await sleep(200);
  page.click(274,982);await sleep(150);page.click(100,242);
  page.wait('window.__cloudArchiveProof?.list?.some(w=>w.workspaceId==="ws-bob"&&w.state==="archived")');await sleep(250);
  if(mirror) {
    stage='local-cleanup-error';
    page.wait('window.__cloudArchiveProof.list?.some(w=>w.workspaceId==="ws-bob"&&w.localMirrorCleanup?.state==="error")');await sleep(200);
    assert.ok(fs.existsSync(path.join(mirror.root,'clean')));
    page.screenshot('cloud-mirror-cleanup-error.png');
    fs.writeFileSync(mirror.marker,JSON.stringify({version:1,workspaceId:'ws-bob',files:mirror.entries}));
    stage='local-cleanup-retry';page.click(1064,228);
    page.wait('window.__cloudArchiveProof.cleanups.length===1');
    page.wait('window.__cloudArchiveProof.list?.some(w=>w.workspaceId==="ws-bob"&&w.localMirrorCleanup?.state==="complete")');await sleep(200);
    assert.ok(!fs.existsSync(path.join(mirror.root,'clean')));
    assert.equal(fs.readFileSync(path.join(mirror.root,'edited'),'utf8'),'newer local work');
    assert.equal(fs.readFileSync(path.join(mirror.root,'.env'),'utf8'),'local secret fixture');
    assert.equal(fs.readFileSync(path.join(mirror.root,'extra'),'utf8'),'unrelated local file');
    assert.equal(fs.readFileSync(path.join(mirror.root,'.git/HEAD'),'utf8'),'ref: refs/heads/local\n');
    assert.ok(!fs.existsSync(mirror.cache));
    assert.deepEqual(JSON.parse(fs.readFileSync(mirror.marker)).files.map(f=>f.path),['edited']);
    page.screenshot('cloud-mirror-cleanup-completed.png');
  }
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
    retriesSameDeletion:true,confirmedTombstone:true,verifiedLocalMirrorCleanup:!!mirror,automaticExecutionCalls:0,liveProvider:false};
  artifact('cloud-delete-browser-acceptance.json',receipt);
  fs.writeFileSync(file.replace(/\.[^.]+$/,'.done'),'verified\n');console.log(JSON.stringify(receipt));
}catch(error){try{page?.screenshot('cloud-delete-failure.png');artifact('cloud-delete-failure.json',{stage,error:error.message,proof:page?.evaluate('window.__cloudArchiveProof')});}catch{}console.error(`Cloud delete acceptance failed at ${stage}: ${error.message}`);process.exitCode=1;}
finally{page?.close();await host?.close();}
