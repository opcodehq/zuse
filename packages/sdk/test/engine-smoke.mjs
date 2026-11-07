// Opt-in round-trip against real engine, CLI and MCP processes; no paid models.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { once } from 'node:events';
import { createZuseClient } from '../dist/index.js';

const repository = resolve(import.meta.dirname, '../../..');
const serverBinary = process.env.ZUSE_SERVER_BINARY || join(repository, 'target/debug/zuse-server');
const controlBinary = process.env.ZUSE_CONTROL_BINARY || join(repository, 'target/debug/zuse-control');
const root = await mkdtemp(join(tmpdir(), 'zuse-control-smoke-'));
const discoveryFile = join(root, 'profiles/local/zots/control.json');
let server;
let api;
async function start() {
  let output = '';
  server = spawn(serverBinary, ['--data-dir', root, '--listen', '127.0.0.1:0', '--ipc-port', '0'], { stdio: ['ignore', 'pipe', 'pipe'] });
  server.stdout.on('data', chunk => { output = (output + chunk).slice(-65536); });
  server.stderr.on('data', () => {});
  for (let attempt = 0; attempt < 200; attempt++) {
    if (server.exitCode !== null) throw new Error(`Test engine exited with code ${server.exitCode}`);
    const port = output.match(/Open http:\/\/127\.0\.0\.1:(\d+)/)?.[1];
    if (port) {
      const discovery = JSON.parse(await readFile(discoveryFile, 'utf8'));
      api = createZuseClient({ endpoint: `ws://127.0.0.1:${port}/api/control`, token: discovery.token });
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
    child.on('exit', code => { if (responses.length < 2) reject(new Error(`MCP ended early: ${code}`)); });
    child.stdout.on('data', chunk => {
      buffer += chunk;
      while (buffer.includes('\n')) {
        const newline = buffer.indexOf('\n');
        responses.push(JSON.parse(buffer.slice(0, newline)));
        buffer = buffer.slice(newline + 1);
      }
      if (responses.length >= 2) resolve();
    });
  });
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: {} }) + '\n');
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }) + '\n');
  const timer = setTimeout(() => child.kill('SIGKILL'), 10000);
  try {
    await probe;
    const catalog = responses.find(response => response.id === 2).result.tools;
    assert.ok(catalog.some(tool => tool.name === 'ZotCommand' && tool.inputSchema.properties.commandId));
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
  const initial = await api.zots.command(create);
  await api.zots.command({ commandId: 'smoke-notes', zotId: 'smoke', expectedRevision: null, action: { type: 'writeFile', path: 'NOTES.md', content: 'Keep this across restart', expectedVersion: 1 } });
  assert.deepEqual(await api.zots.command(create), initial);
  await assert.rejects(api.zots.command({ ...create, action: { type: 'delete' } }));
  const files = JSON.parse(execFileSync(controlBinary, ['--discovery', discoveryFile, 'call', 'ZotFiles', '{"zotId":"smoke"}'], { encoding: 'utf8' }));
  assert.equal(files.find(file => file.path === 'NOTES.md').content, 'Keep this across restart');
  await mcpProbe();
  await stop();
  await start();
  assert.equal((await api.zots.files('smoke')).find(file => file.path === 'NOTES.md').version, 2);
  assert.deepEqual(await api.zots.command(create), initial);
  console.log('SDK, CLI and MCP engine smoke passed, including restart and command replay.');
} finally {
  await stop();
  await rm(root, { recursive: true, force: true });
}
