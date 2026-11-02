// Actual compiled viewport -> host authority -> Axum/PostgreSQL -> selected provider adapter.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page;
const file=process.env.ZUSE_CLOUD_BILLING_UI_FIXTURE_FILE;
try {
  const origin=fs.readFileSync(file,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);
  process.env.ZUSE_CLOUD_API_URL=origin;
  host=await engine('cloud-portal');page=browser('cloud-portal');
  page.open(host.url,fixture('cloud-portal-observer.js'));
  page.wait('window.__portalProof?.methods.includes("WatchChats")');await sleep(250);
  fs.writeFileSync('.context/cloud-portal-browser.ready','ready\n');
  while(!fs.existsSync('.context/cloud-portal-browser.done'))await sleep(250);
  const proof=page.evaluate('window.__portalProof');
  assert.equal(proof.portals.length,1,'One Manage billing click must create one portal session');
  assert.equal(proof.portals[0].accountId,'dev-user');
  assert.equal(proof.portals[0].providerHost,'stripe.example.test');
  assert.deepEqual(proof.opened,['stripe.example.test']);
  assert.ok(!proof.methods.includes('CloudEnvironment'),'Billing cannot wake compute');
  artifact('cloud-portal-browser-acceptance.json',{compiledGpuiBrowser:true,actualHostApiAndPostgres:true,selectedExistingProvider:true,portalOpened:true,noComputeWake:true,liveStripe:false,proof});
  fs.writeFileSync(file.replace(/\.[^.]+$/,'.done'),'verified\n');
  console.log('Cloud portal browser acceptance passed');
}catch(error){try{page?.screenshot('cloud-portal-browser-failure.png');}catch{}console.error(`Cloud portal acceptance failed: ${error.message}`);process.exitCode=1;}
finally{page?.close();await host?.close();}
