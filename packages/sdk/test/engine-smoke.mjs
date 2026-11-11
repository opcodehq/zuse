// Opt-in round-trip against real engine, CLI and MCP processes; no paid models.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { once } from 'node:events';
import { createZuseClient } from '../dist/index.js';

const repository = resolve(import.meta.dirname, '../../..');
const serverBinary = process.env.ZUSE_SERVER_BINARY || join(repository, 'target/debug/zuse-cli');
// A binary still named zuse-server takes the serve flags directly.
const serveCommand = basename(serverBinary).startsWith('zuse-server') ? [] : ['serve'];
const controlBinary = process.env.ZUSE_CONTROL_BINARY || join(repository, 'target/debug/zuse-control');
const root = await mkdtemp(join(tmpdir(), 'zuse-control-smoke-'));
const discoveryFile = join(root, 'profiles/local/zots/control.json');
let server;
let api;
let endpoint;
async function start() {
  let output = '';
  server = spawn(serverBinary, [...serveCommand, '--data-dir', root, '--listen', '127.0.0.1:0', '--ipc-port', '0'], { stdio: ['ignore', 'pipe', 'pipe'] });
  server.stdout.on('data', chunk => { output = (output + chunk).slice(-65536); });
  server.stderr.on('data', () => {});
  for (let attempt = 0; attempt < 200; attempt++) {
    if (server.exitCode !== null) throw new Error(`Test engine exited with code ${server.exitCode}`);
    const port = output.match(/Open http:\/\/127\.0\.0\.1:(\d+)/)?.[1];
    if (port) {
      const discovery = JSON.parse(await readFile(discoveryFile, 'utf8'));
      endpoint = `ws://127.0.0.1:${port}/api/control`;
      api = createZuseClient({ endpoint, token: discovery.token });
      await api.info({ signal: AbortSignal.timeout(10000) });
      return;
    }
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error('Test engine did not become ready');
}
async function stop() {
  api?.close();
  if (!server || server.exitCode !== null) return;
  const exited = once(server, 'exit');
  server.kill('SIGTERM');
  const timer = setTimeout(() => server.kill('SIGKILL'), 5000);
  await exited;
  clearTimeout(timer);
}
async function mcpProbe() {
  const child = spawn(controlBinary, ['--discovery', discoveryFile, 'mcp'], { stdio: ['pipe', 'pipe', 'pipe'] });
  let buffer = '';
  const responses = [];
  child.stderr.on('data', () => {});
  const probe = new Promise((resolve, reject) => {
    child.on('error', reject);
    child.on('exit', code => { if (responses.length < 3) reject(new Error(`MCP ended early: ${code}`)); });
    child.stdout.on('data', chunk => {
      buffer += chunk;
      while (buffer.includes('\n')) {
        const newline = buffer.indexOf('\n');
        responses.push(JSON.parse(buffer.slice(0, newline)));
        buffer = buffer.slice(newline + 1);
      }
      if (responses.length >= 3) resolve();
    });
  });
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: {} }) + '\n');
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }) + '\n');
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: 'ZotFiles', arguments: { zotId: 'smoke' } } }) + '\n');
  const timer = setTimeout(() => child.kill('SIGKILL'), 10000);
  try {
    await probe;
    const catalog = responses.find(response => response.id === 2).result.tools;
    assert.ok(catalog.some(tool => tool.name === 'ZotCommand' && tool.inputSchema.properties.commandId));
    const files = JSON.parse(responses.find(response => response.id === 3).result.content[0].text);
    assert.equal(files.find(file => file.path === 'NOTES.md').content, 'Keep this across restart');
    assert.ok(!catalog.some(tool => tool.name === 'UpdateStatus' || tool.name.startsWith('Watch') || tool.name === 'SubscribeTerminal'));
  } finally {
    clearTimeout(timer);
    child.stdin.end();
    if (child.exitCode === null && child.signalCode === null) await once(child, 'exit');
  }
}
try {
  await start();
  const info = await api.call('EngineInfo');
  const config = { name: 'Smoke', purpose: 'Test durable coordination', homeDeviceId: info.deviceId, repositories: [], coordinatorModel: { harness: 'mock', model: null }, workerDeviceId: info.deviceId, maxConcurrentWorkers: 4, maxFollowups: 3 };
  const create = { commandId: 'smoke-create', zotId: 'smoke', expectedRevision: null, action: { type: 'create', config } };
  const unauthorized = createZuseClient({ endpoint, token: 'invalid' });
  try {
    await assert.rejects(unauthorized.zots.command(create, { signal: AbortSignal.timeout(10000) }), error => error.code === 'unauthorized');
  } finally { unauthorized.close(); }
  const initial = await api.zots.command(create);
  await api.zots.command({ commandId: 'smoke-notes', zotId: 'smoke', expectedRevision: null, action: { type: 'writeFile', path: 'NOTES.md', content: 'Keep this across restart', expectedVersion: 1 } });
  assert.deepEqual(await api.zots.command(create), initial);
  await assert.rejects(api.zots.command({ ...create, action: { type: 'delete' } }));
  const files = JSON.parse(execFileSync(controlBinary, ['--discovery', discoveryFile, 'call', 'ZotFiles', '{"zotId":"smoke"}'], { encoding: 'utf8' }));
  assert.equal(files.find(file => file.path === 'NOTES.md').content, 'Keep this across restart');
  const watch = api.zots.watch({ signal: AbortSignal.timeout(10000) });
  assert.equal((await watch.next()).value.find(zot => zot.id === 'smoke').revision, 2);
  const decision = { id: 'approval', workerChatId: null, question: 'Ready to ship?', answer: null };
  const decide = await api.zots.command({ commandId: 'smoke-decision', zotId: 'smoke', expectedRevision: 2, action: { type: 'decide', decision } });
  assert.deepEqual(decide.decisions, [decision]);
  assert.equal((await watch.next()).value.find(zot => zot.id === 'smoke').revision, 3);
  await watch.return();
  await assert.rejects(api.zots.command({ commandId: 'smoke-stale', zotId: 'smoke', expectedRevision: 2, action: { type: 'setStatus', status: 'paused' } }));
  await assert.rejects(api.zots.command({ commandId: 'smoke-stale-file', zotId: 'smoke', expectedRevision: null, action: { type: 'writeFile', path: 'NOTES.md', content: 'stale', expectedVersion: 1 } }));
  await api.zots.command({ commandId: 'smoke-restore', zotId: 'smoke', expectedRevision: 3, action: { type: 'restoreFile', path: 'NOTES.md', version: 1, expectedVersion: 2 } });
  assert.equal((await api.zots.files('smoke')).find(file => file.path === 'NOTES.md').version, 3);
  assert.equal((await api.zots.files('smoke', true)).filter(file => file.path === 'NOTES.md').length, 3);
  await api.zots.command({ commandId: 'smoke-rewrite', zotId: 'smoke', expectedRevision: 4, action: { type: 'writeFile', path: 'NOTES.md', content: 'Keep this across restart', expectedVersion: 3 } });
  const fresh = await api.zots.command({ commandId: 'smoke-fresh', zotId: 'smoke', expectedRevision: 5, action: { type: 'freshConversation' } });
  assert.deepEqual(fresh.decisions, [decision]);
  assert.deepEqual(fresh.previousCoordinatorChats, [initial.coordinatorChatId]);
  assert.notEqual(fresh.coordinatorChatId, initial.coordinatorChatId);
  await mcpProbe();
  await stop();
  await start();
  assert.equal((await api.zots.files('smoke')).find(file => file.path === 'NOTES.md').version, 4);
  assert.deepEqual(await api.zots.command(create), initial);
  const afterRestart = api.zots.watch({ signal: AbortSignal.timeout(10000) });
  assert.deepEqual((await afterRestart.next()).value.find(zot => zot.id === 'smoke').decisions, [decision]);
  await afterRestart.return();
  const deletion = { commandId: 'smoke-delete', zotId: 'smoke', expectedRevision: 6, action: { type: 'delete' } };
  assert.equal(await api.zots.command(deletion), null);
  assert.equal(await api.zots.command(deletion), null);
  await assert.rejects(api.zots.files('smoke'));
  console.log('SDK, CLI and MCP engine smoke passed, including restart and command replay.');
} finally {
  await stop();
  await rm(root, { recursive: true, force: true });
}
