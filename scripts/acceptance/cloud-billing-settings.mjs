// Compiled Cloud settings against isolated account/billing data and a disposable engine.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,child,url,stage='startup';
const state=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
const update=value=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(value)});
const click=async(x,y)=>{page.click(x,y);await sleep(180);};
const press=key=>page.run(['press',key]);
const type=text=>page.run(['keyboard','type',text]);
const events=async method=>(await state()).events.filter(e=>e.method===method);
const opened=()=>page.evaluate('window.__generalParity.openedUrls');
async function until(test,label){for(let n=0;n<80;n++){const value=await state();if(test(value))return value;await sleep(75);}throw Error(label);}
async function replaceCap(value,y=489){await click(1010,y);press('Control+a');type(value);}
async function bind(id,billing={}){
 await update({billing,state:{user:{id,name:'Avery Parker',email:id+'@example.test'},login:null,error:null}});
 await until(s=>s.events.some(e=>e.method==='CloudBillingSummary'&&e.owner===id),'Fresh billing owner '+id);await sleep(220);
}
function pixels(file,points){
 const png=fs.readFileSync('.context/'+file).toString('base64');
 return page.evaluate(`(async()=>{const i=new Image();i.src='data:image/png;base64,${png}';await i.decode();const c=document.createElement('canvas');c.width=i.width;c.height=i.height;const x=c.getContext('2d');x.drawImage(i,0,0);return ${JSON.stringify(points)}.map(([a,b])=>Array.from(x.getImageData(a,b,1,1).data));})()`);
}
try{
 host=await engine('cloud-billing-settings');const ready=path.join(host.root,'fixture.ready');
 child=spawn('bun',[fixture('general-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(child.exitCode,null);await sleep(100);}
 assert.ok(fs.existsSync(ready));url=fs.readFileSync(ready,'utf8');
 await update({cloud:true,billing:{writeDelay:1200},state:{user:{id:'avery',name:'Avery Parker',email:'avery@example.test'},login:null,error:null}});
 page=browser('cloud-billing-settings');page.open(url,fixture('general-settings-observer.js'));
 page.wait('window.__generalParity.methods.includes("CloudAccountStatus")');await click(210,978);await click(70,212);
 await until(s=>s.events.some(e=>e.method==='CloudProjects'),'Cloud setup ready');await sleep(250);
 page.screenshot('cloud-layout-setup-dark.png');stage='usage';await click(528,331);await sleep(220);page.screenshot('cloud-layout-usage-dark.png');
 // The actual reference components measure Hosting 193–301, tabs 317–345,
 // Usage 361–579, Billing 595–857, activity 873–981 (last two allow 1px raster rounding).
 const bounds=pixels('cloud-layout-usage-dark.png',[[744,193],[744,300],[744,361],[744,578],[744,595],[744,855],[744,874],[744,979],[744,192],[744,302],[744,360],[744,580],[744,594],[744,858],[744,871],[744,982],[359,260],[360,260],[1127,260],[1128,260]]);
 assert.ok(bounds.slice(0,8).every(([r])=>r>=30&&r<70),'Cloud frames match reference vertical bounds');
 assert.ok(bounds.slice(8,16).every(([r])=>r<20),'Cloud frame gaps match reference');
 assert.ok(bounds[16][0]<20&&bounds[17][0]>=15&&bounds[18][0]>=15&&bounds[19][0]<20,'Hosting uses the 768px column');
 stage='subscription portal';await update({billing:{holdWrites:true}});const portalBefore=opened().length;page.click(1055,266);page.click(1055,266);
 await until(s=>s.events.filter(e=>e.method==='CloudBillingPortal').length===1,'One subscription portal request');await update({billing:{holdWrites:false}});page.wait('window.__generalParity.openedUrls.length===1');
 assert.deepEqual(opened().slice(portalBefore),['https://billing.example.test/portal']);
 stage='invalid cap';await replaceCap('-1');await click(1087,489);await sleep(150);
 assert.equal((await events('CloudBillingSetCap')).length,0,'Invalid cap never reaches billing');page.screenshot('cloud-layout-invalid-cap.png');
 stage='cap uncertain retry';await replaceCap('50',537);await update({billing:{failNextCap:true,holdWrites:true}});page.click(1087,537);page.click(1087,489);
 await until(s=>s.events.filter(e=>e.method==='CloudBillingSetCap').length===1,'One cap request while busy');await update({billing:{holdWrites:false}});await sleep(220);page.screenshot('cloud-layout-cap-retry.png');
 const first=(await events('CloudBillingSetCap'))[0];assert.equal(first.overageCapMicros,50000000);
 await click(1087,537);await until(s=>s.events.filter(e=>e.method==='CloudBillingSetCap').length===2,'Retry saved cap');await sleep(1300);
 const caps=await events('CloudBillingSetCap');assert.equal(caps[1].idempotencyKey,first.idempotencyKey,'Uncertain retry preserves the same request key');assert.equal((await state()).billing.capMicros,50000000);page.screenshot('cloud-layout-cap-saved.png');
 stage='prepaid keyboard and checkout';await update({billing:{holdWrites:true}});await click(478,720);press('End');await sleep(100);page.screenshot('cloud-layout-credit-keyboard.png');
 const beforeCredits=opened().length;page.click(1071,720);page.click(1071,720);
 await until(s=>s.events.filter(e=>e.method==='CloudPrepaidCheckout').length===1,'One prepaid checkout while busy');await update({billing:{holdWrites:false}});page.wait('window.__generalParity.openedUrls.length===2');
 const credits=await events('CloudPrepaidCheckout');assert.equal(credits[0].amountCents,10000,'End selects $100 before checkout');
 assert.deepEqual(opened().slice(beforeCredits),['https://billing.example.test/credits']);
 stage='refresh balance';const reads=(await events('CloudPrepaidBalance')).length,summaries=(await events('CloudBillingSummary')).length;
 await update({billing:{creditCents:6000}});await click(1058,658);await until(s=>s.events.filter(e=>e.method==='CloudPrepaidBalance').length===reads+1,'Refresh balance reads prepaid data');
 assert.equal((await events('CloudBillingSummary')).length,summaries,'Balance refresh preserves the cap draft and usage history');page.screenshot('cloud-layout-refreshed-credits.png');
 stage='light and compact';await click(70,60);await click(1036,378);await click(70,212);await click(528,331);await sleep(180);page.screenshot('cloud-layout-usage-light.png');
 page.run(['set','viewport','760','900']);await sleep(180);page.screenshot('cloud-layout-compact-light.png');page.run(['set','viewport','1280','1000']);await sleep(100);
 await click(70,60);await click(1087,378);await click(70,212);await click(528,331);
 stage='unavailable credits';await update({billing:{creditAvailable:false,debitCents:700}});await click(1058,658);await sleep(150);const purchases=(await events('CloudPrepaidCheckout')).length;await click(1071,748);assert.equal((await events('CloudPrepaidCheckout')).length,purchases);page.screenshot('cloud-layout-credits-unavailable.png');
 stage='billing hold';await bind('hold-account',{status:'billing-hold',usedMicros:75000000,overageMicros:50000000,capMicros:50000000,creditAvailable:true,debitCents:0});await click(528,331);await sleep(150);page.screenshot('cloud-layout-billing-hold.png');
 stage='subscription checkout';await bind('no-subscription',{subscribed:false,status:'active',usedMicros:0,overageMicros:0});page.screenshot('cloud-layout-unsubscribed.png');await update({billing:{holdWrites:true}});const checkoutBefore=opened().length;
 page.click(1032,266);page.click(1032,266);await until(s=>s.events.filter(e=>e.method==='CloudBillingCheckout').length===1,'One subscription checkout');await update({billing:{holdWrites:false}});page.wait('window.__generalParity.openedUrls.length===3');assert.deepEqual(opened().slice(checkoutBefore),['https://billing.example.test/subscription']);
 stage='subscription read retry';await bind('subscription-retry',{entitlementError:'Subscription service is unavailable. Retry to reload.'});page.screenshot('cloud-layout-subscription-error.png');
 await update({billing:{entitlementError:null,subscribed:true}});await click(1083,314);await sleep(180);await click(528,331);await sleep(180);page.screenshot('cloud-layout-subscription-recovered.png');
 stage='usage pagination';await bind('history-account',{usageCount:12,usedMicros:12000000,overageMicros:0});await click(528,331);await sleep(180);await click(417,599);await sleep(180);page.screenshot('cloud-layout-history-expanded.png');
 page.run(['mouse','move','1050','650']);page.run(['scroll','down','650']);await sleep(180);page.screenshot('cloud-layout-history-scrolled.png');
 // Roving keyboard focus reaches the history continuation even when it starts below the fold.
 press('Tab');press('Enter');await until(s=>s.events.some(e=>e.method==='CloudBillingUsage'&&e.owner==='history-account'&&e.cursor==='older'),'Load more retrieves the next page');await sleep(180);page.screenshot('cloud-layout-history-page-two.png');
 const proof=await state();assert.equal(proof.events.filter(e=>e.blocked).length,0);assert.ok(proof.events.filter(e=>e.method?.startsWith('Cloud')).every(e=>e.expectedAccountId===e.owner),'All billing and repository requests pin the displayed account');
 assert.equal(page.run(['errors']).errors.length,0);assert.ok(!page.evaluate('window.__generalParity.methods').some(m=>['CloudEnvironment','CloudCreate','QueueMessage','QueueCommand'].includes(m)));
 artifact('cloud-billing-settings-acceptance.json',{compiledGpui:true,disposableRealEngine:true,isolatedBillingProtocol:true,referenceFrameBounds:true,hostingAndUsageComposition:true,portalDuplicateGuard:true,invalidCapBlocked:true,uncertainCapRetryReusesKey:true,prepaidKeyboardSelection:true,checkoutDuplicateGuard:true,balanceRefreshIndependent:true,lightAndCompact:true,unavailableCredits:true,billingHold:true,subscriptionCheckout:true,subscriptionRetry:true,usagePagination:true,noComputeWake:true,livePaymentAcceptance:false});
 console.log('Cloud billing settings passed: reference bounds, portal and checkout, cap validation/retry, prepaid keyboard selection/refresh, billing states, light/compact and pagination.');
}catch(error){try{page?.screenshot('cloud-layout-failure.png');artifact('cloud-layout-failure.json',{stage,error:error.message,fixture:await state(),browser:page?.evaluate('window.__generalParity'),console:page?.run(['console']),errors:page?.run(['errors'])});}catch{}console.error('Cloud billing settings failed at '+stage+': '+error.message+(error.cause?' ('+String(error.cause.code??error.cause.message)+')':''));process.exitCode=1;}
finally{page?.close();child?.kill('SIGTERM');await host?.close();}
