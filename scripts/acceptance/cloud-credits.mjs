// Compiled viewport -> host -> real API/PostgreSQL -> Stripe HTTP credit fixture.
import fs from 'node:fs';import assert from 'node:assert/strict';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page;const file=process.env.ZUSE_CLOUD_CREDITS_UI_FIXTURE_FILE;
try{
  const origin=fs.readFileSync(file,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);process.env.ZUSE_CLOUD_API_URL=origin;
  host=await engine('cloud-credits');page=browser('cloud-credits');page.open(host.url,fixture('cloud-credits-observer.js'));page.wait('window.__creditsProof?.methods.includes("WatchChats")');await sleep(250);fs.writeFileSync('.context/cloud-credits-browser.ready','ready\n');
  while(!fs.existsSync('.context/cloud-credits-browser.done'))await sleep(250);
  const proof=page.evaluate('window.__creditsProof');assert.equal(proof.checkouts.length,1);assert.equal(proof.checkouts[0].amountCents,5000);assert.equal(proof.checkouts[0].accountId,'dev-user');assert.deepEqual(proof.opened,['stripe.example.test']);assert.ok(proof.balances.some(b=>b.creditCents===1500));assert.ok(proof.balances.some(b=>b.creditCents===6500));assert.ok(!proof.methods.includes('CloudEnvironment'));
  artifact('cloud-credits-browser-acceptance.json',{compiledGpuiBrowser:true,actualHostApiPostgresAndProviderHttp:true,selectedAmountCents:5000,refreshedAfterVerifiedFunding:true,creditCents:6500,noComputeWake:true,liveStripe:false,proof});fs.writeFileSync(file.replace(/\.[^.]+$/,'.done'),'verified\n');console.log('Prepaid credit browser acceptance passed');
}catch(error){try{page?.screenshot('cloud-credits-browser-failure.png');}catch{}console.error(`Prepaid credit acceptance failed: ${error.message}`);process.exitCode=1;}
finally{page?.close();await host?.close();}
