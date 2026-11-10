#!/usr/bin/env node
// Opt-in live lifecycle measurements. Never logs credentials or provider response bodies.
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { performance } from 'node:perf_hooks';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const args = process.argv.slice(2);
const value = (name, fallback) => {
  const index = args.indexOf(name);
  return index < 0 ? fallback : args[index + 1];
};
if (args.includes('--help')) {
  console.log('node scripts/cloud-provider-benchmark.mjs [--provider boxd|boat|all] [--live] [--sleep-ms 0..60000] [--boat-resume-type small|default|large] [--boxd-sdk /path/to/sdk/dist/web.js] [--boxd-rust-probe /path/to/provider_auth_probe] [--output .context/report.json]');
  process.exit(0);
}
const selected = value('--provider', 'all');
if (!['all', 'boxd', 'boat'].includes(selected)) throw new Error('Invalid provider');
const live = args.includes('--live');
const rustProbe = value('--boxd-rust-probe', undefined);
const runFile = promisify(execFile);
async function verifyRustBoxd(id) {
  const env = Object.fromEntries(['PATH', 'BOXD_API_KEY', 'BOXD_ORG', 'BOXD_BASE_URL'].filter(key => process.env[key] !== undefined).map(key => [key, process.env[key]]));
  const { stdout } = await runFile(resolve(rustProbe), ['--machine', id, '--expected-name', runId], { env, timeout: 30000, maxBuffer: 8192 });
  const proof = JSON.parse(stdout);
  if (proof.provider !== 'boxd' || proof.rustAdapterAuthenticated !== true || proof.machineIdentityVerified !== true || proof.execRoundTripVerified !== true) throw new Error('RustBoxdProofInvalid');
  return proof;
}
const sleepMs = Number(value('--sleep-ms', '0'));
if (!Number.isInteger(sleepMs) || sleepMs < 0 || sleepMs > 60000) throw new Error('Invalid sleep duration');
const boatResumeType = value('--boat-resume-type', undefined);
if (boatResumeType !== undefined && !['small', 'default', 'large'].includes(boatResumeType)) throw new Error('Invalid Boat resume type');
const runId = `zuse-bench-${Date.now()}-${randomUUID().slice(0, 8)}`;
const output = resolve(value('--output', `.context/${runId}.json`));
const report = { schemaVersion: 1, runId, startedAt: new Date().toISOString(), live, sleepMs, boatResumeType, providers: [] };
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const elapsed = start => Math.round((performance.now() - start) * 100) / 100;
const quote = text => `'${text.replaceAll("'", "'\\''")}'`;
const persist = async () => {
  await mkdir(resolve(output, '..'), { recursive: true });
  await writeFile(output, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
};
class ProviderStateDeadline extends Error {
  constructor(polls, states) {
    super('ProviderStateDeadline');
    this.name = 'ProviderStateDeadline';
    this.polls = polls;
    this.states = states;
  }
}
const networkCodes = new Set(['UND_ERR_CONNECT_TIMEOUT', 'ECONNRESET', 'ECONNREFUSED', 'ENOTFOUND', 'EAI_AGAIN', 'ETIMEDOUT']);
const safeError = error => ({ name: error.name, status: typeof error.status === 'number' ? error.status : null,
  ...(networkCodes.has(error.cause?.code) ? { networkCode: error.cause.code } : {}),
  ...(error instanceof ProviderStateDeadline ? { polls: error.polls, states: error.states } : {}) });
async function timed(result, name, task) {
  const start = performance.now();
  try {
    const response = await task();
    result.stages.push({ name, elapsedMs: elapsed(start), ok: true });
    await persist();
    return response;
  } catch (error) {
    result.stages.push({ name, elapsedMs: elapsed(start), ok: false, error: safeError(error) });
    await persist();
    throw error;
  }
}

// A disk marker plus a Unix-socket server whose only copy of the response is in RAM.
// Checking the socket proves actual process continuity, not merely PID reuse.
const memory = randomUUID();
const server = `import socket,os,sys,pathlib\nr=pathlib.Path.home()/${JSON.stringify(runId)};r.mkdir();r.joinpath('disk').write_text(${JSON.stringify(runId)})\npid=os.fork()\nif pid:\n os.waitpid(pid,0);sys.exit(0)\nos.setsid()\nif os.fork(): os._exit(0)\nf=os.open('/dev/null',os.O_RDWR)\nfor d in (0,1,2): os.dup2(f,d)\ns=socket.socket(socket.AF_UNIX)\ns.bind(str(r/'memory.sock'))\ns.listen(4)\nwhile True:\n c,_=s.accept()\n c.sendall(${JSON.stringify(memory)}.encode())\n c.close()`;
const setup = `python3 -c ${quote(server)}`;
const probe = `python3 -c ${quote(`import socket,json,pathlib\nr=pathlib.Path.home()/${JSON.stringify(runId)}\nresult={'diskPreserved':r.joinpath('disk').is_file() and r.joinpath('disk').read_text()==${JSON.stringify(runId)},'memoryPreserved':False,'bootId':pathlib.Path('/proc/sys/kernel/random/boot_id').read_text().strip()}\ntry:\n s=socket.socket(socket.AF_UNIX);s.settimeout(2);s.connect(str(r/'memory.sock'));result['memoryPreserved']=s.recv(1000).decode()==${JSON.stringify(memory)};s.close()\nexcept OSError: pass\nprint(json.dumps(result))`)}`;

async function waitState(adapter, id, states, deadlineMs = 90000) {
  const start = performance.now();
  let polls = 0;
  const observations = {};
  while (performance.now() - start < deadlineMs) {
    const state = await adapter.state(id);
    polls += 1;
    // Keep bounded enum observations, never provider response bodies or credentials.
    if (typeof state !== 'string' || !/^[a-z-]{1,32}$/.test(state)) throw new Error('InvalidProviderState');
    if (Object.hasOwn(observations, state) || Object.keys(observations).length < 16) {
      observations[state] = (observations[state] ?? 0) + 1;
    }
    if (states.includes(state)) return { state, polls };
    if (['failed', 'error', 'destroyed'].includes(state)) throw new Error('ProviderTerminalState');
    await sleep(200);
  }
  throw new ProviderStateDeadline(polls, observations);
}

async function boat(observe = () => {}) {
  if (!process.env.BOAT_API_KEY) throw new Error('MissingBoatKey');
  const base = 'https://boat.dev/api/v1';
  async function request(method, suffix, body, extra = {}) {
    const response = await fetch(base + suffix, {
      method,
      headers: { Authorization: `Bearer ${process.env.BOAT_API_KEY}`, 'content-type': 'application/json', ...extra },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(45000),
      redirect: 'error',
    });
    if (!response.ok) {
      await response.body?.cancel();
      const error = new Error('BoatRequestFailed');
      error.status = response.status;
      throw error;
    }
    if (response.status === 204) return {};
    return response.json();
  }
  let stopId;
  function observation(response) {
    const sandbox = response.sandbox;
    const operation = response.stop ?? sandbox?.stop;
    let stop;
    if (operation != null) {
      if (typeof operation.id !== 'string' || !/^[a-zA-Z0-9_-]{1,128}$/.test(operation.id)
          || !['pending', 'failing', 'completed', 'superseded'].includes(operation.status)) {
        const error = new Error('BoatStopInvalid'); error.name = 'BoatStopInvalid'; throw error;
      }
      stop = { id: operation.id, status: operation.status,
        errorPresent: operation.error != null, forced: operation.forced === true };
    }
    observe({ state: typeof sandbox?.state === 'string' && /^[a-z-]{1,32}$/.test(sandbox.state) ? sandbox.state : null,
      stop, snapshotStatus: ['queued', 'in_progress', 'completed', 'failed', 'cancelled'].includes(sandbox?.lastSnapshotStatus) ? sandbox.lastSnapshotStatus : null });
    if (stopId !== undefined && stop?.id !== stopId) {
      const error = new Error('BoatStopOperationChanged'); error.name = 'BoatStopOperationChanged'; throw error;
    }
    return stop;
  }
  return {
    auth: async () => { await request('GET', '/sandboxes'); },
    create: async () => (await request('POST', '/sandboxes', { name: runId, type: 'small', ttlSeconds: 600, noEnv: true }, { 'Idempotency-Key': runId })).sandbox.id,
    state: async id => {
      const response = await request('GET', `/sandboxes/${encodeURIComponent(id)}`);
      if (stopId !== undefined && response.stop == null && response.sandbox?.stop == null) {
        // The sandbox's latest-operation field can be absent; read our operation by ID.
        const operation = await request('GET', `/sandboxes/${encodeURIComponent(id)}/stops/${encodeURIComponent(stopId)}`);
        response.stop = operation.stop;
      }
      const stop = observation(response);
      if (stopId !== undefined && ['failing', 'superseded'].includes(stop?.status)) {
        const name = stop.status === 'failing' ? 'BoatStopFailing' : 'BoatStopSuperseded';
        const error = new Error(name); error.name = name; throw error;
      }
      if (stopId !== undefined && response.sandbox.state === 'archived' && stop?.status !== 'completed') {
        // An archived state alone cannot complete a known pending stop operation.
        return 'archiving';
      }
      return response.sandbox.state;
    },
    running: ['idle', 'ready', 'running'], sleeping: ['archived', 'stopped'],
    exec: async (id, command) => {
      const result = await request('POST', `/sandboxes/${encodeURIComponent(id)}/commands`, { command, timeoutSeconds: 10 });
      if (result.exitCode !== 0 || result.timedOut) throw new Error('BoatCommandFailed');
      return result.stdout;
    },
    pause: async id => {
      const response = await request('POST', `/sandboxes/${encodeURIComponent(id)}/stop`, {});
      stopId = observation(response)?.id;
      if (stopId === undefined) {
        const error = new Error('BoatStopOperationMissing'); error.name = 'BoatStopOperationMissing'; throw error;
      }
      return undefined; // Do not retain arbitrary provider response/error bodies.
    },
    resume: async id => {
      // Successful confirmation ended this observation; a new resume supersedes it normally.
      stopId = undefined;
      await request('POST', `/sandboxes/${encodeURIComponent(id)}/resume`, { ttlSeconds: 600, ...(boatResumeType === undefined ? {} : { type: boatResumeType }) });
    },
    remove: id => request('DELETE', `/sandboxes/${encodeURIComponent(id)}`, undefined, { 'X-Ascii-Confirm-Delete': id }),
    absent: async id => {
      try { await request('GET', `/sandboxes/${encodeURIComponent(id)}`); return false; }
      catch (error) { if (error.status === 404) return true; throw error; }
    },
    recover: async () => {
      let cursor;
      for (let page = 0; page < 20; page += 1) {
        const data = await request('GET', `/sandboxes${cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''}`);
        const owned = data.sandboxes.filter(sandbox => sandbox.name === runId);
        if (owned.length > 1) throw new Error('AmbiguousOwnedResource');
        if (owned.length === 1) return owned[0].id;
        cursor = data.pageInfo?.nextCursor;
        if (!cursor) return null;
      }
      throw new Error('RecoveryPageLimit');
    },
    close: async () => {},
  };
}

async function boxd() {
  if (!process.env.BOXD_API_KEY) throw new Error('MissingBoxdKey');
  const sdkPath = value('--boxd-sdk', process.env.ZUSE_BENCH_BOXD_SDK);
  const { Boxd } = await import(sdkPath ? pathToFileURL(resolve(sdkPath)).href : '@boxd-sh/sdk/web');
  const sdk = new Boxd({ apiKey: process.env.BOXD_API_KEY, timeout: 45000, maxRetries: 0 });
  return {
    auth: async () => { await sdk.machines.list(); },
    create: async () => (await sdk.machines.create({ name: runId, isolated: true, config: { vcpu: 1, memory: '4G', autoDestroyTimeout: 600, autoSuspendTimeout: 0 } })).id,
    state: async id => (await sdk.machines.get(id)).status,
    running: ['running'], sleeping: ['hibernated'],
    exec: async (id, command) => {
      const result = await sdk.machines.exec(id, { command, timeout: 15000 });
      if (!result.success) throw new Error('BoxdCommandFailed');
      return result.stdout;
    },
    pause: id => sdk.machines.hibernate(id),
    resume: id => sdk.machines.wake(id),
    remove: id => sdk.machines.delete(id),
    absent: async id => {
      try { await sdk.machines.get(id); return false; }
      catch (error) { if (error.name === 'NotFoundError') return true; throw error; }
    },
    recover: async () => {
      try { return (await sdk.machines.get(runId)).id; }
      catch (error) { if (error.name === 'NotFoundError') return null; throw error; }
    },
    close: () => sdk.close(),
  };
}

for (const provider of selected === 'all' ? ['boxd', 'boat'] : [selected]) {
  const result = { provider, stages: [], resourceName: runId, resourceId: null, cleanup: 'not-needed' };
  report.providers.push(result);
  let adapter;
  try {
    adapter = await (provider === 'boxd' ? boxd() : boat(observation => {
      if (observation.stop) result.stopOperation = observation.stop;
      const transitions = result.providerTransitions ??= [];
      const previous = transitions.at(-1);
      const previousObservation = previous && { state: previous.state, stop: previous.stop, snapshotStatus: previous.snapshotStatus };
      if (transitions.length < 64 && JSON.stringify(previousObservation) !== JSON.stringify(observation)) {
        transitions.push({ elapsedMs: Date.now() - Date.parse(report.startedAt), ...observation });
      }
    }));
    await timed(result, 'authenticate-read', adapter.auth);
    if (!live) continue;
    result.cleanup = 'create-outcome-unknown';
    await persist();
    result.resourceId = await timed(result, 'create-response', adapter.create);
    if (typeof result.resourceId !== 'string' || !result.resourceId) {
      result.resourceId = null;
      throw new Error('MissingResourceIdentity');
    }
    result.cleanup = 'pending';
    await persist();
    const createdAt = performance.now();
    result.createdState = await timed(result, 'create-running-state', () => waitState(adapter, result.resourceId, adapter.running));
    await timed(result, 'create-exec-ready', () => adapter.exec(result.resourceId, 'true'));
    result.createResponseToExecReadyMs = elapsed(createdAt);
    if (provider === 'boxd' && rustProbe) result.rustAfterCreate = await timed(result, 'rust-adapter-create-exec', () => verifyRustBoxd(result.resourceId));
    await timed(result, 'install-continuity-probe', () => adapter.exec(result.resourceId, setup));
    await sleep(250);
    result.before = JSON.parse(await adapter.exec(result.resourceId, probe));
    if (!result.before.diskPreserved || !result.before.memoryPreserved) throw new Error('ContinuityProbeNotReady');
    await timed(result, provider === 'boat' ? 'stop-response' : 'hibernate-response', () => adapter.pause(result.resourceId));
    result.sleepState = await timed(result, 'sleep-state-confirmed', () => waitState(adapter, result.resourceId, adapter.sleeping));
    if (sleepMs) await timed(result, 'sleep-dwell', () => sleep(sleepMs));
    const resumedAt = performance.now();
    await timed(result, 'resume-response', () => adapter.resume(result.resourceId));
    result.resumedState = await timed(result, 'resume-running-state', () => waitState(adapter, result.resourceId, adapter.running));
    result.after = JSON.parse(await timed(result, 'resume-exec-ready', () => adapter.exec(result.resourceId, probe)));
    result.resumeToExecReadyMs = elapsed(resumedAt);
    if (provider === 'boxd' && rustProbe) result.rustAfterResume = await timed(result, 'rust-adapter-resume-exec', () => verifyRustBoxd(result.resourceId));
    result.bootIdPreserved = result.before.bootId === result.after.bootId;
    if (!result.after.diskPreserved) throw new Error('DiskContinuityLost');
  } catch (error) {
    result.error = safeError(error);
    process.exitCode = 1;
  } finally {
    if (adapter && !result.resourceId && result.cleanup === 'create-outcome-unknown') {
      try {
        result.resourceId = await timed(result, 'recover-create-by-exact-name', adapter.recover);
        // An immediate not-found cannot prove that a timed-out create will never finish.
        if (!result.resourceId) result.cleanup = 'unresolved-create-outcome';
      } catch (error) { result.cleanupError = safeError(error); }
    }
    if (adapter && result.resourceId) {
      try {
        await timed(result, 'delete-owned-resource', () => adapter.remove(result.resourceId));
        if (!await adapter.absent(result.resourceId)) throw new Error('DeletionNotConfirmed');
        result.cleanup = 'confirmed-absent';
      } catch (error) {
        result.cleanup = 'failed';
        result.cleanupError = safeError(error);
        process.exitCode = 1;
      }
    }
    await adapter?.close();
    await persist();
    console.log(JSON.stringify({ provider, authenticated: result.stages.some(stage => stage.name === 'authenticate-read' && stage.ok), resourceId: result.resourceId, cleanup: result.cleanup, error: result.error ?? null }));
  }
}
report.finishedAt = new Date().toISOString();
await persist();
console.log(`Report: ${output}`);
