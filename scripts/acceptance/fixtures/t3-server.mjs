// Starts only the benchmark-owned shipped T3 build and disposable data.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {sleep} from '../harness.mjs';

export async function t3Server(root, {provider, workspace, template='.context/t3-benchmark/profile'}={}) {
  const binary=path.resolve(process.env.T3_BENCH_BINARY ?? '.context/t3-benchmark/home/runtime/versions/0.1.7/t3');
  assert(fs.existsSync(binary),'Install the disposable T3 benchmark binary first');
  const base=path.join(root,'t3');
  assert(!fs.existsSync(base),'T3 benchmark data must be new');
  fs.cpSync(path.resolve(template),base,{recursive:true});
  const config=path.join(root,'claude');fs.mkdirSync(config,{recursive:true});
  const settings={providerInstances:{
    claudeAgent:{driver:'claudeAgent',enabled:true,config:{binaryPath:provider,homePath:config}},
    codex:{driver:'codex',enabled:false,config:{}},
  },textGenerationModelSelection:{instanceId:'claudeAgent',model:'claude-sonnet-4-6'},compactIdleThreads:'off',idleCompaction:'off'};
  fs.writeFileSync(path.join(base,'userdata/settings.json'),JSON.stringify(settings));
  const allocator=net.createServer();await new Promise(resolve=>allocator.listen(0,'127.0.0.1',resolve));
  const port=allocator.address().port;await new Promise(resolve=>allocator.close(resolve));
  const fd=fs.openSync(path.join(root,'t3-server.private.log'),'w',0o600);
  const child=spawn(binary,['serve','--host','127.0.0.1','--port',String(port),'--base-dir',base,'--log-level','warn',workspace],{cwd:root,
    env:{...process.env,PATH:'/usr/bin:/bin',T3CODE_HOME:base,T3CODE_DEV_AUTH_TOKEN:'benchmark-private-token-'+'a'.repeat(40),CLAUDE_CONFIG_DIR:config,CODEX_HOME:path.join(root,'codex'),T3CODE_NO_TELEMETRY:'1',ZUSE_MATCHED_PROVIDER_LOG:path.join(root,'provider-seen.jsonl')},stdio:['ignore',fd,fd]});
  fs.closeSync(fd);
  const origin=`http://127.0.0.1:${port}`;
  let cookies=[];
  let cookie='';
  const request=async (url,init={})=>fetch(origin+url,{...init,headers:{Cookie:cookie,Origin:origin,...init.headers}});
  const connections=new Set();
  const close=async()=>{for(const socket of connections)socket.close();if(child.exitCode===null&&child.signalCode===null){child.kill('SIGTERM');for(let i=0;i<100&&child.exitCode===null&&child.signalCode===null;i++)await sleep(100);assert(child.exitCode!==null||child.signalCode!==null,'T3 benchmark did not stop');}};
  try {
    for(let i=0;i<200;i++) {assert.equal(child.exitCode,null,'T3 benchmark exited');try {const r=await request('/api/auth/session');if(r.ok)break;}catch{}await sleep(100);}
    let credential;
    for(let i=0;i<200;i++) {
      credential=fs.readFileSync(path.join(root,'t3-server.private.log'),'utf8').match(/^Token:\s*(\S+)/m)?.[1];
      if(credential)break;await sleep(100);
    }
    assert(credential,'Disposable T3 startup pairing credential');
    const login=await request('/api/auth/browser-session',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({credential})});
    assert(login.ok,'Disposable benchmark login failed: '+login.status);
    cookies=login.headers.getSetCookie();cookie=cookies.map(value=>value.split(';')[0]).join('; ');
    const auth=await request('/api/auth/session');assert(auth.ok,'Disposable benchmark auth is valid');
    const connect=async()=>{
      const response=await request('/api/auth/websocket-ticket/rpc',{method:'POST'});
      assert(response.ok,'T3 websocket ticket is authorized: '+response.status+' '+(response.ok?'':await response.text()));
      const {ticket}=await response.json();
      const socket=new WebSocket(origin.replace('http:','ws:')+'/ws?orchestrationProtocol=2&wsTicket='+encodeURIComponent(ticket));
      await new Promise((resolve,reject)=>{socket.onopen=resolve;socket.onerror=reject;});
      connections.add(socket);socket.addEventListener('close',()=>connections.delete(socket));
      const pending=new Map();let serial=0;
      socket.addEventListener('close',()=>{for(const job of pending.values()){clearTimeout(job.timer);job.reject(Error('Disposable T3 connection closed'));}pending.clear();});
      socket.addEventListener('message',event=>{
        for(const frame of [].concat(JSON.parse(event.data))){
          if(frame._tag==='Ping'){socket.send(JSON.stringify({_tag:'Pong'}));continue;}
          const job=pending.get(String(frame.requestId));if(!job)continue;
          if(frame._tag==='Chunk') {for(const value of frame.values)job.item?.(value,frame);socket.send(JSON.stringify({_tag:'Ack',requestId:frame.requestId}));}
          if(frame._tag==='Exit'){pending.delete(String(frame.requestId));clearTimeout(job.timer);frame.exit?._tag==='Success'?job.resolve(frame.exit.value):job.reject(Error(JSON.stringify(frame.exit)));}
        }
      });
      return {socket,call:(tag,payload={},item)=>new Promise((resolve,reject)=>{
        const id=String(++serial);const timer=setTimeout(()=>{pending.delete(id);reject(Error(tag+' timed out'));},30000);
        pending.set(id,{resolve,reject,timer,item});socket.send(JSON.stringify({_tag:'Request',id,tag,payload,headers:[]}));
      })};
    };
    const rpc=await connect();
    return {root,base,binary,origin,cookies,request,connect,call:rpc.call,socket:rpc.socket,close,pid:child.pid};
  } catch(error){await close();throw error;}
}
