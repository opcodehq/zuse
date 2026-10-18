import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
const source=await readFile(new URL('../src/browser_frame.js',import.meta.url),'utf8');
const adapter=await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const frames=[],events=[];
class Frame {style={};attrs={};remove(){this.removed=true;}setAttribute(k,v){this.attrs[k]=v;}}
Frame.prototype.credentialless=false;
Object.assign(globalThis,{window:Object.assign(new EventTarget(),{location:{origin:'http://127.0.0.1:27666'}}),HTMLIFrameElement:Frame,document:{createElement(){return new Frame();},body:{appendChild(frame){frames.push(frame);}}}});
const command=(generation,action,extra={})=>adapter.browserFrameCommand(JSON.stringify({generation,action,...extra}));
adapter.installBrowserFrame(value=>events.push(JSON.parse(value).browserFrame),true);
test('isolated navigation scopes layout, callbacks and destruction to its generation',()=>{
 command(1,'navigate',{url:'localhost:3000'});const first=frames.at(-1);assert.ok(first.credentialless);assert.equal(first.src,'http://localhost:3000/');assert.equal(first.referrerPolicy,'no-referrer');assert.ok(!first.attrs.sandbox.includes('allow-top-navigation'));
 command(1,'bounds',{bounds:[200,100,500,600]});command(1,'visible',{visible:true});assert.equal(first.style.display,'block');assert.equal(first.style.left,'200px');
 const late=first.onload;command(2,'navigate',{url:'https://example.com'});const current=frames.at(-1);assert.ok(first.removed);const before=events.length;late();assert.equal(events.length,before);
 command(1,'close');assert.ok(!current.removed);command(2,'bounds',{bounds:[0,0,400,300]});command(2,'visible',{visible:true});assert.equal(current.style.display,'block');command(2,'visible',{visible:false});assert.equal(current.style.display,'none');command(2,'close');assert.ok(current.removed);
});
test('unsupported schemes, credentials, application origin and remote local-proxy URLs never create frames',()=>{
 let generation=3;
 for(const url of ['javascript:alert(1)','data:text/html,hello','file:///etc/passwd','https://user:password@example.com','http://127.0.0.1:27666/api/rpc']){
  const count=frames.length;command(generation++,'navigate',{url});assert.equal(frames.length,count);assert.equal(events.at(-1).kind,'error');
 }
 adapter.installBrowserFrame(value=>events.push(JSON.parse(value).browserFrame),false);
 const count=frames.length;command(generation++,'navigate',{url:'http://project.localhost:7331',preview:true});assert.equal(frames.length,count);assert.equal(events.at(-1).kind,'error');
 delete Frame.prototype.credentialless;command(generation,'navigate',{url:'https://example.com'});assert.equal(frames.length,count);assert.equal(events.at(-1).url,'https://example.com/');assert.equal(events.at(-1).kind,'error');
});
