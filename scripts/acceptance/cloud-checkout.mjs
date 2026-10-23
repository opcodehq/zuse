// Compiled viewport -> host -> actual API/PostgreSQL -> Stripe HTTP fixture.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page;
const file=process.env.ZUSE_CLOUD_CHECKOUT_UI_FIXTURE_FILE;
try{
  const origin=fs.readFileSync(file,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);process.env.ZUSE_CLOUD_API_URL=origin;
  host=await engine('cloud-checkout');page=browser('cloud-checkout');page.open(host.url,fixture('cloud-checkout-observer.js'));
  page.wait('window.__checkoutProof?.methods.includes("WatchChats")');await sleep(250);fs.writeFileSync('.context/cloud-checkout-browser.ready','ready\n');
  while(!fs.existsSync('.context/cloud-checkout-browser.done'))await sleep(250);
  const proof=page.evaluate('window.__checkoutProof');
  assert.equal(proof.checkouts.length,1);assert.equal(proof.checkouts[0].accountId,'dev-user');assert.equal(proof.checkouts[0].providerHost,'stripe.example.test');assert.deepEqual(proof.opened,['stripe.example.test']);
  assert.ok(proof.entitlements.some(items=>items.some(ent=>ent.status==='active'&&ent.offerId==='cloud-workspace-standard-v1')),'UI must refresh the activated subscription');
  assert.ok(proof.methods.includes('CloudBillingEntitlements'));assert.ok(!proof.methods.includes('CloudEnvironment'),'Billing must not wake compute');
  artifact('cloud-checkout-browser-acceptance.json',{compiledGpuiBrowser:true,actualHostApiPostgresAndProviderHttp:true,checkoutOpened:true,refreshedAfterSignedActivation:true,noComputeWake:true,liveStripe:false,proof});
  fs.writeFileSync(file.replace(/\.[^.]+$/,'.done'),'verified\n');console.log('Cloud checkout browser acceptance passed');
}catch(error){try{page?.screenshot('cloud-checkout-browser-failure.png');}catch{}console.error(`Checkout acceptance failed: ${error.message}`);process.exitCode=1;}
finally{page?.close();await host?.close();}
