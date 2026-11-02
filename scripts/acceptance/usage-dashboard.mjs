// Compiled GPUI browser + real disposable workspace + isolated usage/provider DTOs.
import fs from 'node:fs';import path from 'node:path';import {spawn,execFileSync} from 'node:child_process';import assert from 'node:assert/strict';
import {engine,browser,fixture,sleep,artifact} from './harness.mjs';import {seedProjectArchives} from './fixtures/project-archive-data.mjs';
let host,page,proxy,url;
const click=async(x,y)=>{page.run(['batch','--bail',`mouse move ${x} ${y}`,'mouse down','mouse up','wait 250']);await sleep(350);};
const shot=name=>page.screenshot('usage-dashboard-'+name+'.png');
const configure=patch=>fetch(url+'/__fixture',{method:'POST',headers:{Connection:'close'},body:JSON.stringify(patch)});
// Browser commands can block past keep-alive expiry; use fresh fixture-control sockets.
const proof=()=>fetch(url+'/__fixture',{headers:{Connection:'close'}}).then(r=>r.json());
const calls=async method=>(await proof()).events.filter(e=>e.method===method);
const last=async method=>(await calls(method)).at(-1)?.params;
const bottom=()=>click(1272,980);
const frames=()=>{
 page.screenshot('usage-dashboard-geometry.png');
 const pixels=execFileSync('convert',['.context/usage-dashboard-geometry.png','-crop','1x892+261+104','-depth','8','rgb:-']);
 const match=i=>pixels[i*3]===34&&pixels[i*3+1]===37&&pixels[i*3+2]===35,rows=[];
 for(let i=0;i<pixels.length/3;i++){if(!match(i))continue;let end=i;while(end+1<pixels.length/3&&match(end+1))end++;if(end-i>20)rows.push({top:104+i-7,bottom:104+end+8});i=end;}
 return rows;
};
const session=()=>{const rows=frames();assert.ok(rows.length,'The rendered dashboard has visible frames');return rows.at(-1);};
const bluePixels=file=>{const p=execFileSync('convert',[file,'-crop','845x210+354+355','-depth','8','rgb:-']);let n=0;for(let i=0;i<p.length;i+=3)if(p[i+2]>p[i]+30&&p[i+2]>p[i+1]+20)n++;return n;};
const mainPixels=name=>execFileSync('convert',['.context/usage-dashboard-'+name+'.png','-crop','984x850+260+120','-depth','8','rgb:-'],{maxBuffer:4*1024*1024});
try{
 host=await engine('usage-dashboard-acceptance');const data=await seedProjectArchives(host);const ready=path.join(host.root,'fixture.ready');
 proxy=spawn('bun',[fixture('usage-dashboard.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let i=0;i<100&&!fs.existsSync(ready);i++){assert.equal(proxy.exitCode,null);await sleep(100);}url=fs.readFileSync(ready,'utf8');
 page=browser('usage-dashboard-acceptance');page.open(url,fixture('project-archive-observer.js'));page.wait('window.__uiParity.methods.includes("WatchChats")');await sleep(1100);
 await click(95,184);await click(600,912);page.run(['keyboard','type','Keep the active conversation draft.']);await click(900,500);shot('draft-before');
 const draft=execFileSync('convert',['.context/usage-dashboard-draft-before.png','-crop','600x38+324+890','-blur','0x1','-depth','8','rgb:-']);
 await click(175,145);page.run(['batch','--bail','press ArrowDown','wait 180','press Enter','wait 400']);await sleep(600);
 assert.equal((await last('UsageOverview')).projectId,'archive-project');assert.equal((await last('UsageOverview')).targetDeviceId,data.deviceId);shot('project');
 page.run(['press','Escape']);await sleep(650);await click(900,500);shot('draft-early-return');
 assert.ok(execFileSync('convert',['.context/usage-dashboard-draft-early-return.png','-crop','600x38+324+890','-blur','0x1','-depth','8','rgb:-']).equals(draft),'Escape returns from a focused dashboard control');
 await click(175,145);page.run(['batch','--bail','press ArrowDown','wait 180','press Enter','wait 400']);await sleep(600);
 await click(1008,88);assert.equal((await last('UsageOverview')).projectId,null);shot('global');
 const seven=(await last('UsageOverview')).since;
 await click(1156,88);assert.equal((new Date(seven)-new Date((await last('UsageOverview')).since))/86400000,23);
 await click(1188,88);assert.equal((new Date(seven)-new Date((await last('UsageOverview')).since))/86400000,83);
 await click(1122,88);
 await click(1236,88);assert.equal((await last('UsageOverview')).forceRefresh,true);assert.equal((await last('ListAgentAccounts')).forceUsage,true);
 await configure({fail:true});await click(1236,88);shot('refresh-error');await configure({fail:false});await click(1236,88);
 await configure({hold:true});await click(1068,88);shot('scope-loading');const held=(await calls('UsageOverview')).at(-1).id;
 await configure({hold:false});await click(1008,88);shot('before-late');await configure({release:true});await sleep(500);shot('after-late');assert.ok(mainPixels('before-late').equals(mainPixels('after-late')),'Retired project report must not replace the global scope');assert.ok((await proof()).events.some(e=>e.cancel===held));
 page.run(['batch','--bail','mouse move 800 450','wait 250']);shot('hover');await click(1184,644);assert.ok((await last('UsageOverview')).until,'Day selection sends an exclusive upper bound');shot('day');await click(410,153);assert.equal((await last('UsageOverview')).until,null);
 await bottom();shot('lower');
 let breakdown=frames().at(-2);await click(1148,breakdown.top+26);shot('models');
 await bottom();breakdown=frames().at(-2);await click(750,breakdown.bottom-18);shot('models-expanded');
 await bottom();breakdown=frames().at(-2);await click(750,breakdown.bottom-18);breakdown=frames().at(-2);await click(1092,breakdown.top+26);
 let box=session();await click(750,box.top+30);await bottom();shot('sessions');assert.equal((await last('UsageSessions')).limit,10);
 box=session();await click(1218,box.bottom-24);assert.equal((await last('UsageSessions')).offset,10);shot('sessions-page-two');box=session();await click(1194,box.bottom-24);assert.equal((await last('UsageSessions')).offset,0);
 box=session();await click(1050,box.top+120);assert.equal((await last('UsageSessions')).sort,'tokens');await click(1210,box.top+120);assert.equal((await last('UsageSessions')).sort,'cost');await click(840,box.top+120);assert.equal((await last('UsageSessions')).sort,'last-active');
 await click(600,box.top+84);page.run(['keyboard','type','Recorded task 02']);await sleep(350);assert.equal((await last('UsageSessions')).query,'Recorded task 02');assert.equal((await last('UsageSessions')).offset,0);shot('sessions-search');
 page.run(['batch','--bail','press Control+a','press Backspace','wait 300']);await bottom();box=session();
 await click(1174,box.top+84);shot('provider-menu');await click(1155,box.top+186);assert.equal((await last('UsageSessions')).providerId,'codex');shot('provider-filter');
 // A failed session read retries the current provider and query without losing scope.
 await configure({fail:true});box=session();await click(600,box.top+84);page.run(['keyboard','type','retry']);await sleep(350);shot('sessions-error');await configure({fail:false});
 box=session();const beforeRetry=(await calls('UsageSessions')).length;await click(1236,88);assert.equal((await calls('UsageSessions')).length,beforeRetry+1);assert.equal((await last('UsageSessions')).providerId,'codex');assert.equal((await last('UsageSessions')).query,'retry');shot('sessions-retried');
 // Return by keyboard; opening the dashboard never replaces or sends the live draft.
 page.run(['press','Escape']);await sleep(500);await click(900,500);shot('draft-returned');
 assert.ok(execFileSync('convert',['.context/usage-dashboard-draft-returned.png','-crop','600x38+324+890','-blur','0x1','-depth','8','rgb:-']).equals(draft),'Dashboard return preserves the visible draft');
 await configure({unpriced:true});const beforeFull=(await calls('UsageOverview')).length;
 await click(180,978);page.run(['batch','--bail','press End','wait 180','press Enter','wait 450']);await sleep(450);shot('unpriced');
 assert.equal((await calls('UsageOverview')).length,beforeFull+1,'Full usage opens a new global dashboard read');assert.equal((await last('UsageOverview')).projectId,null);
 assert.ok(bluePixels('.context/usage-dashboard-global.png')>1000,'Recorded costs paint a blue cost series');assert.equal(bluePixels('.context/usage-dashboard-unpriced.png'),0,'Unpriced records do not become estimated costs');
 page.run(['press','Escape']);await sleep(400);
 const final=await proof();assert.equal(final.events.filter(e=>e.blocked).length,0);
 artifact('usage-dashboard-acceptance.json',{compiledBrowser:true,realWorkspace:true,syntheticRecordedHistory:true,syntheticProviderAllowances:true,projectAndGlobal:true,allPeriods:true,forceRefresh:true,savedReportOnRefreshFailure:true,lateReportIgnored:true,daySelection:true,sessionsPagination:true,searchAndProviderFilter:true,threeSorts:true,fullUsageKeyboard:true,unpricedCostsRemainUnknown:true,draftPreserved:true,providerWrites:0,events:final.events,errors:page.run(['errors'])});console.log('Usage dashboard acceptance passed.');
}catch(error){console.error('Usage dashboard acceptance failed: '+error.message);try{shot('failure');artifact('usage-dashboard-failure-wire.json',await proof());}catch{}process.exitCode=1;}finally{page?.close();proxy?.kill('SIGTERM');await host?.close();}
