import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createZuseClient, ZuseError } from '../dist/index.js';

class FakeSocket extends EventTarget {
  static last: FakeSocket;
  sent: any[] = [];
  constructor(_url: string) { super(); FakeSocket.last = this; queueMicrotask(() => this.dispatchEvent(new Event('open'))); }
  send(text: string) { this.sent.push(JSON.parse(text)); }
  close() { this.dispatchEvent(new Event('close')); }
  receive(value: object) { this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(value) })); }
}
function client() { return createZuseClient({ endpoint:'ws://127.0.0.1:27654', token:'private', webSocket:FakeSocket as unknown as typeof WebSocket }); }
async function sent() { await new Promise(resolve => setImmediate(resolve)); return FakeSocket.last; }

test('authenticates envelopes and preserves a nullable result', async () => {
  const api=client(); const result=api.call('ZotCommand',{commandId:'same-id'}); const socket=await sent();
  const request=socket.sent[0]; assert.equal(request.method,'ControlCall'); assert.equal(request.params.token,'private');
  assert.equal(request.params.params.commandId,'same-id'); socket.receive({id:request.id,ok:null}); assert.equal(await result,null);api.close();
});
test('cancellation sends cancellation without replay', async () => {
  const api=client();const controller=new AbortController();const result=api.call('ControlInfo',{}, {signal:controller.signal});
  const rejected=assert.rejects(result,(e:any)=>e.code==='cancelled'); const socket=await sent();controller.abort();await rejected;
  assert.deepEqual(socket.sent[1],{id:socket.sent[0].id,cancel:true});api.close();
});
test('subscription return cancels the server operation', async () => {
  const api=client();const iterator=api.zots.watch();const socket=await sent();const id=socket.sent[0].id;
  socket.receive({id,item:[]});assert.deepEqual(await iterator.next(),{value:[],done:false});await iterator.return!();
  assert.deepEqual(socket.sent.at(-1),{id,cancel:true});api.close();
});
test('transport failure rejects writes and never replays them', async () => {
  const api=client();const result=api.call('ZotCommand',{});const rejected=assert.rejects(result,(e:any)=>e.code==='transport');const socket=await sent();
  socket.close();await rejected;assert.equal(socket.sent.length,1);
});
test('unsupported capabilities and bad credentials are typed failures', async () => {
  for(const reason of ['control.unauthorized','control.unsupported_operation']){
    const api=client();const result=api.info();const rejected=assert.rejects(result,(e:any)=>e instanceof ZuseError&&e.code===(reason.endsWith('unauthorized')?'unauthorized':'unsupported'));
    const socket=await sent();socket.receive({id:socket.sent[0].id,err:`capability error: ${reason}`});await rejected;api.close();
  }
});
test('remote connections require TLS and credentials stay out of URLs',()=>{
  assert.throws(()=>createZuseClient({endpoint:'ws://remote.example/api/control',token:'secret'}));
  assert.throws(()=>createZuseClient({endpoint:'wss://remote.example/api/control?token=secret',token:'secret'}));
});
test('slow subscription consumers receive bounded overflow errors',async()=>{
  const api=client();const iterator=api.zots.watch();const socket=await sent();const id=socket.sent[0].id;
  for(let index=0;index<257;index++)socket.receive({id,item:[]});
  await assert.rejects(iterator.next(),(e:any)=>e.code==='overflow');assert.deepEqual(socket.sent.at(-1),{id,cancel:true});api.close();
});
test('requests can be cancelled before a connection opens', async()=>{
  class Connecting extends EventTarget {
    constructor(_url:string){super();}
    send(){assert.fail('cancelled requests must not be sent');}
    close(){this.dispatchEvent(new Event('close'));}
  }
  const api=createZuseClient({endpoint:'ws://127.0.0.1:27654',token:'private',webSocket:Connecting as unknown as typeof WebSocket});
  const controller=new AbortController();const result=api.info({signal:controller.signal});
  const rejected=assert.rejects(result,(e:any)=>e.code==='cancelled');controller.abort();await rejected;api.close();
});
