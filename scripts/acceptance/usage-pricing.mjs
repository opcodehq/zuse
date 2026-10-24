// Actual CLI files -> real engine pricing/cache/ledger -> compiled GPUI dashboard.
import fs from 'node:fs';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {spawn,execFileSync} from 'node:child_process';
import assert from 'node:assert/strict';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';
import {seedProjectArchives} from './fixtures/project-archive-data.mjs';
import {seedUsagePricing} from './fixtures/usage-pricing-data.mjs';
let host,page,proxy,url;
const root = path.resolve('.context','usage-pricing-runtime-' + randomUUID());
const env = seedUsagePricing(root);
const click = async(x,y) => { page.run(['batch','--bail',`mouse move ${x} ${y}`,'mouse down','mouse up','wait 250']); await sleep(450); };
const proof = () => fetch(url+'/__fixture').then(r=>r.json());
const expect = async predicate => { for(let n=0;n<60;n++) { const value=await proof(); if(predicate(value.events))return value.events; await sleep(100); } assert.fail('Real usage reply deadline'); };
const blue = file => { const pixels=execFileSync('convert',[file,'-crop','845x210+354+355','-depth','8','rgb:-']);let n=0;for(let i=0;i<pixels.length;i+=3)if(pixels[i+2]>pixels[i]+30&&pixels[i+2]>pixels[i+1]+20)n++;return n; };
try {
  host = await engine('usage-pricing',{profileRoot:root,env});
  await seedProjectArchives(host);
  const report = await host.call('UsageOverview',{timezone:'UTC',forceRefresh:true});
  assert.equal(report.sessionCount,10); assert.equal(report.summary.costStatus,'partial');
  assert.ok(Math.abs(report.summary.costUsd-10.18)<1e-9);
  const daily=report.groups.map(group=>group.costUsd).sort((a,b)=>a-b);
  assert.equal(daily.length,7);
  daily.forEach((cost,i)=>assert.ok(Math.abs(cost-(i===6?5.215:0.8275))<1e-9));
  const ranked = await host.call('UsageSessions',{sort:'cost',timezone:'UTC'});
  assert.equal(ranked.rows[0].key,'estimated-day-0'); assert.ok(Math.abs(ranked.rows[0].costUsd-4.965)<1e-9);
  assert.equal(ranked.rows.find(row=>row.key==='logged').costUsd,0.25);
  assert.equal(ranked.rows.find(row=>row.key==='free').costUsd,0);
  assert.equal(ranked.rows.find(row=>row.key==='unpriced').costUsd,null);
  const ready = path.join(root,'fixture.ready');
  proxy=spawn('bun',[fixture('usage-pricing.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
  for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
  page=browser('usage-pricing');page.open(url,fixture('project-archive-observer.js'));page.wait('window.__uiParity.methods.includes("WatchChats")');await sleep(1200);
  await click(95,184);await click(175,145);page.run(['batch','--bail','press ArrowDown','wait 180','press Enter','wait 400']);
  await expect(events=>events.some(e=>e.method==='UsageOverview'&&e.replyTo&&e.sessionCount===10));
  await sleep(800);page.screenshot('usage-pricing-real-report.png');
  assert.ok(blue('.context/usage-pricing-real-report.png')>1000,'Estimated costs paint the chart');
  const before=(await proof()).events.length;await click(1236,88);
  await expect(events=>events.slice(before).some(e=>e.method==='UsageOverview'&&e.replyTo&&Math.abs(e.summary.costUsd-10.18)<1e-9));
  const final=await proof();
  assert.ok(final.events.some(e=>e.method==='UsageOverview'&&e.params?.forceRefresh));
  assert.equal(final.events.filter(e=>e.blocked).length,0);
  const errorResult=page.run(['errors']), errors=Array.isArray(errorResult)?errorResult:errorResult.errors;
  assert.deepEqual(errors,[]);
  artifact('usage-pricing-acceptance.json',{compiledBrowser:true,realEngine:true,realImportedFiles:true,usageRepliesUnmodified:true,pinnedPriceSnapshot:true,partialCostUsd:report.summary.costUsd,fastSessionCostUsd:ranked.rows[0].costUsd,loggedCostPreserved:true,loggedZeroPreserved:true,unpricedRemainsUnknown:true,refresh:true,providerAllowancesSynthetic:true,events:final.events,errors});
  console.log('Real usage pricing acceptance passed.');
} catch(error) { console.error('Real usage pricing acceptance failed: '+error.message);try{page?.screenshot('usage-pricing-failure.png');artifact('usage-pricing-failure-wire.json',await proof());}catch{}process.exitCode=1; }
finally {page?.close();proxy?.kill('SIGTERM');await host?.close();}
