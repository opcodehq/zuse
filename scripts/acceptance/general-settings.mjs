// Rendered General/account flows with isolated identity responses and real workspace reads.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,child,url;
const state=()=>fetch(url+'/__fixture').then(r=>r.json());
const update=value=>fetch(url+'/__fixture',{method:'POST',body:JSON.stringify(value)});
async function until(check,label){for(let n=0;n<50;n++){const value=await state();if(check(value))return value;await sleep(100);}throw Error(label);}
const click=async(x,y)=>{page.click(x,y);await sleep(250);};
const press=key=>page.run(['press',key]);
const type=value=>page.run(['keyboard','type',value]);
const prefs=()=>page.evaluate('JSON.parse(localStorage.getItem("zuse.workspace.preferences.v1"))');
const waitPrefs=condition=>page.wait(`(()=>{const p=JSON.parse(localStorage.getItem("zuse.workspace.preferences.v1"));return ${condition};})()`);
async function openSettings(){await click(274,982);}
function pixelSamples(file,points){
 const png=fs.readFileSync('.context/'+file).toString('base64');
 return page.evaluate(`(async()=>{const image=new Image();image.src='data:image/png;base64,${png}';await image.decode();const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;const c=canvas.getContext('2d');c.drawImage(image,0,0);return ${JSON.stringify(points)}.map(([x,y])=>Array.from(c.getImageData(x,y,1,1).data));})()`);
}
try{
 host=await engine('general-settings');const ready=path.join(host.root,'fixture.ready');
 child=spawn('bun',[fixture('general-settings.mjs'),String(host.port),ready,path.resolve('dist/web')],{stdio:'ignore'});
 for(let n=0;n<100&&!fs.existsSync(ready);n++){assert.equal(child.exitCode,null);await sleep(100);}
 assert.ok(fs.existsSync(ready));url=fs.readFileSync(ready,'utf8');await update({hold:true});
 page=browser('general-settings');page.open(url,fixture('general-settings-observer.js'));
 page.wait('window.__generalParity.methods.includes("CloudAccountStatus")');await openSettings();page.screenshot('general-parity-loading.png');
 await update({hold:false});await sleep(150);page.screenshot('general-parity-signed-out.png');
 await click(1076,228);await click(1076,228);
 await until(s=>s.state.login,'Device sign-in progress');
 assert.equal((await state()).events.filter(e=>e.method==='CloudAccountSignIn').length,1,'Repeated clicks must not start another sign-in');
 page.screenshot('general-parity-sign-in-pending.png');
 assert.equal(page.evaluate('window.__generalParity.openedUrls.length'),1,'Sign-in opens the verification URL');
 await click(450,307);assert.equal(page.evaluate('window.__generalParity.openedUrls.length'),2,'Reopen pending sign-in');
 await click(560,307);await until(s=>s.events.some(e=>e.method==='CloudAccountSignOut')&&!s.state.login,'Cancel device sign-in');
 await click(1076,228);await until(s=>s.state.login,'Retry device sign-in');
 await update({state:{user:{id:'avery',name:'Avery Parker',email:'avery@example.test'},login:null,error:null}});await sleep(150);
 page.screenshot('general-parity-signed-in.png');
 // Actual CSS component measurements: Account 149–279.85, Appearance 295.85–409.85,
 // Notifications 425.85–612.85 at the authored 768px settings column.
 const bounds=pixelSamples('general-parity-signed-in.png',[[744,149],[744,279],[744,296],[744,409],[744,426],[744,612],[744,148],[744,281],[744,294],[744,411],[744,424],[744,614]]);
 assert.ok(bounds.slice(0,6).every(([r])=>r>=30&&r<70),'General frames must match the measured reference bounds');
 assert.ok(bounds.slice(6).every(([r])=>r<20),'Gaps must remain outside the measured frames');
 await click(490,251);page.screenshot('general-parity-email-revealed.png');
 const hidden=fs.readFileSync('.context/general-parity-signed-in.png').toString('base64');
 const revealed=fs.readFileSync('.context/general-parity-email-revealed.png').toString('base64');
 const contrasts=page.evaluate(`(async()=>{
  const contrast=async data=>{const i=new Image();i.src='data:image/png;base64,'+data;await i.decode();const c=document.createElement('canvas');c.width=i.width;c.height=i.height;const x=c.getContext('2d');x.drawImage(i,0,0);const pixels=x.getImageData(437,244,127,14).data;let score=0;for(let y=0;y<14;y++)for(let col=0;col<126;col++){const offset=(y*127+col)*4;score+=Math.abs(pixels[offset]-pixels[offset+4]);}return score;};
  return [await contrast('${hidden}'),await contrast('${revealed}')];})()`);
 assert.ok(contrasts[1]>contrasts[0]*1.5,'Reveal must paint sharp email text over the initial blur');await click(490,251);
 await click(533,230);type('Cancelled name');page.screenshot('general-parity-display-name-editing.png');press('Escape');await sleep(100);
 assert.equal(prefs().displayName??'','');page.screenshot('general-parity-edit-cancel.png');
 const stillSettings=pixelSamples('general-parity-edit-cancel.png',[[744,170]]);assert.ok(stillSettings[0][0]>=30,'Escape cancels editing and keeps Settings open');
 await click(533,230);type('Sky Avery');press('Enter');await sleep(100);assert.equal(prefs().displayName,'Sky Avery');
 await click(516,230);press('Control+a');type('Sky on blur');await click(1087,378);waitPrefs("p.displayName==='Sky on blur'");assert.equal(prefs().displayName,'Sky on blur');
 await click(1036,378);waitPrefs("p.appearance==='light'");assert.equal(prefs().appearance,'light');page.screenshot('general-parity-light.png');
 await click(1087,378);waitPrefs("p.appearance==='dark'");assert.equal(prefs().appearance,'dark');
 await click(1095,488);assert.equal(prefs().providerUpdateNotifications,true);press('Space');await sleep(100);assert.equal(prefs().providerUpdateNotifications,false);press('Space');await sleep(100);assert.equal(prefs().providerUpdateNotifications,true);
 await click(1095,542);assert.equal(prefs().completionSound.enabled,false);
 await click(480,584);assert.equal(prefs().completionSound.enabled,false);assert.equal(prefs().completionSound.preset,'chime','Disabled sound selector does not change the preset');page.screenshot('general-parity-sound-disabled.png');
 press('Tab');press('Tab');press('Space');waitPrefs("p.reduceMotion==='on'");assert.equal(prefs().completionSound.enabled,false,'Tab skips the disabled selector and Preview');
 await click(1095,542);waitPrefs('p.completionSound.enabled===true');await sleep(250);await click(480,584);page.screenshot('general-parity-sound-menu.png');press('End');press('Enter');waitPrefs("p.completionSound.preset==='bloom'");assert.equal(prefs().completionSound.preset,'bloom');
 await click(480,584);await click(750,650);await sleep(100);page.screenshot('general-parity-menu-dismissed.png');assert.ok(pixelSamples('general-parity-menu-dismissed.png',[[480,650]])[0][0]>=30,'Outside click dismisses the sound popup');
 await click(1052,706);assert.equal(prefs().reduceMotion,'on');await click(1095,758);assert.equal(prefs().pauseAnimationsInBackground,true);
 const beforeReload=prefs();page.open(url,fixture('general-settings-observer.js'));page.wait('window.__generalParity.methods.includes("CloudAccountStatus")');await openSettings();assert.deepEqual(prefs(),beforeReload,'Actual local preferences survive reload');
 page.screenshot('general-parity-preferences-restored.png');
 // An account replacement retires the in-progress name editor without saving it.
 await click(525,230);press('Control+a');type('Wrong account');
 await update({state:{user:{id:'other',name:'Other person',email:'other@example.test'},login:null,error:null}});await sleep(250);page.screenshot('general-parity-account-switched.png');assert.ok(pixelSamples('general-parity-account-switched.png',[[744,296]])[0][0]>=30,'Account replacement removes the draft editor before Enter');press('Enter');await sleep(100);
 assert.equal(prefs().displayName,'Sky on blur','Old account editor must be discarded');
 await click(1068,239);await until(s=>s.state.user===null,'Sign out');
 await update({failNext:true});await click(1076,228);await sleep(150);page.screenshot('general-parity-sign-in-error.png');await click(1036,397);waitPrefs("p.appearance==='light'");page.screenshot('general-parity-sign-in-error-light.png');await click(1087,397);waitPrefs("p.appearance==='dark'");
 const beforeRetry=(await state()).events.filter(e=>e.method==='CloudAccountSignIn').length;
 await click(1076,228);await until(s=>s.state.login,'Retry after an account error');assert.equal((await state()).events.filter(e=>e.method==='CloudAccountSignIn').length,beforeRetry+1);
 await update({state:{user:{id:'avery',name:'Avery Parker',email:'avery@example.test'},login:null,error:null}});await sleep(150);
 page.run(['set','viewport','760','900']);await sleep(150);page.screenshot('general-parity-compact.png');
 const proof=await state();assert.equal(proof.events.filter(e=>e.blocked).length,0);
 artifact('general-parity-acceptance.json',{compiledGpui:true,disposableRealEngine:true,accountProtocolFixture:true,liveAccountAcceptance:false,referenceCommit:'43e14c1',referenceFrameBounds:true,loadingAndSignedOut:true,pendingSignIn:true,repeatedSignInBlocked:true,reopenAndCancel:true,signedInProfile:true,emailBlurReveal:true,displayNameEnterAndBlurSave:true,escapeCancelsWithoutClosingSettings:true,accountChangeRetiresEditor:true,themeAndNotificationPreferences:true,soundMenuKeyboardAndDisabledState:true,tabSkipsDisabledControls:true,preferencesSurviveReload:true,accountErrorRetry:true,compactSettings:true,agentWrites:0});
 console.log('General settings acceptance passed: reference bounds, account states, sign-in/cancel/retry, email reveal, display-name editing, persistent preferences, keyboard controls and compact layout; zero provider turns.');
}catch(error){try{page?.screenshot('general-parity-failure.png');}catch{}console.error('General settings acceptance failed: '+error.message);process.exitCode=1;}
finally{page?.close();child?.kill('SIGTERM');await host?.close();}
