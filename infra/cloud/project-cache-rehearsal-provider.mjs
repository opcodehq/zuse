// Provider reads only. The pinned SDK uses typed Get/List RPCs; never execute,
// create, restore, stop, delete, snapshot or infer provider absence from SQL.
import {createHash} from 'node:crypto';
import {readFile, readdir, lstat, open} from 'node:fs/promises';
import {resolve, dirname, join} from 'node:path';
import {pathToFileURL} from 'node:url';

const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const require = (ok, code) => { if (!ok) throw Error(code); };
const identifier = value => typeof value === 'string' && /^[-a-zA-Z0-9_]{1,160}$/.test(value);
const digest = value => hash(JSON.stringify(value));
const PIN = '2cb65b3cc4c6c6bae7bc52e865227364ee186aeacc1bf999c3c1e16fceeb51aa';

function bound(bytes, expected) {
  require(/^[a-f0-9]{64}$/.test(expected) && hash(bytes) === expected, 'cache_provider_input_sri');
  return JSON.parse(bytes.toString());
}
function unique(values, code) {
  const rows = [...new Set(values.filter(v => v !== null && v !== undefined))];
  require(rows.length <= 1, code); return rows[0] ?? null;
}

export function targetsFor(packet, observations, tasks) {
  const targets = [], unresolved = [], protectedId = packet.bounds.protectedMachineId;
  const operations = new Map();
  for (const observation of observations) {
    for (const op of observation.operations) {
      require(/^cacheop_[a-f0-9]{32}$/.test(op.id), 'cache_provider_original_operation');
      const rows = operations.get(op.id) ?? []; rows.push(op); operations.set(op.id, rows);
    }
  }
  require(operations.size <= 8, 'cache_provider_operation_bound');
  for (const [operationId, rows] of operations) {
    const suffix = operationId.slice('cacheop_'.length);
    for (const [kind, field, effect, prefix] of [
      ['builder', 'builderId', 'allocate', 'pc-b-'],
      ['verifier', 'verifierId', 'verify', 'pc-v-'],
      ['snapshot', 'artifactId', 'capture', 'pc-s-'],
    ]) {
      const id = unique(rows.map(r => r.resources?.provider?.[field]), 'cache_provider_replaced_resource');
      for (const row of rows) {
        const intent = row.resources?.effects?.[effect];
        require(!intent || (typeof intent.requested === 'boolean'
          && (!intent.requested || intent.operationId === operationId + ':' + effect)), 'cache_provider_effect_binding');
      }
      const requested = rows.some(r => r.resources?.effects?.[effect]?.requested === true);
      require(id === null || identifier(id), 'cache_provider_resource_id');
      require(id === null || requested, 'cache_provider_resource_without_intent');
      require(id !== protectedId && id !== packet.pilot.baseArtifactId, 'cache_provider_protected_target');
      targets.push({kind, operationId, id, name: prefix + suffix, requested});
    }
  }
  const knownWorkspaces = new Set();
  for (const label of ['miss', 'hit', 'restore']) {
    const task = tasks[label]; require(task && typeof task.intentPresent === 'boolean', 'cache_provider_task_receipts');
    if (task.intentPresent) require(task.intent?.action === 'create-' + label && task.intent.method === 'POST'
      && task.intent.path === '/v1/cloud/workspaces' && task.intent.body?.projectId === packet.source.projectId
      && task.intent.body.providerId === 'boxd'
      && task.intent.body.idempotencyKey === packet.tasks?.[label]?.idempotencyKey, 'cache_provider_task_intent_scope');
    const w = task.outcome?.value?.workspace;
    if (!w) {
      if (task.intentPresent || task.outcome) unresolved.push({kind:'task', label, code:'original_admission_unresolved'});
      continue;
    }
    require(task.intentPresent && task.outcome.status >= 200 && task.outcome.status < 300, 'cache_provider_task_admission_receipt');
    require(w.projectId === packet.source.projectId && w.providerId === 'boxd' && identifier(w.workspaceId), 'cache_provider_task_scope');
    require(w.workspaceId !== packet.bounds.protectedWorkspaceId, 'cache_provider_protected_target');
    knownWorkspaces.add(w.workspaceId);
    const workspaceRows = observations.flatMap(o => o.workspaces).filter(r => r.id === w.workspaceId);
    const allocations = observations.flatMap(o => o.allocations ?? []).filter(r => r.workspaceId === w.workspaceId);
    const name = unique(allocations.map(r => r.machineName), 'cache_provider_replaced_task_name');
    const operationId = unique(allocations.map(r => r.operationId), 'cache_provider_replaced_task_operation');
    const id = unique([w.providerSandboxId, ...workspaceRows.map(r => r.machineId)], 'cache_provider_replaced_task_id');
    require(allocations.every(r => r.generation === 1), 'cache_provider_task_generation');
    require(id === null || identifier(id), 'cache_provider_resource_id');
    require(id !== protectedId && (!name || identifier(name)), 'cache_provider_protected_target');
    if (!name || !operationId) unresolved.push({kind:'task', label, workspaceId:w.workspaceId, code:'original_allocation_binding_missing'});
    else targets.push({kind:'task', label, operationId, workspaceId:w.workspaceId, id, name, requested:true});
  }
  for (const row of observations.flatMap(o => o.workspaces)) {
    if (!knownWorkspaces.has(row.id)) unresolved.push({kind:'task', workspaceId:row.id, code:'unbound_scoped_workspace'});
  }
  require(targets.length <= 27, 'cache_provider_target_bound');
  return {targets, unresolved};
}

function normalized(rows, kind) {
  require(Array.isArray(rows) && rows.length <= 2048, 'cache_provider_complete_inventory_required');
  const result = rows.map(row => {
    require(identifier(row.id) && identifier(row.name) && typeof row.status === 'string', 'cache_provider_inventory_shape');
    return {id:row.id, name:row.name, status:row.status, ...(kind === 'snapshot' ? {version:String(row.version)} : {})};
  }).sort((a,b) => a.id.localeCompare(b.id));
  require(new Set(result.map(r => r.id)).size === result.length && new Set(result.map(r => r.name)).size === result.length,
    'cache_provider_ambiguous_inventory');
  return result;
}

export async function inspectRehearsalProvider(input, {sdk, now = Date.now} = {}) {
  const packet = bound(input.packetBytes, input.packetSha256);
  require(packet.kind === 'boxd-project-cache-serial-rehearsal-v1' && packet.environment === 'staging'
    && packet.effectsAuthorized === false && /^[a-f0-9]{32}$/.test(packet.runId), 'cache_provider_packet_scope');
  require(packet.source.ownerId === packet.source.actorId && packet.source.projectId === packet.pilot.projectId
    && packet.source.ownerId === packet.pilot.ownerId && packet.pilot.baseArtifactId
    && identifier(packet.pilot.baseArtifactId) && /^[1-9][0-9]{0,9}$/.test(String(packet.pilot.baseArtifactVersion))
    && packet.source.ownerId === packet.bounds.ownerId && packet.source.projectId === packet.bounds.projectId
    && packet.bounds.protectedGeneration === 1 && packet.pilot.protectedMachineId === packet.bounds.protectedMachineId
    && packet.bounds.protectedMachineId !== packet.pilot.baseArtifactId, 'cache_provider_actor_scope');
  require(['inventory','preparation-absent','clean'].includes(input.phase), 'cache_provider_phase');
  require(input.observations.length >= 1 && input.observations.length <= 16, 'cache_provider_observation_bound');
  let latestObservedAt = 0;
  const observations = input.observations.map(row => {
    const value = bound(row.bytes, row.sha256);
    require(value.packetSha256 === input.packetSha256 && value.readOnly === true
      && Number.isSafeInteger(value.observedAtMs) && value.observedAtMs >= latestObservedAt
      && Array.isArray(value.observation?.operations) && Array.isArray(value.observation?.workspaces), 'cache_provider_observation_scope');
    latestObservedAt = value.observedAtMs;
    return value.observation;
  });
  const {targets, unresolved} = targetsFor(packet, observations, input.tasks);
  require(now() >= latestObservedAt && now() - latestObservedAt <= 60000, 'cache_provider_stale_observation');
  const baseline = input.baseline ? bound(input.baseline.bytes, input.baseline.sha256) : null;
  if (baseline) {
    require(baseline.kind === 'boxd-project-cache-provider-observation-v1' && baseline.runId === packet.runId
      && baseline.packetSha256 === input.packetSha256 && baseline.completeProviderInventory === true
      && baseline.protectedMachineId === packet.bounds.protectedMachineId
      && baseline.unrelatedInventory && Array.isArray(baseline.unrelatedInventory.machines)
      && Array.isArray(baseline.unrelatedInventory.snapshots)
      && digest(baseline.unrelatedInventory) === baseline.unrelatedFingerprint, 'cache_provider_baseline_scope');
    for (const t of targets) require(!baseline.unrelatedInventory[t.kind === 'snapshot' ? 'snapshots' : 'machines']
      .some(r => r.id === t.id || r.name === t.name), 'cache_provider_claimed_preexisting_resource');
  }
  require(input.phase === 'inventory' || baseline, 'cache_provider_preservation_baseline_required');
  const deadline = now() + 60000;
  const read = async call => {
    const remaining = deadline - now(); require(remaining > 0, 'cache_provider_read_deadline');
    let timer;
    try { return await Promise.race([call(), new Promise((_, reject) => {
      timer = setTimeout(() => reject(Error('cache_provider_read_deadline')), remaining);
    })]); } finally { clearTimeout(timer); }
  };
  // SDK0.2.14 ListVMs/ListSnapshots return complete arrays, with no cursor or
  // pagination parameter. An unknown envelope/over-bound inventory fails closed.
  const machines = normalized(await read(() => sdk.machines.list({allContexts:true})), 'machine');
  const snapshots = normalized(await read(() => sdk.snapshots.list()), 'snapshot');
  const protectedRows = machines.filter(r => r.id === packet.bounds.protectedMachineId);
  require(protectedRows.length === 1, 'cache_provider_original_missing');
  const protectedRead = await read(() => sdk.machines.get(packet.bounds.protectedMachineId));
  require(protectedRead.id === protectedRows[0].id && protectedRead.name === protectedRows[0].name, 'cache_provider_original_identity');
  if (baseline) require(JSON.stringify(protectedRows[0]) === JSON.stringify(baseline.protectedMachineMetadata), 'cache_provider_original_changed');
  const base = snapshots.filter(r => r.id === packet.pilot.baseArtifactId);
  require(base.length === 1 && base[0].version === String(packet.pilot.baseArtifactVersion), 'cache_provider_base_snapshot_changed');
  const rows = [];
  const get = async (kind, value) => {
    try { return await read(() => kind === 'snapshot' ? sdk.snapshots.get(value) : sdk.machines.get(value)); }
    catch (error) {
      if (error.name === 'NotFoundError' && (error.status === undefined || error.status === 404)
        && (error.statusCode === undefined || error.statusCode === 404)) return null;
      throw Error('cache_provider_read_unavailable');
    }
  };
  for (const t of targets) {
    const byId = t.id ? await get(t.kind, t.id) : null;
    const byName = await get(t.kind, t.name);
    const inventory = t.kind === 'snapshot' ? snapshots : machines;
    const matches = inventory.filter(r => r.id === t.id || r.name === t.name);
    require(matches.length <= 1, 'cache_provider_resource_identity_conflict');
    require((!byId && !byName) || matches.length === 1, 'cache_provider_inventory_inconsistent');
    for (const r of [byId,byName].filter(Boolean)) require(r.name === t.name && (!t.id || r.id === t.id), 'cache_provider_resource_identity_conflict');
    const absent = !byId && !byName && matches.length === 0;
    const state = !t.requested ? (absent ? 'not-submitted' : 'unrequested-resource-present')
      : !t.id ? 'requested-id-unrecorded' : absent ? 'absent-404' : 'present';
    if (state === 'requested-id-unrecorded' || state === 'unrequested-resource-present') unresolved.push({kind:t.kind, operationId:t.operationId, code:state});
    rows.push({...t, state, id404:t.id ? byId === null : null, name404:byName === null,
      inventoryAbsent:matches.length === 0, discoveredOriginalNameId:!t.id && byName ? byName.id : null});
  }
  const owned = (row, snapshot) => targets.some(t => (t.kind === 'snapshot') === snapshot && (row.id === t.id || row.name === t.name));
  const unrelatedInventory = {machines:machines.filter(r => r.id !== packet.bounds.protectedMachineId && !owned(r,false)),
    snapshots:snapshots.filter(r => !owned(r,true))};
  const unrelatedFingerprint = digest(unrelatedInventory);
  const unrelatedPreserved = baseline ? unrelatedFingerprint === baseline.unrelatedFingerprint : null;
  const absent = t => t.state === 'absent-404' || t.state === 'not-submitted';
  const preparationAbsent = rows.filter(t => ['builder','verifier'].includes(t.kind)).every(absent);
  const allAbsent = unresolved.length === 0 && rows.every(absent);
  const controlled = machines.filter(r => r.id === packet.bounds.protectedMachineId || owned(r,false));
  require(controlled.length <= 2, 'cache_provider_controlled_cap');
  require(now() - latestObservedAt <= 60000, 'cache_provider_stale_observation');
  const result = {version:1, kind:'boxd-project-cache-provider-observation-v1', readOnly:true, provider:'boxd',
    runId:packet.runId, packetSha256:input.packetSha256, observedAtMs:now(), phase:input.phase,
    observationSha256s:input.observations.map(o => o.sha256), latestSqlObservedAtMs:latestObservedAt, completeProviderInventory:true,
    taskReceiptSha256s:Object.fromEntries(Object.entries(input.tasks).map(([k,v]) => [k,
      {intent:v.intentSha256 ?? null,outcome:v.outcomeSha256 ?? null}])),
    totalBoxdMachines:machines.length, controlledProviderCounts:{boxd:controlled.length},
    unverifiedProviders:['boat','e2b','daytona'], protectedMachineId:packet.bounds.protectedMachineId,
    protectedOriginalPresent:true, baseSnapshotPreserved:true, targets:rows, unresolved,
    protectedMachineMetadata:protectedRows[0],
    preparationMachinesIndependentlyAbsent:preparationAbsent && unresolved.length === 0,
    providerAbsenceVerified:allAbsent, unrelatedInventory, unrelatedFingerprint, unrelatedPreserved,
    baselineCaptured:baseline === null, noProviderMutations:true};
  require(!baseline || unrelatedPreserved, 'cache_provider_unrelated_inventory_changed');
  return result;
}

export async function verifiedSdk(path, manifest) {
  require(manifest.version === '0.2.14' && manifest.files?.['index.js'] === PIN, 'cache_provider_sdk_pin');
  const root = dirname(resolve(path));
  const rootStat = await lstat(root); require(rootStat.isDirectory() && !rootStat.isSymbolicLink(), 'cache_provider_sdk_root');
  const files = [];
  const walk = async (directory, prefix = '') => {
    for (const entry of await readdir(directory, {withFileTypes:true})) {
      require(!entry.isSymbolicLink(), 'cache_provider_sdk_symlink');
      const name=prefix+entry.name;
      if (entry.isDirectory()) await walk(join(directory,entry.name),name+'/');
      else if (entry.isFile() && name.endsWith('.js')) files.push(name);
    }
  };
  await walk(root); files.sort();
  require(files.length <= 256 && Object.keys(manifest.files).every(n => /^[A-Za-z0-9_./-]+\.js$/.test(n)
    && !n.startsWith('/') && !n.split('/').includes('..')), 'cache_provider_sdk_paths');
  require(JSON.stringify(files) === JSON.stringify(Object.keys(manifest.files).sort()), 'cache_provider_sdk_closure');
  for (const n of files) {
    const p = join(root,n); const st = await lstat(p);
    require(st.isFile() && !st.isSymbolicLink() && hash(await readFile(p)) === manifest.files[n], 'cache_provider_sdk_sri');
  }
  const vendor = await import(pathToFileURL(resolve(path)).href);
  require(vendor.VERSION === '0.2.14', 'cache_provider_sdk_version');
  return new vendor.Boxd({apiKey:process.env.BOXD_API_KEY, maxRetries:0, timeout:15000});
}

async function main() {
  const args = process.argv.slice(2), options = {};
  for (let i=0;i<args.length;i+=2) { require(args[i]?.startsWith('--') && args[i+1], 'cache_provider_arguments');
    const key=args[i].slice(2); (options[key] ??= []).push(args[i+1]); }
  const one = key => { require(options[key]?.length === 1, 'cache_provider_argument_required'); return options[key][0]; };
  const read = async path => { const st=await lstat(path); require(st.isFile() && !st.isSymbolicLink()
    && (st.mode & 0o077) === 0 && st.size <= 2097152, 'cache_provider_private_file'); return readFile(path); };
  const packetBytes = await read(one('packet')), packetSha256=one('packet-sha256'); bound(packetBytes,packetSha256);
  const packet=JSON.parse(packetBytes.toString());
  require(packet.sourceFiles?.['infra/cloud/project-cache-rehearsal-provider.mjs'] === hash(await readFile(new URL(import.meta.url))),
    'cache_provider_observer_source_sri');
  const observationPaths=options.observation ?? [], observationHashes=options['observation-sha256'] ?? [];
  require(observationPaths.length === observationHashes.length, 'cache_provider_observation_arguments');
  const observations = await Promise.all(observationPaths.map(async(p,i)=>({bytes:await read(p),sha256:observationHashes[i]})));
  const state=resolve(one('state')), tasks={};
  const stateStat=await lstat(state); require(stateStat.isDirectory() && !stateStat.isSymbolicLink()
    && (stateStat.mode & 0o077) === 0, 'cache_provider_private_state');
  for (const label of ['miss','hit','restore']) {
    const intent=join(state,'create-'+label+'.intent.json'), outcome=join(state,'create-'+label+'.outcome.json');
    const exists=async p=>{try{await lstat(p);return true;}catch(e){if(e.code==='ENOENT')return false;throw e;}};
    const intentBytes=await exists(intent)?await read(intent):null, outcomeBytes=await exists(outcome)?await read(outcome):null;
    tasks[label]={intentPresent:!!intentBytes,intent:intentBytes?JSON.parse(intentBytes.toString()):null,
      outcome:outcomeBytes?JSON.parse(outcomeBytes.toString()):null,
      intentSha256:intentBytes?hash(intentBytes):null,outcomeSha256:outcomeBytes?hash(outcomeBytes):null};
  }
  const baseline=options.baseline?{bytes:await read(one('baseline')),sha256:one('baseline-sha256')}:null;
  const manifest=bound(await read(one('sdk-manifest')),one('sdk-manifest-sha256'));
  let sdk;
  try {
    sdk=await verifiedSdk(one('sdk'),manifest);
    const result=await inspectRehearsalProvider({packetBytes,packetSha256,observations,tasks,baseline,phase:one('phase')},{sdk});
    const f=await open(one('output'),'wx',0o600);try{await f.writeFile(JSON.stringify(result,null,2)+'\n');await f.sync();}finally{await f.close();}
    console.log(JSON.stringify({status:'boxd-provider-read-only-observed',runId:result.runId,
      controlledProviderCounts:result.controlledProviderCounts,providerAbsenceVerified:result.providerAbsenceVerified,
      preparationMachinesIndependentlyAbsent:result.preparationMachinesIndependentlyAbsent,unrelatedPreserved:result.unrelatedPreserved}));
    if ((result.phase==='clean'&&!result.providerAbsenceVerified)||(result.phase==='preparation-absent'&&!result.preparationMachinesIndependentlyAbsent))process.exitCode=1;
  } finally { sdk?.close(); }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try { await main(); } catch { console.log(JSON.stringify({status:'boxd-provider-observation-unconfirmed',providerAbsenceVerified:false,noProviderMutations:true}));process.exitCode=1; }
}
