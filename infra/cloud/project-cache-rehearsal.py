#!/usr/bin/env python3
"""Offline packet preparation for one serial Boxd cache pilot; never performs effects.

Candidate and source metadata must come from current independently observed inputs.
The resulting packet remains unexecutable until an exact future API binary is bound.
No credentials, DB connections, provider calls or service changes occur here.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import time
import uuid

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('cache_rehearsal_rollout', HERE / 'project-cache-rollout.py')
ROLLOUT = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ROLLOUT)
S = ROLLOUT.SECURE
require = S.require
RUNTIME = '1f2e4b942c8ae16d8c4ede346036e9da0c8227acddc614209de3fea0611f8fe1'

def compact(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=False).encode()

def recipe(command):
    # Exact EnvironmentRecipe serde field order, including Option None fields.
    return dict(version=1, scripts=dict(initialize=command, refresh=None), startup=[],
                environment={}, secretBindings=[], setupTimeoutSeconds=60)

def packet(candidate_path, pin_path, source_path, bounds_path, artifact_version, expires_at, run_id=None):
    candidate, pin, source, bounds = [S.load(p) for p in (candidate_path, pin_path, source_path, bounds_path)]
    ROLLOUT.bounds(bounds)
    S.exact(source, ('version','ownerId','actorId','projectId','repositoryIdentity','sourceCommit',
        'recipeSource','dependencyInputs','environmentRevision','codexConnectionId','codexModel',
        'sourceObservedAtMs','repositoryRecipeAbsent','dedicatedProject'), 'rehearsal_source_fields')
    now = int(time.time() * 1000)
    require(source['version'] == 1 and source['ownerId'] == source['actorId'] == pin['ownerId'] == bounds['ownerId']
        and source['projectId'] == bounds['projectId'], 'rehearsal_actor_project')
    require(source['dedicatedProject'] is True and source['repositoryRecipeAbsent'] is True
        and source['recipeSource'] == 'project', 'rehearsal_dedicated_project_recipe_required')
    require(type(source['environmentRevision']) is int and source['environmentRevision'] >= 0
        and type(source['sourceObservedAtMs']) is int and 0 <= now-source['sourceObservedAtMs'] <= 86400000,
        'rehearsal_fresh_source_required')
    require(isinstance(source['sourceCommit'], str) and re.fullmatch('[a-f0-9]{40}|[a-f0-9]{64}', source['sourceCommit'])
        and re.fullmatch(r'github.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', source['repositoryIdentity']), 'rehearsal_source_identity')
    require(S.identifier(source['codexConnectionId']) and isinstance(source['codexModel'], str)
        and re.fullmatch('[A-Za-z0-9._:/-]{1,160}', source['codexModel']), 'rehearsal_codex_binding')
    deps = source['dependencyInputs']
    require(isinstance(deps, dict) and len(deps) <= 128 and all(isinstance(k,str) and k and '..' not in k.split('/')
        and not k.startswith('/') and isinstance(v,str) and re.fullmatch('[a-f0-9]{40}|[a-f0-9]{64}',v) for k,v in deps.items()), 'rehearsal_dependencies')
    require(candidate['runtimeSha256'] == candidate['initialRuntimeBootstrap']['runtimeSha256'] == RUNTIME,
        'rehearsal_corrected_runtime_required')
    require(S.is_version(artifact_version) and type(expires_at) is int
        and now < expires_at <= now + bounds['maxDurationSeconds'] * 1000
        and expires_at <= pin['expiresAt'], 'rehearsal_expiry')
    require(run_id is None or isinstance(run_id,str)and re.fullmatch('[a-f0-9]{32}',run_id),'rehearsal_run_id')
    run = run_id or uuid.uuid4().hex
    marker = '.zuse-cache-rehearsal-' + run
    good = recipe("test ! -e '" + marker + "' && printf '%s' prepared > '" + marker + "'")
    bad = recipe('exit 23')
    dependency = S.digest(compact([source['sourceCommit'], dict(sorted(deps.items()))]))
    inputs = [dict(recipeDigest=S.digest(compact(r)), recipeSource='project', sourceCommit=source['sourceCommit'], dependencyDigest=dependency) for r in (good,bad)]
    pilot = dict(version=1,pilotId='cache-pilot-'+run,ownerId=source['ownerId'],actorId=source['actorId'],projectId=source['projectId'],
        repositoryIdentity=source['repositoryIdentity'],candidateSha256=pin['candidateSha256'],baseReleaseId=candidate['releaseId'],
        baseArtifactId=candidate['artifactId'],baseArtifactVersion=artifact_version,runtimeSha256=RUNTIME,
        architecture=candidate['architecture'],expiresAt=expires_at,protectedWorkspaceId=bounds['protectedWorkspaceId'],
        protectedMachineId=bounds['protectedMachineId'],maxActiveMachines=2,maxOwnedMachines=1,inputs=inputs,
        mechanismCapability=dict(version=1,runtimeSha256=RUNTIME,environmentCacheVersion=1,secretFreeSourceBundleVersion=1,
            retainedTaskStorageAbsentRequired=True,credentialFreePreparationRequired=True,preparationCaptureVersion=1))
    def task(label, first):
        return dict(projectId=source['projectId'],providerId='boxd',baseRef=source['sourceCommit'],branch='zuse/cache-'+run+'-'+label,
            agent='codex',model=source['codexModel'],agentConnectionId=source['codexConnectionId'],idempotencyKey=str(uuid.uuid4()),
            sizeId='default',runtimeMode='full-access',initialMessageDelivery='server-admitted-v2',firstMessage=first)
    first = "Run a shell test that the file '" + marker + "' contains exactly prepared. Do not install, commit, push or access credentials. Reply CACHE_PILOT_HIT_DONE."
    rev=source['environmentRevision']
    return dict(version=1,kind='boxd-project-cache-serial-rehearsal-v1',environment='staging',apiOrigin='https://api-rust-stag.zuse.sh',
        runId=run,createdAtMs=now,expiresAtMs=expires_at,bounds=bounds,pilot=pilot,pilotFingerprint=S.digest(compact(pilot)),basePin=pin,source=source,
        futureApiBinaryBinding=None,sourceFiles={str(p.relative_to(HERE.parents[1])):S.digest(p.read_bytes()) for p in
            (Path(__file__).resolve(),HERE/'project-cache-rehearsal-run.py',HERE/'project-cache-rehearsal-observe.py',HERE/'project-cache-rehearsal-provider.mjs',HERE/'project-cache-rehearsal-provider-others.py',HERE/'project-cache-project-bootstrap.py',HERE/'project-cache-rollout.py',HERE/'database.py',HERE/'project-cache-certification.py',HERE/'staging-github-canary.py',HERE/'staging-owner-device.py')},
        tasks=dict(miss=task('miss',None),hit=task('hit',first),restore=task('restore',None)),
        environmentBodies=dict(good=dict(expectedRevision=rev,recipe=good),bad=dict(expectedRevision=rev+1,recipe=bad),restore=dict(expectedRevision=rev+2,recipe=good)),
        marker=marker,commandIds={k:str(uuid.uuid4())for k in ('delete-miss','delete-hit','delete-restore','sleep-hit','resume-hit','wake-followup')},wakeMessageId=str(uuid.uuid4()),maxAwaitedTicks=64,maxModelTurns=2,cleanupReserveSeconds=bounds['cleanupReserveSeconds'],
        sequence=['save-good','create-miss','observe-same-operation-capacity-deferral','delete-miss','prove-provider-404',
            'await-original-preparation','prove-builder-404-before-verifier','prove-verifier-404-before-ready','create-hit',
            'prove-same-compatible-artifact','one-codex-ram-sleep-resume','delete-hit','prove-provider-404','save-bad',
            'await-failed-preparation-cleanup','prove-last-good-unchanged','restore-good','prove-compatible-restored-selection',
            'disconnect-dedicated-project','cleanup-original-resources','prove-protected-original-unchanged'],
        effectsAuthorized=False)

def prepare(args):
    require(not args.output.exists(), 'rehearsal_new_directory_required')
    value=packet(args.candidate,args.base_pin,args.source,args.bounds,args.artifact_version,args.expires_at,args.run_id)
    # All shape/pin checks are performed before publishing the directory.
    # ROLLOUT.plan additionally checks the exact typed pilot/base-pin binding.
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        pilot=Path(tmp)/'pilot.json';S.write_new(pilot,value['pilot'])
        rollout=ROLLOUT.plan(args.candidate,args.bounds,pilot,args.base_pin)
    args.output.mkdir(mode=0o700)
    for name,item in [('packet.json',value),('pilot.json',value['pilot']),('rollout-plan.json',rollout)]:
        S.write_new(args.output/name,item)
    for label,body in value['tasks'].items():S.write_new(args.output/('task-'+label+'.json'),body)
    for label,body in value['environmentBodies'].items():S.write_new(args.output/('environment-'+label+'.json'),body)
    return dict(status='offline-packet-prepared',packetSha256=S.digest(S.read(args.output/'packet.json')),
        pilotFingerprint=rollout['pilot']['fingerprint'],requiresFutureApiBinding=True,effects=False)

def bind(args):
    value=S.load(args.packet)
    require(value.get('futureApiBinaryBinding') is None and value.get('effectsAuthorized') is False,
        'rehearsal_original_unbound_packet_required')
    require(S.is_hash(args.api_sha256) and args.api_binary.is_absolute() and args.api_binary.is_file()
        and S.digest(args.api_binary.read_bytes())==args.api_sha256,'rehearsal_actual_api_binary_required')
    require(args.api_binary.stat().st_mode & 0o111, 'rehearsal_executable_required')
    require(value['bounds']['apiSha256']==args.api_sha256, 'rehearsal_reviewed_bounds_api_mismatch')
    value['futureApiBinaryBinding']=dict(path=str(args.api_binary),sha256=args.api_sha256)
    S.write_new(args.output,value)
    return dict(status='offline-api-bound-packet',packetSha256=S.digest(S.read(args.output)),effects=False)

def main():
    os.umask(0o077)
    p=argparse.ArgumentParser(description=__doc__);sub=p.add_subparsers(dest='action',required=True)
    a=sub.add_parser('prepare')
    for n in ('candidate','base-pin','source','bounds','output'):a.add_argument('--'+n,type=Path,required=True)
    a.add_argument('--artifact-version',required=True);a.add_argument('--expires-at',type=int,required=True);a.add_argument('--run-id')
    a=sub.add_parser('bind-api');a.add_argument('--packet',type=Path,required=True);a.add_argument('--api-binary',type=Path,required=True)
    a.add_argument('--api-sha256',required=True);a.add_argument('--output',type=Path,required=True)
    args=p.parse_args();print(json.dumps(prepare(args)if args.action=='prepare'else bind(args)))

if __name__=='__main__':
    try:main()
    except (S.Rejected,ValueError,OSError,KeyError,TypeError):raise SystemExit('cache_rehearsal_inputs_rejected_no_effects')from None
