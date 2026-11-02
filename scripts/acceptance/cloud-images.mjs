// Compiled shared GPUI panel -> host RPC -> Axum/PostgreSQL -> durable image worker.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page;
try{
  const origin=fs.readFileSync(process.env.ZUSE_CLOUD_IMAGES_UI_FIXTURE_FILE,'utf8').trim();
  assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);process.env.ZUSE_CLOUD_API_URL=origin;
  host=await engine('cloud-images');
  await assert.rejects(host.call('CloudImageBuild',{expectedAccountId:'another-account',expectedOrganizationId:null,mode:'rebuild',idempotencyKey:'must-not-start'}),/account changed/);
  await assert.rejects(host.call('CloudImage',{expectedAccountId:'dev-user',expectedOrganizationId:'another-org'}),/account changed/);
  page=browser('cloud-images');page.open(host.url,fixture('cloud-images-observer.js'));
  page.wait('window.__imageProof?.methods.includes("WatchChats")');
  artifact('cloud-images-browser-ready.json',{session:page.session});
  // Inspected canvas controls drive the actual build, rebuild and history interactions.
  while(!fs.existsSync('.context/cloud-images-browser.done'))await sleep(250);
  const proof=page.evaluate('window.__imageProof');
  assert.ok(proof.builds.some(b=>b.request.mode==='update'));
  assert.ok(proof.builds.some(b=>b.request.mode==='rebuild'));
  assert.ok(proof.statuses.some(s=>s.state==='building'));
  assert.ok(proof.statuses.some(s=>s.state==='ready'));
  assert.ok(proof.builds.every(b=>b.request.expectedAccountId==='dev-user'&&b.request.expectedOrganizationId===null));
  assert.equal(new Set(proof.builds.map(b=>b.request.idempotencyKey)).size,proof.builds.length,'One key per accepted user build');
  assert.ok(!proof.methods.includes('CloudEnvironment'),'Image administration cannot wake a workspace');
  const final=await host.call('CloudImage',{expectedAccountId:'dev-user',expectedOrganizationId:null});assert.equal(final.data.state,'ready');
  artifact('cloud-images-browser-proof.json',proof);
  artifact('cloud-images-browser-acceptance.json',{compiledGpui:true,sharedNativeBrowserPanel:true,actualHostApiPostgres:true,durableWorker:true,buildAndRebuild:true,polledProgress:true,scopeChangesRejected:true,noWorkspaceWake:true,liveBoxd:false});
  console.log('Cloud image browser acceptance passed');
}catch(error){console.error(`Cloud image browser acceptance failed: ${error.message}`);process.exitCode=1;}
finally{page?.close();await host?.close();}
