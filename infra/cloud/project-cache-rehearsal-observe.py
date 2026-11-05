#!/usr/bin/env python3
"""Read-only scoped cache evidence. SQL state never substitutes for provider404.

An original observation can be compared for same-operation deferral continuation
or unchanged compatible artifact after failure. Output is private, metadata only.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import time
HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('cache_rehearsal_observer',HERE/'project-cache-rehearsal.py')
M=importlib.util.module_from_spec(spec);spec.loader.exec_module(M)
S=M.S;DB=M.ROLLOUT.DB;require=S.require

def statement(p):
 owner,project=[DB.literal(p['source'][k])for k in ('ownerId','projectId')]
 release=DB.literal(p['pilot']['baseReleaseId'])
 # Explicit projections exclude credentials, source bodies, transcripts and config.
 return f"""BEGIN READ ONLY; SET LOCAL search_path=zuse_staging,pg_catalog; SET LOCAL statement_timeout='10s';
SELECT json_build_object(
 'requests',coalesce((SELECT json_agg(json_build_object('id',request_id,'actorId',actor_id,'revision',environment_revision,'state',state,'errorCode',last_error_code) ORDER BY environment_revision) FROM zuse_rust_project_preparation_requests WHERE owner_id={owner} AND project_id={project}),'[]'::json),
 'operations',coalesce((SELECT json_agg(json_build_object('id',operation_id,'requestId',request_id,'key',cache_key,'state',state,'phase',phase,'effects',resource_receipts->'effects','resources',resource_receipts) ORDER BY created_at) FROM zuse_rust_project_cache_operations WHERE owner_id={owner} AND project_id={project}),'[]'::json),
 'entries',coalesce((SELECT json_agg(json_build_object('key',cache_key,'state',state,'artifactId',artifact_id,'artifactVersion',artifact_version,'operationId',preparation_operation_id,'identity',identity,'receipt',receipt,'revoked',revoked_at IS NOT NULL) ORDER BY created_at) FROM zuse_rust_project_cache_entries WHERE owner_id={owner} AND project_id={project}),'[]'::json),
 'workspaces',coalesce((SELECT json_agg(json_build_object('id',workspace_id,'idempotencyKey',idempotency_key,'state',state,'machineId',provider_sandbox_id,'cache',request_config->'rustProjectCache','fetchRemote',request_config->'rustRepositoryCheckout'->'fetchRemote','generation',request_config->'runtimeGeneration')) FROM api_cloud_workspaces WHERE account_id={owner} AND project_id={project}),'[]'::json),
 'allocations',coalesce((SELECT json_agg(json_build_object('operationId',a.operation_id,'workspaceId',a.workspace_id,'generation',a.generation,'machineName',a.machine_name,'snapshotId',a.snapshot_id,'phase',a.phase)) FROM zuse_rust_allocations a JOIN api_cloud_workspaces w ON w.workspace_id=a.workspace_id WHERE w.account_id={owner} AND w.project_id={project}),'[]'::json),
 'sourceObjects',coalesce((SELECT json_agg(json_build_object('bundleSha256',bundle_sha256,'state',state,'cleanupAttempts',cleanup_attempts)) FROM zuse_rust_project_source_bundles WHERE owner_id={owner} AND project_id={project}),'[]'::json),
 'base',(SELECT json_build_object('state',state,'certifiedManifest',runtime_manifest IS NOT NULL) FROM cloud_managed_bases WHERE provider='boxd' AND release_id={release}),
 'foreignCacheWork',((SELECT count(*) FROM zuse_rust_project_cache_operations WHERE NOT(owner_id={owner} AND project_id={project}) AND state IN('queued','running','uncertain','cleaning'))+(SELECT count(*) FROM zuse_rust_project_cache_entries WHERE NOT(owner_id={owner} AND project_id={project}) AND state='ready')+(SELECT count(*) FROM zuse_rust_project_source_bundles WHERE NOT(owner_id={owner} AND project_id={project}))+(SELECT count(*) FROM zuse_rust_project_preparation_requests WHERE NOT(owner_id={owner} AND project_id={project}) AND state IN('queued','blocked','resolving','preparing'))),
 'foreignDueComputerJobs',(SELECT count(*) FROM zuse_rust_jobs j WHERE kind LIKE 'workspace-%' AND state IN('pending','running') AND (state='running' OR due_at<=floor(extract(epoch FROM clock_timestamp())*1000)::bigint) AND resource_id<>{DB.literal(p['bounds']['protectedWorkspaceId'])} AND NOT EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.workspace_id=j.resource_id AND w.account_id={owner} AND w.project_id={project})),
 'cacheCertificates',(SELECT count(*) FROM cloud_project_cache_capabilities WHERE provider='boxd' AND release_id={release})); COMMIT;"""

def verify(p,v,phase,original=None):
 require(v['base']==dict(state='preparing',certifiedManifest=False)and v['cacheCertificates']==0,'rehearsal_no_base_promotion')
 require(all(r['actorId']==p['source']['actorId']for r in v['requests']),'rehearsal_actor_changed')
 good=p['pilot']['inputs'][0]
 compatible=[e for e in v['entries']if e['state']=='ready'and not e['revoked']and e['identity'].get('recipeDigest')==good['recipeDigest']
  and e['identity'].get('sourceCommit')==good['sourceCommit']and e['identity'].get('baseArtifactId')==p['pilot']['baseArtifactId']
  and e['identity'].get('baseArtifactVersion')==p['pilot']['baseArtifactVersion']
  and e['identity'].get('baseRuntimeSha256')==p['pilot']['runtimeSha256']
  and e['identity'].get('dependencyInputs')==p['source']['dependencyInputs']
  and e['identity'].get('stagingPilotSha256')==p['pilotFingerprint']]
 if phase=='deferred':
  blocked=[r for r in v['requests']if r['state']=='blocked'and r['errorCode']=='staging_project_cache_resource_limit']
  require(len(blocked)==1,'rehearsal_capacity_deferral_not_observed')
  ops=[o for o in v['operations']if o['requestId']==blocked[0]['id']]
  require(len(ops)==1 and ops[0]['state']=='queued'and not (ops[0]['effects']or{}).get('allocate',{}).get('requested',False),'rehearsal_provider_intent_before_capacity')
 elif phase in ('ready','failure-fallback','restored-hit'):
  require(len(compatible)==1,'rehearsal_exact_compatible_entry_required')
  if phase=='ready':
   require(original is not None,'rehearsal_original_deferral_required')
   ids={o['id']for o in original['operations']}
   require(compatible[0]['operationId']in ids,'rehearsal_replaced_original_operation')
  else:
   require(original is not None,'rehearsal_original_ready_required')
   before=[e for e in original['entries']if e['key']==compatible[0]['key']]
   require(len(before)==1 and before[0]['artifactId']==compatible[0]['artifactId']and before[0]['receipt']==compatible[0]['receipt'],'rehearsal_last_good_changed')
   if phase=='failure-fallback':require(any(r['state']=='failed'for r in v['requests']),'rehearsal_failed_preparation_not_observed')
  if phase=='restored-hit':
   require(any(w['cache']and w['cache'].get('cacheKey')==compatible[0]['key']and w['fetchRemote']is False for w in v['workspaces']),'rehearsal_restored_hit_not_observed')
 elif phase=='clean':
  require(all(w['state']=='deleted'for w in v['workspaces'])and all(e['state']=='retired'for e in v['entries'])
   and all(o['state']in ('deleted','failed','revoked')for o in v['operations']),'rehearsal_original_cleanup_pending')
  require(all(s['state']=='cleaning'and s['cleanupAttempts']>0 for s in v['sourceObjects']),'rehearsal_source_cleanup_pending')
 return v

def main():
 os.umask(0o077);a=argparse.ArgumentParser(description=__doc__)
 for n in ('packet','database-url-file','output'):a.add_argument('--'+n,type=Path,required=True)
 a.add_argument('--phase',choices=('inspect','deferred','ready','failure-fallback','restored-hit','clean'),default='inspect')
 a.add_argument('--original',type=Path);a=a.parse_args();p=S.load(a.packet)
 rows=DB.execute(S.read(a.database_url_file,16384).decode().strip(),statement(p));require(len(rows)==1,'rehearsal_readonly_snapshot')
 original=S.load(a.original)if a.original else None
 if a.phase in ('ready','failure-fallback','restored-hit'):
  require(original is not None and original.get('packetSha256')==S.digest(S.read(a.packet))
   and original.get('phase')==('deferred'if a.phase=='ready'else'ready'),'rehearsal_original_phase_required')
 v=rows[0];verify(p,v,a.phase,original['observation']if original else None)
 S.write_new(a.output,dict(version=1,readOnly=True,observedAtMs=int(time.time()*1000),packetSha256=S.digest(S.read(a.packet)),phase=a.phase,ownerId=p['source']['ownerId'],projectId=p['source']['projectId'],observerSha256=S.digest(Path(__file__).read_bytes()),observation=v,providerAbsenceVerified=False))
 print(json.dumps(dict(status='scoped-readonly-observed',phase=a.phase,providerAbsenceVerified=False)))
if __name__=='__main__':
 try:main()
 except (S.Rejected,ValueError,OSError,KeyError,TypeError):raise SystemExit('cache_rehearsal_observation_rejected')from None
