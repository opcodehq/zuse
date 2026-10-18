import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
const source=await readFile(new URL('../src/voice_media.js',import.meta.url),'utf8');
const media=await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const events=[];const tracks=[];const peers=[];const contexts=[];
const tick=()=>new Promise(resolve=>setTimeout(resolve,0));
class Track {enabled=true;readyState='live';stop(){this.readyState='ended';}}
const stream=()=>{const track=new Track();tracks.push(track);return {getTracks:()=>[track],getAudioTracks:()=>[track]};};
const node=()=>({gain:{value:0},connect(){},disconnect(){},fftSize:256,getByteTimeDomainData(values){values.fill(128);}});
class Context {state='running';constructor(){contexts.push(this);}resume(){return Promise.resolve();}close(){this.state='closed';return Promise.resolve();}createGain(){return node();}createAnalyser(){return node();}createMediaStreamSource(){return node();}}
class Peer extends EventTarget {
  connectionState='new';iceGatheringState='complete';listeners=0;closed=false;
  constructor(){super();peers.push(this);}createDataChannel(){return {};}
  addTrack(track){assert.equal(track.enabled,false,'capture must be disabled before adding a sender');}
  createOffer(){return Promise.resolve({type:'offer',sdp:'offer-sdp'});}
  setLocalDescription(value){this.localDescription=value;return Promise.resolve();}
  setRemoteDescription(){this.connectionState='connected';return Promise.resolve();}
  addEventListener(...args){this.listeners++;super.addEventListener(...args);}
  removeEventListener(...args){this.listeners--;super.removeEventListener(...args);}
  close(){this.closed=true;this.connectionState='closed';}
}
const document=new EventTarget();const window=new EventTarget();
Object.assign(window,{isSecureContext:true,RTCPeerConnection:Peer,AudioContext:Context});
Object.assign(globalThis,{window,document,RTCPeerConnection:Peer,AudioContext:Context,matchMedia:()=>Object.assign(new EventTarget(),{matches:false})});
const devices={getUserMedia:async()=>stream()};Object.defineProperty(globalThis,'navigator',{value:{mediaDevices:devices},configurable:true});
media.installVoiceMedia(serialized=>events.push(JSON.parse(serialized).voiceMedia));
const command=(action,attempt,value={})=>media.voiceMediaCommand(JSON.stringify({action,attempt,value}));
const reset=()=>{events.length=0;tracks.length=0;peers.length=0;contexts.length=0;devices.getUserMedia=async()=>stream();};
test('capture stays disabled through offer and answer until explicit activation, then mute and stop release it',async()=>{
 reset();command('prepare','one');await tick();assert.equal(events.at(-1).kind,'prepared');assert.equal(tracks[0].enabled,false);
 command('offer','one');await tick();assert.equal(events.at(-1).kind,'offer');
 command('answer','one',{sdp:'answer'});await tick();assert.equal(events.at(-1).kind,'ready');assert.equal(tracks[0].enabled,false);
 command('activate','one',{muted:false});await tick();assert.equal(events.at(-1).kind,'active');assert.equal(tracks[0].enabled,true);
 command('mute','one',{muted:true});await tick();assert.equal(tracks[0].enabled,false);
 command('stop','one');assert.equal(tracks[0].readyState,'ended');assert.ok(peers[0].closed);assert.equal(contexts[0].state,'closed');
});
test('mute remains usable while the microphone permission prompt is pending',async()=>{
 reset();let resolve;devices.getUserMedia=()=>new Promise(done=>resolve=done);
 command('prepare','permission');command('mute','permission',{muted:true});await tick();
 assert.equal(events.length,0);resolve(stream());await tick();assert.equal(events.at(-1).kind,'prepared');
 assert.equal(tracks[0].enabled,false);command('activate','permission',{muted:true});await tick();
 assert.equal(events.at(-1).kind,'active');assert.equal(tracks[0].enabled,false);command('stop','permission');
});
test('a permission result arriving after cancellation stops its tracks and cannot replace a newer call',async()=>{
 reset();let resolve;devices.getUserMedia=()=>new Promise(done=>resolve=done);
 command('prepare','old');command('stop','old');
 devices.getUserMedia=async()=>stream();command('prepare','new');await tick();
 const oldStream=stream();resolve(oldStream);await tick();
 assert.equal(oldStream.getTracks()[0].readyState,'ended');assert.equal(events.some(e=>e.attempt==='old'),false);
 command('activate','old',{muted:false});await tick();assert.equal(tracks[0].enabled,false);
 command('stop','old');assert.equal(tracks[0].readyState,'live');command('stop','new');assert.equal(tracks[0].readyState,'ended');
});
test('cancelled media waits remove their listeners and retire pending asynchronous operations',async()=>{
 reset();command('prepare','wait');await tick();peers[0].iceGatheringState='gathering';
 command('offer','wait');await tick();assert.equal(peers[0].listeners,1);
 command('stop','wait');await tick();assert.equal(peers[0].listeners,0);assert.ok(peers[0].closed);assert.equal(events.some(e=>e.kind==='offer'),false);
});
test('a newer mute wins over an activation whose audio resume is still pending',async()=>{
 reset();command('prepare','mute');await tick();let resolve;contexts[0].resume=()=>new Promise(done=>resolve=done);
 command('activate','mute',{muted:false});command('mute','mute',{muted:true});await tick();resolve();await tick();
 assert.equal(tracks[0].enabled,false);command('stop','mute');
});
test('permission refusal fails locally without constructing a peer or leaking raw errors',async()=>{
 reset();devices.getUserMedia=async()=>{throw new DOMException('private device details','NotAllowedError');};
 command('prepare','denied');await tick();assert.equal(peers.length,0);assert.equal(events.length,1);assert.equal(events[0].kind,'error');assert.match(events[0].message,/permission/);assert.ok(!events[0].message.includes('private device details'));
});
test('local capture expires if host lease acknowledgements stop reaching the controller',async()=>{
 reset();const original=globalThis.performance;let now=0;
 Object.defineProperty(globalThis,'performance',{value:{now:()=>now},configurable:true});
 try{
  command('prepare','expired');await tick();command('activate','expired',{muted:false});await tick();assert.equal(tracks[0].enabled,true);
  command('lease','expired');now=15001;
  await new Promise(resolve=>setTimeout(resolve,1100));
  assert.equal(tracks[0].readyState,'ended');assert.ok(peers[0].closed);assert.equal(contexts[0].state,'closed');assert.equal(events.filter(e=>e.kind==='error').length,1);
 }finally{command('stop','expired');Object.defineProperty(globalThis,'performance',{value:original,configurable:true});}
});
