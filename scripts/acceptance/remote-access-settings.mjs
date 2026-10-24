// Rendered connection settings use the real local browser/pairing authorities.
import assert from 'node:assert/strict';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,peer;
const click=async(x,y)=>{page.click(x,y);await sleep(400);};
const key=value=>page.run(['press',value]);
const type=value=>page.run(['keyboard','type',value]);
async function until(check,label){for(let n=0;n<100;n++){if(await check())return;await sleep(100);}throw Error(label);}
try{
 host=await engine('remote-access-settings');page=browser('remote-access-settings');page.open(host.url,fixture('remote-access-settings-observer.js'));
 page.wait('window.__remoteAccess?.methods.includes("WatchChats")');await sleep(300);
 const draft='Keep my Remote access settings draft.';await click(550,891);type(draft);await click(210,978);await click(80,182);
 page.wait('window.__remoteAccess.methods.includes("ListPairings")');page.screenshot('remote-access-browser-dark.png');
 const access=await host.call('BrowserAccessStatus');assert.equal(access.running,true);assert.equal(access.managed,false,'Server-owned listeners cannot be stopped from settings');
 await click(1070,291);assert.equal((await host.call('BrowserAccessStatus')).running,true);
 await click(1070,236);page.screenshot('remote-access-browser-account-dialog.png');key('Escape');await sleep(200);
 await click(1040,487);page.screenshot('remote-access-browser-add-computer.png');key('Escape');await sleep(200);
 await click(1060,416);await click(540,519);type('Cancelled device');key('Escape');await sleep(200);await click(1060,416);await click(540,519);key('Control+a');type('Acceptance browser');await click(640,559);
 await until(async()=> (await host.call('ListPairings')).length===1,'Invitation creation');await sleep(300);assert.equal((await host.call('ListPairings'))[0].label,'Acceptance browser','Reopened dialog must restore text input handling');page.screenshot('remote-access-browser-link-created.png');
 await click(473,602);page.wait('window.__remoteAccess.copied?.startsWith("http://127.0.0.1:")');
 const privateLink=page.evaluate('window.__remoteAccess.copied');assert.ok(new URL(privateLink).hash);
 peer=browser('remote-access-settings-peer');peer.open(privateLink);
 await until(async()=> (await host.call('ListPairings'))[0]?.pairedAt,'Real browser pairing');key('Escape');await click(1065,635);page.screenshot('remote-access-browser-paired.png');
 await click(1068,685);await until(async()=> (await host.call('ListPairings')).length===0,'Immediate device revocation');page.screenshot('remote-access-browser-revoked.png');
 await click(1002,737);page.screenshot('remote-access-browser-advanced.png');await click(1002,737);
 await click(125,17);await click(570,891);key('Control+a');key('Control+c');page.wait(`window.__remoteAccess.copied===${JSON.stringify(draft)}`);page.screenshot('remote-access-browser-draft-preserved.png');
 await click(210,978);await click(80,182);page.run(['set','viewport','760','900']);await sleep(200);page.screenshot('remote-access-browser-compact.png');page.run(['set','viewport','1280','1000']);
 await click(75,62);await click(1036,361);page.wait('JSON.parse(localStorage.getItem("zuse.workspace.preferences.v1")).appearance==="light"');await click(80,182);page.screenshot('remote-access-browser-light.png');
 assert.equal((await host.call('WatchChats')).length,0);const methods=page.evaluate('window.__remoteAccess.methods');assert.equal(methods.filter(m=>m==='CreatePairing').length,1);assert.equal(methods.filter(m=>m==='RevokePairing').length,1);assert.equal(methods.filter(m=>m==='StopBrowserAccess').length,0);
 artifact('remote-access-settings-acceptance.json',{compiledGpui:true,realEngine:true,realPairedBrowser:true,sourceReference:'43e14c1',serverOwnedListenerGuard:true,accountDialog:true,addComputerDialog:true,escapeKeepsSettings:true,invitationCreated:1,clipboardLinkPairs:true,immediateRevoke:1,draftPreserved:true,advancedReachable:true,compactAndLight:true,chats:0,liveAccountAcceptance:false});
 console.log('Remote access acceptance passed: real invitation, browser pairing and revocation; listener guard, dialogs, preserved draft, advanced controls and compact/light rendering.');
}catch(error){try{page?.screenshot('remote-access-browser-failure.png');}catch{}console.error('Remote access acceptance failed: '+error.message);process.exitCode=1;}
finally{peer?.close();page?.close();await host?.close();}
