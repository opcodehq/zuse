// Compiled cloud settings/account lifecycle, with isolated account and cloud data.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,child,url,stage='startup';
const status=()=>fetch(url+'/__fixture').then(r=>r.json());
const update=value=>fetch(url+'/__fixture',{method:'POST',body:JSON.stringify(value)});
async function until(test,label){for(let n=0;n<60;n++){const value=await status();if(test(value))return value;await sleep(75);}throw Error(label);}
const click=async(x,y)=>{page.click(x,y);await sleep(200);};
const user=id=>({user:{id,name:id==='avery'?'Avery Parker':'Other person',email:id+'@example.test'},login:null,error:null});
try{
 host=await engine('cloud-settings-account');const ready=path.join(host.root,'fixture.ready');
 child=spawn('bun',[fixture('general-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(child.exitCode,null);await sleep(100);}
 assert.ok(fs.existsSync(ready));url=fs.readFileSync(ready,'utf8');await update({cloud:true,hold:true});
 page=browser('cloud-settings-account');page.open(url,fixture('general-settings-observer.js'));
 page.wait('window.__generalParity.methods.includes("CloudAccountStatus")');await click(210,978);await click(70,212);
 stage='loading';page.screenshot('cloud-account-loading.png');
 assert.equal((await status()).events.filter(e=>e.method?.startsWith('CloudBilling')).length,0);
 await update({hold:false});await sleep(150);stage='signed-out';page.screenshot('cloud-account-signed-out.png');
 const gate=fs.readFileSync('.context/cloud-account-signed-out.png').toString('base64');
 const pixels=page.evaluate(`(async()=>{const i=new Image();i.src='data:image/png;base64,${gate}';await i.decode();const c=document.createElement('canvas');c.width=i.width;c.height=i.height;const x=c.getContext('2d');x.drawImage(i,0,0);return [[744,149],[744,224],[744,225],[359,190],[360,190],[1127,190],[1128,190],[1064,186]].map(([a,b])=>Array.from(x.getImageData(a,b,1,1).data));})()`);
 assert.ok(pixels[0][0]>=15&&pixels[1][0]>=15&&pixels[2][0]<15,'Cloud gate matches reference y=149 to 224.5: '+JSON.stringify(pixels.slice(0,3)));
 assert.ok(pixels[3][0]<15&&pixels[4][0]>=15&&pixels[5][0]>=15&&pixels[6][0]<15,'Cloud gate matches reference width 768');
 assert.ok(pixels[7][1]>pixels[7][0]+35&&pixels[7][1]>pixels[7][2]+80,'Cloud uses the reference primary sign-in action');

 // Cloud's action uses the same sign-in operation as General.
 await click(1080,188);await click(1080,188);
 await until(s=>s.state.login,'Cloud sign-in must publish progress');
 assert.equal((await status()).events.filter(e=>e.method==='CloudAccountSignIn').length,1);
 assert.equal(page.evaluate('window.__generalParity.openedUrls.length'),1);
 page.screenshot('cloud-account-sign-in-pending.png');
 await update({state:user('avery')});
 await until(s=>s.events.some(e=>e.method==='CloudProjects'&&e.expectedAccountId==='avery'),'Signed-in cloud repositories must bind to General account');
 await sleep(250);stage='signed-in';page.screenshot('cloud-account-signed-in-setup.png');
 await click(528,331);await sleep(200);page.screenshot('cloud-account-signed-in-usage.png');
 await click(416,331);page.run(['press','End']);await sleep(100);
 assert.ok((await status()).events.some(e=>e.method==='CloudBillingSummary'&&e.owner==='avery'));
 // Preserve the focused Usage tab and verify its roving keyboard focus in compact layout.
 page.run(['set','viewport','760','900']);await sleep(200);page.screenshot('cloud-account-compact-usage.png');
 const png=fs.readFileSync('.context/cloud-account-compact-usage.png').toString('base64');
 const focus=page.evaluate(`(async()=>{const i=new Image();i.src='data:image/png;base64,${png}';await i.decode();const c=document.createElement('canvas');c.width=i.width;c.height=i.height;const x=c.getContext('2d');x.drawImage(i,0,0);const p=x.getImageData(0,0,i.width,i.height).data;let min=i.width,max=-1;for(let y=310;y<350;y++)for(let col=50;col<300;col++){const n=(y*i.width+col)*4;if(p[n+1]>p[n]+35&&p[n+1]>p[n+2]+70){min=Math.min(min,col);max=Math.max(max,col);}}return {min,max};})()`);
 assert.ok(focus.min>=176&&focus.max<=288&&focus.max>focus.min,'End must focus the selected Usage tab');
 page.run(['set','viewport','1280','1000']);await sleep(200);
 // Hold requests for A, sign out, then deliver those old responses.
 const prior=(await status()).events.filter(e=>e.method==='CloudBillingSummary').length;
 await update({holdCloud:true});await click(416,331);await click(528,331);await sleep(100);
 const before=(await status()).events.filter(e=>e.method==='CloudBillingSummary').length;
 const held=(await status()).events.filter(e=>e.method==='CloudBillingSummary').at(-1).id;
 assert.ok(before>prior,'Usage navigation starts the held billing refresh');await update({state:{user:null,login:null,error:null}});await sleep(200);
 await update({holdCloud:false});await sleep(200);stage='late-responses';page.screenshot('cloud-account-signed-out-after-late-responses.png');
 const initial=fs.readFileSync('.context/cloud-account-signed-out.png').toString('base64');
 const retired=fs.readFileSync('.context/cloud-account-signed-out-after-late-responses.png').toString('base64');
 const difference=page.evaluate(`(async()=>{const read=async b=>{const i=new Image();i.src='data:image/png;base64,'+b;await i.decode();const c=document.createElement('canvas');c.width=i.width;c.height=i.height;const x=c.getContext('2d');x.drawImage(i,0,0);return x.getImageData(360,149,768,80).data;};const a=await read('${initial}'),b=await read('${retired}');let n=0;for(let i=0;i<a.length;i++)if(a[i]!==b[i])n++;return n;})()`);
 assert.equal(difference,0,'Late responses must leave the signed-out card unchanged');
 // Switching to B must recreate all owner-scoped requests with B's binding.
 await update({state:user('other')});await sleep(250);await click(416,331);
 await until(s=>s.events.some(e=>e.method==='CloudProjects'&&e.expectedAccountId==='other'),'Account replacement must bind fresh repository requests');
 page.screenshot('cloud-account-switched-setup.png');
 await click(70,60);await sleep(200);page.screenshot('cloud-account-general-same-account.png');
 const proof=await status();assert.equal(proof.events.filter(e=>e.blocked).length,0);
 assert.equal(proof.events.filter(e=>['CloudBillingPortal','CloudBillingCheckout','CloudPrepaidCheckout','CloudBillingSetCap'].includes(e.method)).length,0,'Account navigation does not request billing writes');
 assert.ok(proof.events.filter(e=>e.method?.startsWith('CloudBilling')||e.method==='CloudPrepaidBalance').every(e=>e.expectedAccountId===e.owner),'Billing requests pin the displayed account');
 assert.equal(page.run(['errors']).errors.length,0,'Cloud lifecycle must not produce browser runtime errors');
 assert.ok(proof.events.some(e=>e.cancel===held),'Account replacement cancels its outstanding billing request');
 artifact('cloud-account-acceptance.json',{compiledGpui:true,disposableRealEngine:true,isolatedAccountAndCloudProtocol:true,loadingAndSignedOut:true,referenceGateBounds:true,primaryAction:true,cloudSignIn:true,duplicateSignInBlocked:true,signedInSetupAndUsage:true,sharedGeneralIdentity:true,accountSwitchRebindsRepositories:true,lateResponsesRetired:true,cloudTabsKeyboardFocus:true,compactSettings:true,providerOrPaymentWrites:0,liveAccountAcceptance:false});
 console.log('Cloud account acceptance passed: sign-in, shared identity, Setup/Usage, account replacement, late-response retirement and compact keyboard focus; zero execution/payment writes.');
}catch(error){try{page?.screenshot('cloud-account-failure.png');artifact('cloud-account-failure.json',{stage,fixture:await status(),browser:page?.evaluate('window.__generalParity'),console:page?.run(['console']),errors:page?.run(['errors'])});}catch{}console.error('Cloud account acceptance failed at '+stage+': '+error.message);process.exitCode=1;}
finally{page?.close();child?.kill('SIGTERM');await host?.close();}
