// Shared dashboard geometry and navigation against compiled GPUI with isolated report DTOs.
import fs from 'node:fs';import path from 'node:path';import {spawn,execFileSync} from 'node:child_process';import assert from 'node:assert/strict';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';import {seedProjectArchives} from './fixtures/project-archive-data.mjs';
let host,page,proxy,url,cost,token;
const phase=process.argv.find(arg=>arg.startsWith('--phase='))?.slice(8)??'all';assert.ok(['all','interactions','lifecycle','sessions'].includes(phase));const interactions=phase==='all'||phase==='interactions',lifecycle=phase==='all'||phase==='lifecycle';
// Browser commands can block past keep-alive expiry; use fresh fixture-control sockets.
const proof=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
const configure=patch=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(patch)});
const calls=async method=>(await proof()).events.filter(e=>e.method===method);
const last=async method=>(await calls(method)).at(-1);
const click=async(x,y)=>{page.run(['batch','--bail',`mouse move ${x} ${y}`,'mouse down','mouse up','wait 200']);await sleep(350);};
const press=async key=>{page.run(['press',key]);await sleep(300);};
const shot=name=>page.screenshot('usage-details-'+name+'.png');
const pixels=(name,region)=>execFileSync('convert',['.context/usage-details-'+name+'.png','-crop',region,'-depth','8','rgb:-'],{maxBuffer:5*1024*1024});
const waitFor=async predicate=>{for(let i=0;i<80;i++){if(await predicate())return;await sleep(100);}assert.fail('Usage interaction deadline');};
const bottom=()=>click(1272,980);
const frames=()=>{shot('geometry');const p=pixels('geometry','1x892+261+104'),rows=[];const same=i=>p[i*3]===34&&p[i*3+1]===37&&p[i*3+2]===35;for(let i=0;i<p.length/3;i++){if(!same(i))continue;let end=i;while(end+1<p.length/3&&same(end+1))end++;if(end-i>20)rows.push({top:104+i-7,bottom:104+end+8});i=end;}return rows;};
const tooltip=name=>{const p=pixels(name,'1000x500+260+170');let left=1000,right=0,top=500,bottom=0,n=0;for(let y=0;y<500;y++)for(let x=0;x<1000;x++){const i=(y*1000+x)*3;if(p[i]===25&&p[i+1]===28&&p[i+2]===26){left=Math.min(left,x);right=Math.max(right,x);top=Math.min(top,y);bottom=Math.max(bottom,y);n++;}}assert.ok(n>1000,'The tooltip paints its measured popover');return{x:left+259,y:top+169,width:right-left+3,height:bottom-top+3};};
const blue=name=>{const p=pixels(name,'845x210+354+355');let n=0;for(let i=0;i<p.length;i+=3)if(p[i+2]>p[i]+30&&p[i+2]>p[i+1]+20)n++;return n;};
try{
 host=await engine('usage-details');await seedProjectArchives(host);const ready=path.join(host.root,'fixture.ready');
 proxy=spawn('bun',[fixture('usage-dashboard.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let i=0;i<100&&!fs.existsSync(ready);i++){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
 page=browser('usage-details-'+phase);page.open(url,fixture('project-archive-observer.js'));page.wait('window.__uiParity.methods.includes("WatchChats")');await sleep(1000);
 if(interactions){
 await click(95,184);await click(600,912);page.run(['keyboard','type','Keep this draft through usage settings.']);await click(900,500);shot('draft-before');const draft=pixels('draft-before','600x38+324+890');
 await click(175,145);await press('ArrowDown');await press('Enter');await waitFor(async()=> (await last('UsageOverview'))?.params.projectId==='archive-project');
 await click(1008,88);assert.equal((await last('UsageOverview')).params.projectId,null);
 page.run(['mouse','move','600','450']);await sleep(450);shot('cost-tooltip');cost=tooltip('cost-tooltip');
 await click(1148,153);page.run(['mouse','move','600','450']);await sleep(450);shot('token-tooltip');token=tooltip('token-tooltip');
 assert.ok(Math.abs(cost.width-171.40625)<=2&&Math.abs(cost.height-40.34375)<=2,'Cost tooltip follows source dimensions');
 assert.ok(Math.abs(token.width-131.8125)<=2&&Math.abs(token.height-90.875)<=2,'Four-series tooltip follows source dimensions');
 await click(210,978);shot('settings-global');const beforeGlobal=(await calls('UsageOverview')).length;await press('Escape');await waitFor(async()=>(await calls('UsageOverview')).length>beforeGlobal);assert.equal((await last('UsageOverview')).params.projectId,null);shot('global-returned');
 await click(1068,88);await click(1156,88);const thirty=(await last('UsageOverview')).params.since;
 await click(210,978);const beforeCurrent=(await calls('UsageOverview')).length;await press('Escape');await waitFor(async()=>(await calls('UsageOverview')).length>beforeCurrent);assert.equal((await last('UsageOverview')).params.projectId,'archive-project');assert.equal((await last('UsageOverview')).params.since,thirty);shot('current-returned');
 await click(1122,88);await click(1008,88);await bottom();let box=frames().at(-1);await click(750,box.top+30);await bottom();box=frames().at(-1);const tableHeight=box.bottom-box.top;shot('tables');
 const beforeDisabled=(await calls('UsageSessions')).length;await click(1186,box.bottom-26);assert.equal((await calls('UsageSessions')).length,beforeDisabled,'Disabled Previous page cannot reload page one');
 await click(1190,box.top+84);await press('Escape');assert.equal((await last('UsageSessions')).params.providerId,null);shot('provider-dismissed');
 await click(1190,box.top+84);await press('ArrowDown');await press('ArrowDown');await press('Enter');await waitFor(async()=>(await last('UsageSessions')).params.providerId==='codex');shot('provider-keyboard');
 box=frames().at(-1);await click(1190,box.top+84);await click(600,box.top+84);page.run(['keyboard','type','Recorded task 02']);await waitFor(async()=>(await last('UsageSessions')).params.query==='Recorded task 02');await bottom();const one=frames().at(-1);shot('search');assert.ok(Math.abs(tableHeight-(one.bottom-one.top)-252)<=3,'Nine removed session rows account for exactly 9 × 28px');
 await press('Escape');await click(900,500);shot('draft-returned');assert.ok(pixels('draft-returned','600x38+324+890').equals(draft),'Settings and menu interactions preserve the live draft');
 console.log('Usage interactions passed.');
 }
 if(!interactions||lifecycle){await click(95,184);await click(175,145);await press('ArrowDown');await press('Enter');await click(1008,88);}
 if(lifecycle){await configure({longSeries:true});await click(1236,88);page.run(['mouse','move','354','450']);await sleep(350);shot('ninety-buckets');await click(1205,638);
 const selected=(await last('UsageOverview')).params;const tomorrow=new Date();tomorrow.setUTCHours(0,0,0,0);tomorrow.setUTCDate(tomorrow.getUTCDate()+1);assert.equal(+new Date(selected.since),+tomorrow,'View day targets the first visible bucket after truncating 91 to 90');assert.equal(+new Date(selected.until)-+new Date(selected.since),86400000);
 await click(210,978);const beforeDay=(await calls('UsageOverview')).length;await press('Escape');await waitFor(async()=>(await calls('UsageOverview')).length>beforeDay);assert.equal((await last('UsageOverview')).params.since,selected.since);assert.equal((await last('UsageOverview')).params.until,selected.until);shot('day-returned');
 await configure({longSeries:false});await click(420,153);await click(1236,88);
 await configure({hold:true});await click(1236,88);const held=(await last('UsageOverview')).id;await configure({owner:'usage-fixture-owner-b'});await waitFor(async()=>(await last('UsageOverview')).id!==held);await sleep(500);shot('owner-pending');assert.equal(blue('owner-pending'),0,'An account replacement clears previous report and caches');
 const pending=pixels('owner-pending','984x850+260+120');await configure({releaseId:held});await sleep(500);shot('retired-owner');assert.ok(pixels('retired-owner','984x850+260+120').equals(pending),'A retired account receipt cannot repopulate its report');
 await configure({hold:false,release:true});await sleep(500);shot('owner-loaded');const ownerTotal=pixels('owner-loaded','240x45+284+211');assert.ok(blue('owner-loaded')>1000);
 await configure({hold:true});await click(1068,88);await click(1008,88);shot('owner-cached');assert.ok(pixels('owner-cached','240x45+284+211').equals(ownerTotal),'Scope cache belongs to the replacement account');await configure({hold:false,release:true});
 console.log('Usage lifecycle passed.');
 }
 if(phase==='sessions'){
 await bottom();let box=frames().at(-1);await click(750,box.top+30);await bottom();box=frames().at(-1);
 await click(1218,box.bottom-26);await waitFor(async()=>(await last('UsageSessions')).params.offset===10);box=frames().at(-1);await click(1194,box.bottom-26);await waitFor(async()=>(await last('UsageSessions')).params.offset===0);
 for(const [x,sort] of [[1210,'cost'],[840,'last-active'],[1050,'tokens']]){await click(x,box.top+120);await waitFor(async()=>(await last('UsageSessions')).params.sort===sort);}
 await click(1190,box.top+84);await press('ArrowDown');await press('ArrowDown');await press('Enter');await waitFor(async()=>(await last('UsageSessions')).params.providerId==='codex');
 await configure({fail:true});await click(600,box.top+84);page.run(['keyboard','type','retry']);await waitFor(async()=>(await last('UsageSessions')).params.query==='retry');shot('sessions-error');await configure({fail:false});const beforeRetry=(await calls('UsageSessions')).length;await click(1236,88);await waitFor(async()=>(await calls('UsageSessions')).length>beforeRetry);assert.equal((await last('UsageSessions')).params.providerId,'codex');assert.equal((await last('UsageSessions')).params.query,'retry');shot('sessions-retried');console.log('Usage session paging, sorting and retry passed.');
 }
 const final=await proof();assert.equal(final.events.filter(e=>e.blocked).length,0);const result=page.run(['errors']),errors=Array.isArray(result)?result:result.errors;assert.deepEqual(errors,[]);
 artifact('usage-details-'+(phase==='all'?'acceptance':phase)+'.json',{compiledBrowser:true,phase,syntheticRecordedHistory:true,syntheticProviderAllowances:true,realWorkspace:true,...(interactions?{sourceTooltipDimensions:{cost,token},sessionRows28px:true,keyboardProviderSelection:true,outsideDismissal:true,globalAndCurrentSettingsReturn:true,periodRetained:true,draftPreserved:true}:{}),...(lifecycle?{periodAndDayRetained:true,ninetyBucketRange:true,retiredOwnerIgnored:true,cacheAccountBound:true}:{}),...(phase==='sessions'?{pagination:true,threeSorts:true,queryAndProviderRetry:true}:{}),errors,events:final.events});console.log('Usage details acceptance passed.');
}catch(error){console.error('Usage details acceptance failed: '+(error.stack??error.message));try{shot('failure');artifact('usage-details-failure-wire.json',await proof());}catch{}process.exitCode=1;}
finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}
