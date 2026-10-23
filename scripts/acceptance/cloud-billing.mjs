// Compiled GPUI browser -> host account authority -> actual Axum/PostgreSQL billing.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page;
const file=process.env.ZUSE_CLOUD_BILLING_UI_FIXTURE_FILE;
try {
  const origin=fs.readFileSync(file,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);
  process.env.ZUSE_CLOUD_API_URL=origin;
  host=await engine('cloud-billing');page=browser('cloud-billing');
  page.open(host.url,fixture('cloud-billing-observer.js'));
  page.wait('window.__billingProof?.methods.includes("WatchChats")');await sleep(250);
  page.screenshot('cloud-billing-browser-start.png');
  // The manual breakpoint permits inspected canvas coordinates; wire results verify real actions.
  fs.writeFileSync('.context/cloud-billing-browser.ready','ready\n');
  while(!fs.existsSync('.context/cloud-billing-browser.done'))await sleep(250);
  const proof=page.evaluate('window.__billingProof');
  assert.ok(proof.summaries.some(s=>s.providerCostMicros===42000000&&s.overageChargeMicros===7350000));
  assert.ok(proof.pages.length>=2,'Usage must paginate through the visible controls');
  assert.ok(proof.pages[0].nextCursor,'Initial usage page must expose pagination');
  assert.ok(proof.caps.some(c=>c.request.overageCapMicros===31000000&&c.result.overageCapMicros===31000000));
  assert.ok(proof.errors.some(e=>e.method==='CloudBillingSetCap'&&e.error.includes('already incurred')),'A rejected cap must be visible and actionable');
  assert.ok(!proof.methods.includes('CloudEnvironment'),'Billing reads and cap updates cannot wake compute');
  const summary=await host.call('CloudBillingSummary');assert.equal(summary.data.overageCapMicros,31000000);
  artifact('cloud-billing-browser-acceptance.json',{compiledGpuiBrowser:true,actualHostApiAndPostgres:true,ledgerReservationsAndStorage:true,usagePagination:true,savedCapMicros:31000000,rejectedBelowIncurredCap:true,noComputeWake:true,liveStripe:false});
  artifact('cloud-billing-browser-wire-proof.json',proof);
  fs.writeFileSync(file.replace(/\.[^.]+$/,'.done'),'verified\n');
  console.log('Cloud billing browser acceptance passed');
}catch(error){try{page?.screenshot('cloud-billing-browser-failure.png');artifact('cloud-billing-browser-failure.json',{error:error.message,proof:page?.evaluate('window.__billingProof')});}catch{}console.error(`Cloud billing acceptance failed: ${error.message}`);process.exitCode=1;}
finally{page?.close();await host?.close();}
