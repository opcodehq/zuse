#!/usr/bin/env python3
"""One journaled serial pilot action. Default validate is offline; effects require Root GO.

No arbitrary URL, method, command or shell is accepted. API tokens stay in the
existing owner-canary authority. Worker effects use only the reviewed API ELF.
"""
import argparse
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import time

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('cache_rehearsal',HERE/'project-cache-rehearsal.py')
M=importlib.util.module_from_spec(spec);spec.loader.exec_module(M)
S=M.S;require=S.require

def validate(packet):
    require(packet.get('kind')=='boxd-project-cache-serial-rehearsal-v1'and packet.get('environment')=='staging'
        and packet.get('effectsAuthorized')is False,'rehearsal_packet_required')
    M.ROLLOUT.bounds(packet['bounds'])
    require(packet['pilot']['ownerId']==packet['source']['actorId']==packet['source']['ownerId']
        and packet['pilot']['projectId']==packet['source']['projectId'],'rehearsal_scope')
    require(packet['pilot']['runtimeSha256']==M.RUNTIME,'rehearsal_runtime')
    for n,h in packet['sourceFiles'].items():
        p=HERE.parents[1]/n
        require(p.is_file()and S.digest(p.read_bytes())==h,'rehearsal_source_changed')
    return dict(status='offline-valid',requiresFutureApiBinding=packet['futureApiBinaryBinding']is None,effects=False)

def original_cleanup_scope(a,p,evidence,binding):
    require(evidence.get('originalScopeVerified')is True and evidence.get('apiSha256')==binding['sha256']
        and evidence.get('preparationBackgroundDisabled')is True,'rehearsal_cleanup_authority')
    reference=evidence.get('originalSqlProof',{})
    require(isinstance(reference.get('path'),str)and S.is_hash(reference.get('sha256')),'rehearsal_original_sql_required')
    raw=S.read(reference['path']);require(S.digest(raw)==reference['sha256'],'rehearsal_original_sql_changed')
    proof=S.decode(raw)
    require(proof.get('readOnly')is True and proof.get('packetSha256')==S.digest(S.read(a.packet))
        and proof.get('ownerId')==p['source']['ownerId']and proof.get('projectId')==p['source']['projectId']
        and proof.get('observerSha256')==S.digest((HERE/'project-cache-rehearsal-observe.py').read_bytes())
        and type(proof.get('observedAtMs'))is int and 0<=int(time.time()*1000)-proof['observedAtMs']<=60000,
        'rehearsal_original_sql_scope')
    value=proof['observation'];require(value.get('foreignCacheWork')==0,'rehearsal_foreign_cache_cleanup_forbidden')
    task_keys={v['idempotencyKey']for v in p['tasks'].values()}
    require(all(w['idempotencyKey']in task_keys and w['id']!=p['bounds']['protectedWorkspaceId']
        and w.get('machineId')!=p['bounds']['protectedMachineId']for w in value['workspaces']), 'rehearsal_foreign_task_cleanup_forbidden')
    for workspace in value['workspaces']:
        label=next(k for k,v in p['tasks'].items()if v['idempotencyKey']==workspace['idempotencyKey'])
        intent=S.load(a.state/('create-'+label+'.intent.json'))
        require(intent.get('method')=='POST'and intent.get('path')=='/v1/cloud/workspaces'
            and intent.get('body')==p['tasks'][label],'rehearsal_original_task_intent_required')
    require(all(r['actorId']==p['source']['actorId']for r in value['requests']),'rehearsal_foreign_actor_cleanup_forbidden')
    keys=set()
    for entry in value['entries']:
        identity=entry['identity'];keys.add(entry['key'])
        require(identity.get('ownerId')==p['source']['ownerId']and identity.get('projectId')==p['source']['projectId']
            and identity.get('stagingPilotSha256')==p['pilotFingerprint'],'rehearsal_foreign_cache_identity')
        require(entry.get('artifactId')!=p['pilot']['baseArtifactId'],'rehearsal_base_cleanup_forbidden')
    for operation in value['operations']:
        require(operation['key']in keys and operation['id'].startswith('cacheop_'),'rehearsal_original_operation_required')
        provider=operation['resources'].get('provider',{})
        require(all(provider.get(k)!=p['bounds']['protectedMachineId']for k in ('builderId','verifierId'))
            and provider.get('artifactId')!=p['pilot']['baseArtifactId'],'rehearsal_protected_cleanup_forbidden')
    require(value.get('foreignDueComputerJobs')==0,'rehearsal_foreign_due_jobs')
    return value

def authorize(a,packet,cleanup):
    validate(packet);go=S.load(a.go)
    require(go.get('approved')is True and go.get('packetSha256')==S.digest(S.read(a.packet))
        and go.get('runId')==packet['runId'],'rehearsal_explicit_root_go')
    require(cleanup or int(time.time()*1000)<min(go.get('expiresAtMs',0),packet['expiresAtMs']-packet['cleanupReserveSeconds']*1000),'rehearsal_expired')
    binding=packet['futureApiBinaryBinding'];require(binding is not None,'rehearsal_api_not_bound')
    require(Path(binding['path']).is_absolute()and S.digest(Path(binding['path']).read_bytes())==binding['sha256'],
        'rehearsal_api_changed')
    evidence=S.load(a.inventory)
    if cleanup:
        original_cleanup_scope(a,packet,evidence,binding)
        return binding
    value=original_cleanup_scope(a,packet,evidence,binding)
    require(evidence.get('boxdAndDurableScopeVerified')is True and evidence.get('runId')==packet['runId']
        and type(evidence.get('observedAtMs'))is int and 0<=int(time.time()*1000)-evidence['observedAtMs']<=60000,
        'rehearsal_fresh_boxd_durable_scope')
    counts=evidence.get('controlledProviderCounts',{})
    require(set(counts)=={'boxd','boat','e2b','daytona'}and type(counts['boxd'])is int and 1<=counts['boxd']<=2
        and all(counts[k]is None or type(counts[k])is int and counts[k]==0 for k in ('boat','e2b','daytona')),
        'rehearsal_boxd_only_controlled_cap')
    require(evidence.get('protectedWorkspaceId')==packet['bounds']['protectedWorkspaceId']
        and evidence.get('protectedMachineId')==packet['bounds']['protectedMachineId']
        and evidence.get('protectedGeneration')==1 and evidence.get('initialMessageNeverLeased')is True
        and evidence.get('protectedRetainedSha256')==packet['bounds']['protectedRetainedSha256'],
        'rehearsal_original_changed')
    require(evidence.get('ownershipPolicy')=='protected-original-and-exact-case-resources-v1','rehearsal_ownership_policy')
    baselines=evidence.get('unrelatedBaselineSha256',{})
    require(set(baselines)=={'boxd','boat','e2b','daytona'}and S.is_hash(baselines['boxd'])and all(v is None or S.is_hash(v)for k,v in baselines.items()if k!='boxd'),'rehearsal_explicit_unrelated_baselines_required')
    require(go.get('unrelatedBaselineSha256')==baselines,'rehearsal_root_baseline_changed')
    boxd_ref=evidence.get('boxdProviderProof',{})
    require(isinstance(boxd_ref,dict)and S.is_hash(boxd_ref.get('sha256'))and isinstance(boxd_ref.get('path'),str),'rehearsal_bound_boxd_proof_required')
    boxd_raw=S.read(boxd_ref['path']);require(S.digest(boxd_raw)==boxd_ref['sha256'],'rehearsal_boxd_proof_changed')
    boxd=S.decode(boxd_raw)
    require(boxd.get('kind')=='boxd-project-cache-provider-observation-v1'and boxd.get('runId')==packet['runId']
        and boxd.get('packetSha256')==S.digest(S.read(a.packet))and boxd.get('readOnly')is True
        and boxd.get('completeProviderInventory')is True and boxd.get('noProviderMutations')is True
        and boxd.get('controlledProviderCounts')==dict(boxd=counts['boxd'])
        and boxd.get('protectedMachineId')==packet['bounds']['protectedMachineId']
        and boxd.get('protectedOriginalPresent')is True and boxd.get('baseSnapshotPreserved')is True
        and type(boxd.get('observedAtMs'))is int and 0<=int(time.time()*1000)-boxd['observedAtMs']<=60000,
        'rehearsal_boxd_proof_scope')
    require(boxd.get('unrelatedFingerprint')==baselines['boxd']
        and (boxd.get('unrelatedPreserved')is True or boxd.get('baselineCaptured')is True),'rehearsal_boxd_unrelated_changed')
    require(evidence.get('preparationMachinesIndependentlyAbsent')==boxd.get('preparationMachinesIndependentlyAbsent'),
        'rehearsal_boxd_absence_mismatch')
    other_ref=evidence.get('otherProviderProof')
    if other_ref is None:
        require(all(counts[k]is None for k in ('boat','e2b','daytona'))
            and evidence.get('otherProviderObservationStatus')=='unknown'
            and evidence.get('otherProviderUnknownReason')in ('not-observed','preflight-failed','restricted','unreachable'),
            'rehearsal_other_unknown_must_be_explicit')
    else:
        require(isinstance(other_ref.get('path'),str)and S.is_hash(other_ref.get('sha256')),'rehearsal_other_preservation_observation_required')
        other_raw=S.read(other_ref['path']);require(S.digest(other_raw)==other_ref['sha256'],'rehearsal_other_observation_changed')
        other=S.decode(other_raw)
        require(other.get('kind')=='boxd-cache-other-provider-observation-v1'and other.get('runId')==packet['runId']
            and other.get('packetSha256')==S.digest(S.read(a.packet))and other.get('readOnly')is True
            and other.get('noProviderMutations')is True and type(other.get('oldestObservedAtMs'))is int
            and 0<=int(time.time()*1000)-other['oldestObservedAtMs']<=60000,'rehearsal_other_observation_scope')
        require(other.get('controlledProviderCounts')=={k:counts[k]for k in ('boat','e2b','daytona')},'rehearsal_other_count_mismatch')
        for provider in ('boat','e2b','daytona'):
            item=other.get('providers',{}).get(provider,{})
            require(item.get('code')!='unauthorized_case_footprint','rehearsal_nonboxd_case_footprint')
            if item.get('status')=='verified':
                require(item.get('unrelatedFingerprint')==baselines[provider]and item.get('unrelatedPreserved')is True,'rehearsal_other_baseline_changed')
            else:
                require(item.get('status')=='unknown'and counts[provider]is None,'rehearsal_unknown_other_count')
    # Unknown noncase preservation is reported separately. It is never adopted
    # as a fresh baseline or represented as global certification.
    require(evidence.get('apiSha256')==binding['sha256'] and evidence.get('preparationBackgroundDisabled')is True
        and evidence.get('foreignPreparationRequests')==0,'rehearsal_active_api_preparation_scope')
    require(evidence.get('foreignOwnedMachines')==0 and evidence.get('exclusiveLane')is True,
        'rehearsal_exclusive_owned_lane')
    return binding

def run(a):
    packet=S.load(a.packet)
    if a.action=='validate':return validate(packet)
    cleanup=a.action in ('delete-miss','delete-hit','delete-restore','disconnect-project','cleanup-tick')
    binding=authorize(a,packet,cleanup)
    require(a.state.is_dir()and a.state.stat().st_uid==os.getuid()and not a.state.stat().st_mode&0o077,
        'rehearsal_private_state_required')
    descriptor=os.open(a.state/'operation.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
    try:
        require(stat.S_ISREG(os.fstat(descriptor).st_mode),'rehearsal_lock_required');fcntl.flock(descriptor,fcntl.LOCK_EX)
        result=locked(a,packet,binding,cleanup)
        result['globallyVerified']=False
        result['originalCleanupOnly']=cleanup
        return result
    finally:os.close(descriptor)

def locked(a,p,binding,cleanup):
    # Per-step intent has no bearer. Any uncertain mutation must be inspected;
    # supplying a fresh action name cannot resubmit the same semantic API action.
    key=a.action
    if key in ('tick','cleanup-tick'):
        require(type(a.tick)is int and 0<=a.tick<p['maxAwaitedTicks'],'rehearsal_tick_budget')
        key+='-'+str(a.tick)
    intent=a.state/(key+'.intent.json');outcome=a.state/(key+'.outcome.json')
    require(not intent.exists()and not outcome.exists(),'rehearsal_inspect_original_no_retry')
    if a.action in ('create-miss','save-bad','save-restore'):
        prior={'create-miss':'save-good','save-bad':'delete-hit','save-restore':'save-bad'}[a.action]
        require((a.state/(prior+'.outcome.json')).is_file(),'rehearsal_prior_semantic_action_required')
    if a.action=='save-restore':
        failed=S.load(a.state/'failure-fallback.observation.json')
        require(failed.get('packetSha256')==S.digest(S.read(a.packet))and failed.get('phase')=='failure-fallback',
            'rehearsal_actual_failed_recipe_observation_required')
    if a.action in ('create-hit','create-restore'):
        require((a.state/'ready.observation.json').is_file(),'rehearsal_ready_observation_required')
        ready=S.load(a.state/'ready.observation.json')
        require(ready.get('packetSha256')==S.digest(S.read(a.packet))and ready.get('phase')=='ready','rehearsal_exact_ready_observation')
        evidence=S.load(a.inventory)
        require(evidence.get('preparationMachinesIndependentlyAbsent')is True,'rehearsal_builder_verifier_absence_required')
    if a.action=='disconnect-project':
        value=original_cleanup_scope(a,p,S.load(a.inventory),binding)
        require(all(w['state']=='deleted'for w in value['workspaces']),'rehearsal_task_deletion_first')
    if a.action in ('tick','cleanup-tick'):
        require(os.environ.get('ZUSE_DEPLOYMENT_ENVIRONMENT')=='staging','rehearsal_worker_staging')
        env=dict(os.environ)
        require(not any(k in env for k in ('GITHUB_APP_PRIVATE_KEY','GITHUB_APP_PRIVATE_KEY_FILE','GITHUB_APP_CLIENT_SECRET')),'rehearsal_worker_api_signing_authority_forbidden')
        for key in ('BOAT_API_KEY','E2B_API_KEY','DAYTONA_API_KEY'):
            env.pop(key,None)
        if cleanup:
            env.pop('ZUSE_CLOUD_STAGING_BASE_PIN',None)
            env.pop('ZUSE_CLOUD_STAGING_PROJECT_CACHE_PILOT',None)
            env['ZUSE_CLOUD_PROJECT_PREPARATION_ENABLED']='0'
        else:
            require(json.loads(env.get('ZUSE_CLOUD_STAGING_BASE_PIN','null'))==p['basePin'],'rehearsal_process_base_pin_mismatch')
            require(json.loads(env.get('ZUSE_CLOUD_STAGING_PROJECT_CACHE_PILOT','null'))==p['pilot'],
                'rehearsal_process_pilot_mismatch')
            env['ZUSE_CLOUD_PROJECT_PREPARATION_ENABLED']='1'
        env['ZUSE_CLOUD_PROJECT_PREPARATION_BACKGROUND']='0'
        command=[binding['path'],'project-preparation-worker','--once']
        S.write_new(intent,dict(action=a.action,command=command,apiSha256=binding['sha256'],startedAtMs=int(time.time()*1000)))
        try:
            result=subprocess.run(command,env=env,capture_output=True,text=True,timeout=90)
            # stdout is the worker's single effects-summary JSON, never transcripts.
            summary=json.loads(result.stdout.strip())if result.returncode==0 else None
            S.write_new(outcome,dict(exitCode=result.returncode,summary=summary,stderrDiscarded=True))
            require(result.returncode==0,'rehearsal_worker_failed_inspect_original')
            return dict(status='awaited-worker-returned',tick=a.tick,summary=summary)
        except (subprocess.SubprocessError,ValueError):
            if not outcome.exists():S.write_new(outcome,dict(uncertain=True))
            raise S.Rejected('rehearsal_worker_uncertain_inspect_original')from None
    spec=importlib.util.spec_from_file_location('rehearsal_canary',HERE/'staging-github-canary.py')
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    canary=module.Canary(a.owner_dir,p['source']['ownerId'])
    project='/v1/cloud/projects/'+p['source']['projectId']
    if a.action.startswith('save-'):
        label=a.action.removeprefix('save-');method,path,body='PUT',project+'/environment',p['environmentBodies'][label]
    elif a.action.startswith('create-'):
        label=a.action.removeprefix('create-');method,path,body='POST','/v1/cloud/workspaces',p['tasks'][label]
        # Count only controlled resources; unrelated account resources remain untouched.
        require(S.load(a.inventory)['controlledProviderCounts']['boxd']==1,'rehearsal_task_slot_occupied')
    elif a.action.startswith('delete-'):
        label=a.action.removeprefix('delete-');prior=S.load(a.state/('create-'+label+'.outcome.json'))
        if isinstance(prior.get('value'),dict):
            w=prior['value']['workspace'];require(w['projectId']==p['source']['projectId'],'rehearsal_task_scope')
        else:
            value=original_cleanup_scope(a,p,S.load(a.inventory),binding)
            rows=[w for w in value['workspaces']if w['idempotencyKey']==p['tasks'][label]['idempotencyKey']]
            require(len(rows)==1,'rehearsal_original_create_outcome_unknown')
            w=dict(workspaceId=rows[0]['id'])
        method,path,body='POST','/v1/cloud/workspaces/'+w['workspaceId']+'/delete',dict(workspaceId=w['workspaceId'],commandId=p['commandIds'][a.action])
    elif a.action in ('sleep-hit','resume-hit','wake-followup'):
        prior=S.load(a.state/'create-hit.outcome.json')['value'];w=prior['workspace']
        require(w['projectId']==p['source']['projectId'],'rehearsal_hit_scope')
        if a.action=='sleep-hit':
            require(S.load(a.inventory).get('cachedFirstTurnCompleted')is True,'rehearsal_first_turn_completion_required')
            method,path,body='POST','/v1/cloud/workspaces/'+w['workspaceId']+'/pause',dict(workspaceId=w['workspaceId'],commandId=p['commandIds'][a.action])
        elif a.action=='resume-hit':
            require(S.load(a.inventory).get('sameHitSleepingWithMemoryReceipt')is True,'rehearsal_memory_sleep_receipt_required')
            method,path,body='POST','/v1/cloud/workspaces/'+w['workspaceId']+'/resume',dict(workspaceId=w['workspaceId'],commandId=p['commandIds'][a.action])
        else:
            require(S.load(a.inventory).get('sameHitMemoryWakeCompleted')is True,'rehearsal_original_memory_wake_required')
            sid=prior.get('initialSessionId')or w['initialSessionId'];mid=p['wakeMessageId']
            prompt="Run a shell test that '"+p['marker']+"' still contains exactly prepared, then reply CACHE_PILOT_RAM_RESUMED_DONE. Do not install, commit, push or access credentials."
            action=dict(type='submit',messageId=mid,attachments=[],payload=dict(kind='run',messageId=mid,request=dict(prompt=prompt,cwd='.',harness='codex',sandbox='danger-full-access',autoApprove=True,model=p['source']['codexModel'],reasoning=None,resume=None,modelOptions={},attachments=[])))
            method,path,body='POST','/v1/cloud/workspaces/'+w['workspaceId']+'/task-commands',dict(commandId=p['commandIds'][a.action],sessionId=sid,wake='preserve-sleep',action=action)
    elif a.action=='disconnect-project':method,path,body='DELETE',project,None
    else:raise S.Rejected('rehearsal_action_invalid')
    owner_lock=os.open(canary.root/'operation.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
    try:
        require(stat.S_ISREG(os.fstat(owner_lock).st_mode),'rehearsal_owner_lock');fcntl.flock(owner_lock,fcntl.LOCK_EX)
        token=canary.credential()
        S.write_new(intent,dict(action=a.action,method=method,path=path,body=body,startedAtMs=int(time.time()*1000)))
        try:code,value=canary.flow.http.request(method,p['apiOrigin']+path,token=token,body=body)
        except Exception:
            S.write_new(outcome,dict(uncertain=True));raise S.Rejected('rehearsal_http_uncertain_inspect_original')from None
        S.write_new(outcome,dict(status=code,value=value))
        require(200<=code<300,'rehearsal_http_failed_inspect_original')
        return dict(status='journaled-api-returned',action=a.action,httpStatus=code)
    finally:os.close(owner_lock)

def main():
    os.umask(0o077);p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('action',choices=('validate','save-good','save-bad','save-restore','create-miss','create-hit','create-restore',
        'delete-miss','delete-hit','delete-restore','sleep-hit','resume-hit','wake-followup','disconnect-project','tick','cleanup-tick'))
    p.add_argument('--packet',type=Path,required=True);p.add_argument('--state',type=Path);p.add_argument('--go',type=Path)
    p.add_argument('--inventory',type=Path);p.add_argument('--owner-dir',type=Path);p.add_argument('--tick',type=int)
    a=p.parse_args();require(a.action=='validate'or all((a.state,a.go,a.inventory)), 'rehearsal_explicit_operator_inputs')
    print(json.dumps(run(a)))

if __name__=='__main__':
    try:main()
    except (S.Rejected,ValueError,OSError,KeyError,TypeError):raise SystemExit('cache_rehearsal_rejected_inspect_original')from None
