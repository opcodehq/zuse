import assert from 'node:assert/strict';
import fs from 'node:fs';
import {gzipSync} from 'node:zlib';
import test from 'node:test';

const source=fs.readFileSync(new URL('../src/rpc_compression.js',import.meta.url));
const {decodeRpcFrame,supportsRpcCompression}=await import('data:text/javascript;base64,'+source.toString('base64'));

test('native gzip decoder preserves exact UTF-8 transcript text',async()=>{
  assert(supportsRpcCompression());
  const text=JSON.stringify({id:5,item:{reset:[{text:'Chat 本🦀 '.repeat(14000)}]}});
  const bytes=Buffer.byteLength(text);
  assert.equal(await decodeRpcFrame(gzipSync(text),bytes),text);
});

test('decoded byte cap rejects a small compressed oversized message',async()=>{
  await assert.rejects(decodeRpcFrame(gzipSync('x'.repeat(1024*1024)),65536),/message limit/);
});

test('invalid framing, truncated gzip and invalid UTF-8 fail closed',async()=>{
  await assert.rejects(decodeRpcFrame(new Uint8Array([1,2,3]),65536),/Unexpected/);
  await assert.rejects(decodeRpcFrame(gzipSync('complete').subarray(0,12),65536));
  await assert.rejects(decodeRpcFrame(gzipSync(Buffer.from([255,254])),65536));
});

async function statefulFrames(texts) {
  const {createDeflate,constants} = await import('node:zlib');
  const stream=createDeflate({level:3});
  let chunks=[]; stream.on('data',chunk=>chunks.push(chunk));
  const frames=[];
  for(const text of texts){
    stream.write(text);
    await new Promise((resolve,reject)=>stream.flush(constants.Z_SYNC_FLUSH,error=>error?reject(error):resolve()));
    const header=Buffer.alloc(6);header.write('ZD');header.writeUInt32LE(Buffer.byteLength(text),2);
    frames.push(Buffer.concat([header,...chunks]));chunks=[];
  }
  stream.destroy();return frames;
}

test('connection dictionary preserves every frame and reduces repeated envelopes',async()=>{
  const {createRpcDecoder}=await import('data:text/javascript;base64,'+source.toString('base64'));
  const texts=Array.from({length:40},(_,i)=>JSON.stringify({id:7,item:{append:[{entry:'long-entry-id-123456789',part:'text-part-123456789',text:'Live 本🦀 token '+i,len:i}]}}));
  const frames=await statefulFrames(texts),decoder=createRpcDecoder();
  try {
    for(let i=0;i<frames.length;i++)assert.equal(await decodeRpcFrame(frames[i],65536,decoder),texts[i]);
    assert(frames.reduce((n,frame)=>n+frame.length,0)*2 < texts.reduce((n,text)=>n+gzipSync(text).length,0));
  } finally {decoder.close();}
});

test('stateful decoded cap, dictionary isolation and cancellation fail closed',async()=>{
  const {createRpcDecoder}=await import('data:text/javascript;base64,'+source.toString('base64'));
  const frames=await statefulFrames(['x'.repeat(10000),'x'.repeat(10000)]);
  const decoder=createRpcDecoder();
  await assert.rejects(decodeRpcFrame(frames[0],100,decoder),/message limit/);
  await assert.rejects(decodeRpcFrame(frames[0],20000,decoder),/closed/);
  const fresh=createRpcDecoder();
  try {await assert.rejects(decodeRpcFrame(frames[1],20000,fresh));} finally {fresh.close();}
  const canceled=createRpcDecoder();canceled.close();
  await assert.rejects(decodeRpcFrame(frames[0],20000,canceled),/closed/);
});


test('truncated stateful frame has a bounded decode lifetime',async()=>{
  const {createRpcDecoder}=await import('data:text/javascript;base64,'+source.toString('base64'));
  const frames=await statefulFrames(['bounded text '.repeat(100)]);
  const decoder=createRpcDecoder({timeoutMs:10});
  await assert.rejects(decodeRpcFrame(frames[0].subarray(0,10),65536,decoder),/Truncated|closed/);
  decoder.close();
});
