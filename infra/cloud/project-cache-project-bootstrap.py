#!/usr/bin/env python3
"""Propose an ordinary new temporary project, then bind its factual server ID.

Default propose is offline. Connect is separately Root-approved and journaled;
never retries an uncertain POST or aliases/upserts a preexisting repository row.
GitHub objects are read-only throughout. No migration/flags/provider effects.
"""
import argparse,fcntl,importlib.util,json,os,time,uuid,urllib.request,urllib.error
from pathlib import Path
HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('cache_bootstrap',HERE/'project-cache-rehearsal.py')
M=importlib.util.module_from_spec(spec);spec.loader.exec_module(M)
S=M.S;require=S.require
OWNER='user_01KW7R9WGJFFSKDNESE7RN00N1'
REPO='github.com/swarajbachu/testing-audio'
COMMIT='beff5c4990e27f046507533c0230ca6091f23d3c'

def propose(a):
 source=S.load(a.source_proof);meta=S.load(a.uniqueness_proof);live=meta['metadata'];repo=source['repository'];tree=source['tree'];branch=source['branch']
 require(source['readOnly']is True and source['connectorUser']==dict(login='swarajbachu',id=97094800)
  and repo['id']==903682796 and repo['private']is True and repo['full_name']=='swarajbachu/testing-audio'
  and branch['commit']['sha']==COMMIT and branch['commit']['parents']==[],'bootstrap_exact_source')
 require(tree['truncated']is False and len(tree['tree'])==39 and sum(x.get('size',0)for x in tree['tree'])==362554
  and not any(x['path']=='.zuse/setup.json'or x['mode']not in ('100644','040000')for x in tree['tree']), 'bootstrap_bounded_no_repository_recipe')
 require(live['readOnly']is True and live['ownerId']==OWNER and live['repositoryIdentity']==REPO
  and live['query']['projects']==[], 'bootstrap_no_existing_project_required')
 pin=live['activeBasePin'];candidate=S.load(a.candidate)
 require(pin['runtimeSha256']==M.RUNTIME if 'runtimeSha256'in pin else candidate['runtimeSha256']==M.RUNTIME,'bootstrap_runtime')
 require(pin['candidateSha256']==S.digest(S.canonical(candidate))and pin['artifactId']==candidate['artifactId'], 'bootstrap_actual_pin_candidate')
 run=uuid.uuid4().hex
 value=dict(version=1,kind='boxd-project-cache-project-bootstrap-v1',ownerId=OWNER,repositoryIdentity=REPO,
  repositoryId=903682796,sourceCommit=COMMIT,treeGitSha=tree['sha'],sourceProofSha256=S.digest(S.read(a.source_proof)),
  uniquenessProofSha256=S.digest(S.read(a.uniqueness_proof)),candidateSha256=pin['candidateSha256'],
  candidateFileSha256=S.digest(S.read(a.candidate)),basePin=pin,runId=run,createdAtMs=int(time.time()*1000),
  request=dict(repositoryUrl='https://github.com/swarajbachu/testing-audio.git',defaultBranch=COMMIT,visibility='private',
    displayName='Temporary cache rehearsal '+run[:12],cloudEnvironment={},secretBindings=[],idempotencyKey='cache-project-'+run),
  requestHeaders={'If-None-Match':'*'},actualProjectId=None,environmentRevisionBefore=0,recipeCommands=dict(good="test ! -e '.zuse-cache-rehearsal-"+run+"' && printf '%s' prepared > '.zuse-cache-rehearsal-"+run+"'",bad='exit 23'),sourceObservedAtMs=source['observedAtMs'],
  dependencyInputs={x['path']:x['sha']for x in tree['tree']if x['path']=='pnpm-lock.yaml'},
  proposedEffects=['ordinary-staging-project-create','three-CAS-environment-saves','project-remove-after-owned-task-deletion'],
  forbiddenEffects=['GitHub-write','existing-project-change','base-promotion','certificate-publication'],effectsAuthorized=False)
 S.write_new(a.output,value);return dict(status='offline-project-proposal',proposalSha256=S.digest(S.read(a.output)),effects=False)

def connect(a):
 p=S.load(a.proposal);go=S.load(a.go);unique=S.load(a.uniqueness_proof);authority=S.load(a.api_proof)
 require(S.is_hash(go.get('apiSha256'))and authority.get('apiSha256')==go['apiSha256']
  and authority.get('readOnly')is True and authority.get('environment')=='staging'
  and authority.get('actualProcessVerified')is True and type(authority.get('observedAtMs'))is int
  and 0<=int(time.time()*1000)-authority['observedAtMs']<=60000,'bootstrap_actual_api_process_required')
 require(p['kind']=='boxd-project-cache-project-bootstrap-v1'and p['actualProjectId']is None
  and p['ownerId']==OWNER and p['repositoryIdentity']==REPO,'bootstrap_proposal')
 require(go.get('approved')is True and go.get('action')=='connect-temporary-cache-project'
  and go.get('proposalSha256')==S.digest(S.read(a.proposal))and go.get('expiresAtMs',0)>int(time.time()*1000),'bootstrap_explicit_go')
 require(unique['metadata']['query']['projects']==[]and unique['metadata']['ownerId']==OWNER
  and unique['metadata']['repositoryIdentity']==REPO and 0<=int(time.time()*1000)-unique['observedAtMs']<=60000,'bootstrap_fresh_uniqueness_required')
 require(not a.receipt.exists()and not Path(str(a.receipt)+'.intent.json').exists(),'bootstrap_inspect_original_no_retry')
 spec=importlib.util.spec_from_file_location('bootstrap_canary',HERE/'staging-github-canary.py');c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
 canary=c.Canary(a.owner_dir,OWNER)
 with(canary.root/'operation.lock').open('a')as lock:
  fcntl.flock(lock,fcntl.LOCK_EX)
  current=canary.request('GET','/v1/cloud/projects')
  require(not any(x['repositoryIdentity']==REPO for x in current['projects']),'bootstrap_repository_already_connected')
  token=canary.credential();S.write_new(str(a.receipt)+'.intent.json',dict(proposalSha256=S.digest(S.read(a.proposal)),request=p['request'],headers={'If-None-Match':'*'},startedAtMs=int(time.time()*1000)))
  request=urllib.request.Request('https://api-rust-stag.zuse.sh/v1/cloud/projects',data=json.dumps(p['request']).encode(),
   headers={'Accept':'application/json','Content-Type':'application/json','Authorization':'Bearer '+token,'If-None-Match':'*'},method='POST')
  try:
   try:response=canary.flow.http.opener.open(request,timeout=15)
   except urllib.error.HTTPError as error:response=error
   with response:code,value=response.code,canary.flow.http.decode(response.read(1048577))
  except Exception:
   S.write_new(a.receipt,dict(uncertain=True));raise S.Rejected('bootstrap_inspect_original_no_retry')from None
  S.write_new(a.receipt,dict(status=code,value=value,receivedAtMs=int(time.time()*1000)))
  require(code==201 and value['repositoryIdentity']==REPO and S.identifier(value['projectId']),'bootstrap_failed_inspect_original')
  return dict(status='ordinary-project-created',projectId=value['projectId'],providerEffects=False)

def materialize(a):
 p=S.load(a.proposal);r=S.load(a.receipt);intent=S.load(str(a.receipt)+'.intent.json')
 require(intent['proposalSha256']==S.digest(S.read(a.proposal))and r['status']==201 and r['value']['repositoryIdentity']==REPO,'bootstrap_original_response_required')
 project=r['value']['projectId'];require(S.identifier(project)and r['value']['createdAt']>=intent['startedAtMs']-10000,'bootstrap_new_project_identity')
 require(not a.output.exists(),'bootstrap_new_output_required');a.output.mkdir(mode=0o700)
 source=dict(version=1,ownerId=OWNER,actorId=OWNER,projectId=project,repositoryIdentity=REPO,sourceCommit=COMMIT,
  recipeSource='project',dependencyInputs=p['dependencyInputs'],environmentRevision=0,codexConnectionId=a.codex_connection,
  codexModel=a.codex_model,sourceObservedAtMs=p['sourceObservedAtMs'],repositoryRecipeAbsent=True,dedicatedProject=True)
 bounds=dict(version=1,caseId=p['runId'],ownerId=OWNER,projectId=project,protectedWorkspaceId='workspace_6676a60e2b5e454197145b59944ca0a3',
  protectedMachineId='1818ae3f-e59c-423f-80a5-4e426a52f596',protectedGeneration=1,
  protectedRetainedSha256='1cd20c0f44d1f26f90d84bd6ede5d765e52939c2ef0560cf2afca4a6a03d94df',apiSha256=a.future_api_sha256,
  maxActiveMachines=2,maxOwnedMachines=1,maxDurationSeconds=900,cleanupReserveSeconds=180)
 M.ROLLOUT.bounds(bounds)
 for n,v in [('source.json',source),('bounds.json',bounds),('base-pin.json',p['basePin'])]:S.write_new(a.output/n,v)
 return dict(status='factual-project-id-bound',projectId=project,apiBinaryStillUnbound=True,effects=False)

def main():
 os.umask(0o077);p=argparse.ArgumentParser(description=__doc__);sub=p.add_subparsers(dest='action',required=True)
 a=sub.add_parser('propose')
 for n in ('source-proof','uniqueness-proof','candidate','output'):a.add_argument('--'+n,type=Path,required=True)
 a=sub.add_parser('connect')
 for n in ('proposal','go','api-proof','uniqueness-proof','owner-dir','receipt'):a.add_argument('--'+n,type=Path,required=True)
 a=sub.add_parser('materialize')
 for n in ('proposal','receipt','output'):a.add_argument('--'+n,type=Path,required=True)
 for n in ('codex-connection','codex-model','future-api-sha256'):a.add_argument('--'+n,required=True)
 a=p.parse_args();print(json.dumps(globals()[a.action](a)))
if __name__=='__main__':
 try:main()
 except (S.Rejected,ValueError,OSError,KeyError,TypeError):raise SystemExit('cache_project_bootstrap_rejected_inspect_original')from None
