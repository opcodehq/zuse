// Disposable engine/browser ownership shared by end-to-end acceptance scenarios.
// Private launch links stay in the temporary profile's private log.
import fs from 'node:fs';
import path from 'node:path';
import {spawn, execFileSync} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import assert from 'node:assert/strict';

export const workspace = process.cwd();
export const sleep = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
export function fixture(name) { return path.join(workspace, 'scripts/acceptance/fixtures', name); }
export function artifact(name, value) {
  const file = path.join(workspace, '.context', name);
  fs.writeFileSync(file, JSON.stringify(value, null, 2) + '\n');
}
export function rpc(socket) {
  let next = 1;
  const pending = new Map();
  socket.addEventListener('message', event => {
    const frame = JSON.parse(event.data);
    const call = pending.get(frame.id);
    if (!call) return;
    if (frame.err || Object.hasOwn(frame, 'ok') || Object.hasOwn(frame, 'item')) {
      pending.delete(frame.id); clearTimeout(call.timer);
      if (frame.err) call.reject(new Error(String(frame.err)));
      else {
        if (Object.hasOwn(frame, 'item')) socket.send(JSON.stringify({id:frame.id,cancel:true}));
        call.resolve(frame.item ?? frame.ok);
      }
    }
  });
  socket.addEventListener('close', () => {
    for (const call of pending.values()) { clearTimeout(call.timer); call.reject(new Error('Engine connection closed')); }
    pending.clear();
  });
  return (method, params = {}) => new Promise((resolve, reject) => {
    const id = next++;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 30000);
    pending.set(id, {resolve, reject, timer});
    socket.send(JSON.stringify({id, method, params}));
  });
}
export async function engine(prefix, options = {}) {
  const root = options.profileRoot ?? path.join(workspace, '.context', `${prefix}-runtime-${randomUUID()}`);
  fs.mkdirSync(root, {mode:0o700,recursive:true});
  const log = path.join(root, options.profileRoot ? `server-${randomUUID()}.private.log` : 'server.private.log');
  const descriptor = fs.openSync(log, 'a', 0o600);
  const child = spawn(options.binary ?? path.join(workspace, 'target/debug/zuse-server'), [
    '--listen=127.0.0.1:0','--ipc-port=0',`--data-dir=${path.join(root,'profile')}`,`--assets=${path.join(workspace,'dist/web')}`,
  ], {stdio:['ignore',descriptor,descriptor], env:{...process.env,...options.env,RUST_LOG:'error'}});
  fs.closeSync(descriptor);
  let socket;
  const close = async () => {
    socket?.close();
    if (child.exitCode === null && child.signalCode === null) {
      child.kill('SIGTERM');
      for (let n=0;n<100 && child.exitCode===null && child.signalCode===null;n++) await sleep(100);
      assert.ok(child.exitCode!==null || child.signalCode!==null,'Disposable engine did not stop');
    }
  };
  try {
    let url, port;
    for (let n=0;n<200;n++) {
      assert.equal(child.exitCode,null,'Disposable engine exited');
      const output = fs.readFileSync(log,'utf8');
      url = output.match(/Open (http:\/\/[^\s]+)/)?.[1];
      port = output.match(/Agent tools listen on 127\.0\.0\.1:(\d+)/)?.[1];
      if (url && port) break;
      await sleep(100);
    }
    assert.ok(url && port,'Engine startup deadline');
    socket = await new Promise((resolve,reject) => {
      const connection = new WebSocket(`ws://127.0.0.1:${port}`);
      connection.onopen = () => resolve(connection);
      connection.onerror = () => reject(new Error('Engine connection failed'));
    });
    return {root,url,port:Number(port),call:rpc(socket),close};
  } catch(error) { await close(); throw error; }
}
export function browser(prefix) {
  const session = execFileSync('agent-browser',['session','id','--scope','worktree','--prefix',prefix],{encoding:'utf8'}).trim();
  function run(args, input) {
    let result;
    try { result = JSON.parse(execFileSync('agent-browser',['--session',session,'--json',...args],{input,encoding:'utf8',maxBuffer:2*1024*1024})); }
    catch { throw new Error(`Browser ${args[0]} failed`); }
    if (Array.isArray(result)) {
      if (result.some(item => !item.success)) throw new Error('Browser batch action failed');
      return result.map(item => item.data);
    }
    if (!result.success) throw new Error('Browser action failed');
    return result.data;
  }
  return {
    session,
    run,
    evaluate: source => run(['eval','--stdin'],source).result,
    click(x,y) { run(['mouse','move',String(Math.round(x)),String(Math.round(y))]); run(['mouse','down']); run(['mouse','up']); },
    wait: condition => run(['wait','--fn',condition]),
    screenshot: name => run(['screenshot',path.join(workspace,'.context',name)]),
    open(url, init) {
      run(['--headed','--webgpu',...(init?['--init-script',init]:[]),'open',url]);
      run(['set','viewport','1280','1000']);
    },
    close() { try { run(['close']); } catch {} },
  };
}
