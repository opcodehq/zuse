import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

async function fixture(live, failCreate = false, rustProbeFailure = false) {
  const directory = await mkdtemp(join(tmpdir(), 'zuse-provider-benchmark-'));
  const sdk = join(directory, 'sdk.mjs');
  const log = join(directory, 'calls.jsonl');
  const output = join(directory, 'report.json');
  const probe = join(directory, 'probe.mjs');
  await writeFile(probe, `#!/usr/bin/env node\nimport {appendFileSync} from 'node:fs';\nappendFileSync(${JSON.stringify(log)},JSON.stringify('rust-probe')+'\\n');\nconsole.error(process.env.BOXD_API_KEY);process.exit(1);\n`, { mode: 0o700 });
  await writeFile(sdk, `
import {appendFileSync} from 'node:fs';
const log = name => appendFileSync(${JSON.stringify(log)},JSON.stringify(name)+'\\n');
export class Boxd {
 constructor() {
  let removed = false;
  this.machines = {
   list: async()=>{log('authenticate');return []},
   create: async()=>{log('create');if(process.env.TEST_FAIL_CREATE)throw new Error(process.env.BOXD_API_KEY);return {id:'owned-test-resource'}},
   get: async()=>{log('get');if(removed){const e=new Error('absent');e.name='NotFoundError';throw e}return {id:'owned-test-resource',status:'running'}},
   exec: async()=>{log('exec');if(process.env.TEST_EXEC_SUCCESS)return {success:true,stdout:''};throw new Error(process.env.BOXD_API_KEY)},
   delete: async(id)=>{if(id!=='owned-test-resource')throw new Error('wrong-resource');log('delete');removed=true},
  };
 }
 async close(){log('close')}
}
`);
  const run = spawnSync(process.execPath, ['scripts/cloud-provider-benchmark.mjs', '--provider', 'boxd', '--boxd-sdk', sdk, '--output', output, ...(live ? ['--live'] : []), ...(rustProbeFailure ? ['--boxd-rust-probe', probe] : [])], {
    cwd: process.cwd(), encoding: 'utf8', timeout: 10000,
    env: { ...process.env, BOXD_API_KEY: 'synthetic-secret-never-log', TEST_FAIL_CREATE: failCreate ? '1' : '', TEST_EXEC_SUCCESS: rustProbeFailure ? '1' : '' },
  });
  const report = JSON.parse(await readFile(output, 'utf8'));
  const calls = (await readFile(log, 'utf8')).trim().split('\n').map(JSON.parse);
  return { directory, run, report, calls };
}

test('default mode authenticates without creating or modifying machines', async () => {
  const result = await fixture(false);
  try {
    assert.equal(result.run.status, 0);
    assert.deepEqual(result.calls, ['authenticate', 'close']);
    assert.equal(result.report.live, false);
    assert.equal(result.report.providers[0].resourceId, null);
  } finally { await rm(result.directory, { recursive: true }); }
});

test('ambiguous create is reconciled by unique name and never replayed', async () => {
  const result = await fixture(true, true);
  try {
    assert.equal(result.run.status, 1);
    assert.deepEqual(result.calls, ['authenticate', 'create', 'get', 'delete', 'get', 'close']);
    assert.equal(result.report.providers[0].cleanup, 'confirmed-absent');
    assert.ok(result.report.providers[0].stages.some(stage => stage.name === 'recover-create-by-exact-name' && stage.ok));
    assert.ok(!JSON.stringify(result.report).includes('synthetic-secret-never-log'));
  } finally { await rm(result.directory, { recursive: true }); }
});

test('failure after allocation deletes only the owned resource and redacts error text', async () => {
  const result = await fixture(true);
  try {
    assert.equal(result.run.status, 1);
    assert.deepEqual(result.calls, ['authenticate', 'create', 'get', 'exec', 'delete', 'get', 'close']);
    assert.equal(result.report.providers[0].cleanup, 'confirmed-absent');
    assert.equal(result.report.providers[0].resourceId, 'owned-test-resource');
    assert.ok(!JSON.stringify(result.report).includes('synthetic-secret-never-log'));
    assert.ok(!result.run.stdout.includes('synthetic-secret-never-log'));
    assert.ok(!result.run.stderr.includes('synthetic-secret-never-log'));
  } finally { await rm(result.directory, { recursive: true }); }
});

test('Rust wire-probe failure cleans up the machine without logging child stderr', async () => {
  const result = await fixture(true, false, true);
  try {
    assert.equal(result.run.status, 1);
    assert.deepEqual(result.calls, ['authenticate', 'create', 'get', 'exec', 'rust-probe', 'delete', 'get', 'close']);
    assert.equal(result.report.providers[0].cleanup, 'confirmed-absent');
    assert.ok(result.report.providers[0].stages.some(stage => stage.name === 'rust-adapter-create-exec' && !stage.ok));
    assert.ok(!JSON.stringify(result.report).includes('synthetic-secret-never-log'));
    assert.ok(!result.run.stdout.includes('synthetic-secret-never-log'));
    assert.ok(!result.run.stderr.includes('synthetic-secret-never-log'));
  } finally { await rm(result.directory, { recursive: true }); }
});

async function boatStopFixture(stopStatus, state = 'archiving', omitLatest = false) {
  const directory = await mkdtemp(join(tmpdir(), 'zuse-boat-stop-'));
  const shim = join(directory, 'fetch.mjs');
  const output = join(directory, 'report.json');
  await writeFile(shim, `
import {performance} from 'node:perf_hooks';
let clock=0, stopped=false, removed=false;
Object.defineProperty(performance,'now',{value:()=>clock});
globalThis.fetch=async(url,options={})=>{
 const path=new URL(url).pathname, method=options.method??'GET';
 let value={};
 if(method==='DELETE'){removed=true;return new Response(null,{status:204})}
 if(method==='GET'&&path==='/api/v1/sandboxes')value={sandboxes:[]};
 else if(method==='POST'&&path==='/api/v1/sandboxes')value={sandbox:{id:'owned-boat'}};
 else if(path.endsWith('/commands'))value={exitCode:0,stdout:JSON.stringify({diskPreserved:true,memoryPreserved:true,bootId:'synthetic'})};
 else if(path.endsWith('/stop')){if(JSON.parse(options.body).force===true)throw new Error('unexpected force');stopped=true;value={stop:{id:'stop-test',status:'pending'},sandbox:{id:'owned-boat',state:'archiving'}}}
 else if(path.endsWith('/resume')){stopped=false;value={sandbox:{id:'owned-boat',state:'ready'}}}
 else if(path.endsWith('/stops/stop-test'))value={stop:{id:'stop-test',status:${JSON.stringify(stopStatus)},error:null}};
 else if(removed)return new Response('{}',{status:404});
 else if(stopped){clock+=100000;value={sandbox:{id:'owned-boat',state:${JSON.stringify(state)},stop:${omitLatest ? 'null' : `{id:'stop-test',status:${JSON.stringify(stopStatus)},error:'synthetic-secret-never-log'}`},lastSnapshotStatus:'failed'}}}
 else value={sandbox:{id:'owned-boat',state:'ready'}};
 return Response.json(value);
};
`);
  const run = spawnSync(process.execPath, ['--import', shim, 'scripts/cloud-provider-benchmark.mjs', '--provider', 'boat', '--live', '--output', output], {
    cwd: process.cwd(), encoding: 'utf8', timeout: 5000,
    env: { ...process.env, BOAT_API_KEY: 'synthetic-secret-never-log' },
  });
  return { directory, run, report: JSON.parse(await readFile(output, 'utf8')) };
}

test('a concurrent resume superseding Boat Stop is distinct from a snapshot failure', async () => {
  const result = await boatStopFixture('superseded');
  try {
    assert.equal(result.run.status, 1);
    const provider = result.report.providers[0];
    assert.equal(provider.error.name, 'BoatStopSuperseded');
    assert.equal(provider.stopOperation.status, 'superseded');
    assert.equal(provider.cleanup, 'confirmed-absent');
  } finally { await rm(result.directory, { recursive: true }); }
});

test('Boat stop failure is diagnosed from its operation instead of a blind state timeout', async () => {
  const result = await boatStopFixture('failing');
  try {
    assert.equal(result.run.status, 1);
    const provider = result.report.providers[0];
    assert.equal(provider.error.name, 'BoatStopFailing');
    assert.equal(provider.stopOperation.status, 'failing');
    assert.equal(provider.stopOperation.id, 'stop-test');
    assert.equal(provider.stopOperation.errorPresent, true);
    assert.ok(!provider.stages.some(stage => stage.name === 'resume-response'));
    assert.equal(provider.cleanup, 'confirmed-absent');
    assert.ok(!JSON.stringify(result.report).includes('synthetic-secret-never-log'));
    assert.ok(!result.run.stderr.includes('synthetic-secret-never-log'));
  } finally { await rm(result.directory, { recursive: true }); }
});


test('an archived state cannot confirm a still-pending Boat stop operation', async () => {
  const result = await boatStopFixture('pending', 'archived');
  try {
    const provider = result.report.providers[0];
    assert.equal(result.run.status, 1);
    assert.equal(provider.error.name, 'ProviderStateDeadline');
    assert.equal(provider.stopOperation.status, 'pending');
    assert.ok(!provider.stages.some(stage => stage.name === 'resume-response'));
    assert.equal(provider.cleanup, 'confirmed-absent');
  } finally { await rm(result.directory, { recursive: true }); }
});

test('a completed Boat stop operation and archived state allow resume', async () => {
  const result = await boatStopFixture('completed', 'archived');
  try {
    const provider = result.report.providers[0];
    assert.equal(result.run.status, 0, result.run.stderr);
    assert.equal(provider.stopOperation.status, 'completed');
    assert.ok(provider.stages.some(stage => stage.name === 'resume-response' && stage.ok));
    assert.equal(provider.cleanup, 'confirmed-absent');
  } finally { await rm(result.directory, { recursive: true }); }
});

test('Boat stop is read by operation ID when the latest field is absent', async () => {
  const result = await boatStopFixture('completed', 'archived', true);
  try {
    const provider = result.report.providers[0];
    assert.equal(result.run.status, 0, result.run.stderr);
    assert.equal(provider.stopOperation.id, 'stop-test');
    assert.equal(provider.stopOperation.status, 'completed');
    assert.equal(provider.cleanup, 'confirmed-absent');
  } finally { await rm(result.directory, { recursive: true }); }
});
