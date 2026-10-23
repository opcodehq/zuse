// The destructive action is exercised only against the isolated image fixture.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page;
try {
 const origin=fs.readFileSync(process.env.ZUSE_CLOUD_IMAGES_UI_FIXTURE_FILE,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);process.env.ZUSE_CLOUD_API_URL=origin;
 host=await engine('cloud-image-delete');
 const scope={expectedAccountId:'dev-user',expectedOrganizationId:null};
 const initial=(await host.call('CloudImage',scope)).data;assert.equal(initial.storage.state,'retained');
 await assert.rejects(host.call('CloudImageDelete',{...scope,expectedAccountId:'foreign',snapshotId:initial.storage.snapshotId}),/account changed/);
 page=browser('cloud-image-delete');page.open(host.url,fixture('cloud-images-observer.js'));page.wait('window.__imageProof?.methods.includes("WatchChats")');
 artifact('cloud-image-delete-browser-ready.json',{session:page.session});
 // Inspected canvas controls exercise cancel, keyboard confirmation and cleanup polling.
 while(!fs.existsSync('.context/cloud-image-delete-browser.done'))await sleep(250);
 const proof=page.evaluate('window.__imageProof');assert.equal(proof.deleted.length,1);
 assert.equal(proof.deleted[0].request.snapshotId,initial.storage.snapshotId);
 assert.ok(proof.statuses.some(s=>s.storage?.state==='deleting'));
 assert.equal(proof.statuses.at(-1).storage,null);assert.ok(!proof.methods.includes('CloudEnvironment'));
 const after=(await host.call('CloudImage',scope)).data;assert.equal(after.generation,undefined);assert.equal(after.storage,undefined);
 await host.call('CloudImageDelete',{...scope,snapshotId:initial.storage.snapshotId});
 artifact('cloud-image-delete-browser-proof.json',proof);artifact('cloud-image-delete-browser-acceptance.json',{compiledGpui:true,actualHostApiPostgresWorker:true,scopeGuard:true,oneExplicitDeletion:true,deletingThenRemoved:true,idempotentRedelivery:true,noWorkspaceWake:true,liveBoxd:false});
 console.log('Image deletion browser acceptance passed');
}catch(error){console.error(error);process.exitCode=1;}finally{page?.close();await host?.close();}
