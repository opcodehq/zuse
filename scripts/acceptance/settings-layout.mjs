// Reference settings geometry at the breakpoint, with actual preference and focus actions.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {execFileSync} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page;
const click=async(x,y)=>{page.click(x,y);await sleep(250);};
const key=value=>page.run(['press',value]);
function samples(file,points,width){const pixels=execFileSync('convert',['.context/'+file,'-depth','8','rgb:-'],{maxBuffer:16*1024*1024});return points.map(([x,y])=>[...pixels.subarray((y*width+x)*3,(y*width+x)*3+3)]);}
try{
 host=await engine('settings-layout');page=browser('settings-layout');page.open(host.url,fixture('remote-access-settings-observer.js'));page.wait('window.__remoteAccess?.methods.includes("WatchChats")');
 const draft='Preserve my settings layout draft.';await click(550,891);page.run(['keyboard','type',draft]);await click(210,978);
 const layouts=[];
 for(const width of[1280,801,800,760]){
  page.run(['set','viewport',String(width),'900']);await sleep(250);const compact=width<800;const x=compact?64:width===1280?360:232;const top=compact?141:149;const file='settings-layout-browser-general-'+width+'.png';page.screenshot(file);
  const colors=samples(file,[[Math.round((width+(compact?48:208))/2),top],[Math.round((width+(compact?48:208))/2),top-1],[x,top+30],[x-1,top+30]],width);
  assert.ok(colors[0][0]>=25&&colors[1][0]<20&&colors[2][0]>=25&&colors[3][0]<20,'General bounds at '+width);layouts.push({width,x,top});
 }
 await click(711,465);page.wait('JSON.parse(localStorage.getItem("zuse.workspace.preferences.v1")).providerUpdateNotifications===true');key('Space');page.wait('JSON.parse(localStorage.getItem("zuse.workspace.preferences.v1")).providerUpdateNotifications===false');
 await click(180,559);page.screenshot('settings-layout-browser-sound-popup.png');key('Escape');await sleep(150);
 // Settings remains open after dismissing the popup, and another page is usable.
 await click(24,182);page.wait('window.__remoteAccess.methods.includes("ListPairings")');page.screenshot('settings-layout-browser-remote-760.png');
 await click(24,121);await sleep(150);page.screenshot('settings-layout-browser-defaults-760.png');
 await click(24,242);page.run(['set','viewport','1280','1000']);await sleep(250);page.screenshot('settings-layout-browser-shortcuts-wide.png');
 const wide=samples('settings-layout-browser-shortcuts-wide.png',[[232,128],[231,128]],1280);assert.ok(wide[0][0]>=20&&wide[1][0]<20,'Shortcuts uses the available wide column');
 await click(125,17);await click(570,891);key('Control+a');key('Control+c');page.wait(`window.__remoteAccess.copied===${JSON.stringify(draft)}`);page.screenshot('settings-layout-browser-draft-preserved.png');assert.equal((await host.call('WatchChats')).length,0);
 artifact('settings-layout-browser-acceptance.json',{compiledGpui:true,realEngine:true,layouts,breakpointExclusive:true,preferencePointerAndKeyboard:true,popupEscapeKeepsSettings:true,compactRemoteAndDefaults:true,wideShortcuts:true,draftPreserved:true,chats:0});console.log('Settings layout acceptance passed: breakpoint geometry, compact preference/keyboard/popup actions, navigation, wide shortcuts and exact draft preservation.');
}catch(error){try{page?.screenshot('settings-layout-browser-failure.png');}catch{}console.error('Settings layout acceptance failed: '+error.message);process.exitCode=1;}
finally{page?.close();await host?.close();}
