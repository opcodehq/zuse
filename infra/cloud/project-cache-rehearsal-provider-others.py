#!/usr/bin/env python3
"""Read-only companion to the Boxd cache observer. No allocation or lifecycle APIs.

Reuses the established Boat cursor, E2B X-Next-Token and Daytona nextCursor
inventory contracts. Restricted/incomplete reads remain unknown, never zero.
Only an explicitly Boxd-only case can classify these preserved resources as
unrelated. Operational SQL, Boxd and authority evidence are separate witnesses.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import time
import urllib.error
import urllib.parse
import urllib.request

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('cache_other_secure',HERE/'project-cache-certification.py')
S=importlib.util.module_from_spec(spec);spec.loader.exec_module(S)
PROVIDERS=('boat','e2b','daytona')
ORIGINS={'boat':'https://boat.dev/api/v1','e2b':'https://api.e2b.app','daytona':'https://app.daytona.io/api'}
KEYS={'boat':'BOAT_API_KEY','e2b':'E2B_API_KEY','daytona':'DAYTONA_API_KEY'}
POLICY='protected-original-and-exact-case-resources-v1'

def digest(value):return S.digest(S.canonical(value))
def ids_fingerprint(rows):
    # Same encoding as the already sealed prior Boat inventory receipts.
    return S.digest(json.dumps(sorted(r['id']for r in rows)).encode())

def validate(plan,scope):
    S.require(plan.get('kind')=='boxd-cache-other-provider-observer-v1'and plan.get('readOnly')is True,'other_provider_plan')
    S.require(re.fullmatch('[a-f0-9]{32}',plan.get('runId',''))and scope.get('runId')==plan['runId']
        and scope.get('kind')==plan['scopeKind']and scope.get('effectsAuthorized')is False
        and plan['scopeKind']in('boxd-project-cache-project-bootstrap-v1','boxd-project-cache-serial-rehearsal-v1',
            'whole-cycle-boxd-cache-rehearsal-review'),'other_provider_scope')
    S.require(plan.get('ownershipPolicy')==POLICY and plan.get('caseProvider')=='boxd'
        and plan.get('protectedProvider')=='boxd'and plan.get('otherProviderAllocationsAuthorized')is False
        and plan.get('controlledResourceIds')=={p:[]for p in PROVIDERS},'other_provider_ownership_policy')
    S.require(set(plan.get('seedUnrelated',{}))==set(PROVIDERS),'other_provider_seed_required')
    for p,seed in plan['seedUnrelated'].items():
        S.require(type(seed.get('count'))is int and 0<=seed['count']<=2048 and S.is_hash(seed.get('idsFingerprint'))
            and isinstance(seed.get('stateCounts'),dict)and all(isinstance(k,str)and type(v)is int and v>=0 for k,v in seed['stateCounts'].items())
            and sum(seed['stateCounts'].values())==seed['count'],'other_provider_seed_shape')
    return digest({k:plan[k]for k in('ownershipPolicy','caseProvider','protectedProvider','otherProviderAllocationsAuthorized','controlledResourceIds','seedUnrelated')})

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,*_args,**_kwargs):raise ValueError('redirect_forbidden')

def transport(keys):
    opener=urllib.request.build_opener(NoRedirect)
    def get(provider,path,timeout):
        key=keys.get(KEYS[provider])
        if not key:return None,None,{},'credential_unavailable'
        headers={'X-API-Key':key}if provider=='e2b'else{'Authorization':'Bearer '+key}
        try:
            with opener.open(urllib.request.Request(ORIGINS[provider]+path,headers=headers,method='GET'),timeout=timeout)as response:
                raw=response.read(1048577)
                if len(raw)>1048576:return response.status,None,{},'response_bound'
                return response.status,json.loads(raw),dict(response.headers),None
        except urllib.error.HTTPError as error:
            code=error.code;error.close();return code,None,{},'restricted'if code in(401,403)else'http_unavailable'
        except Exception:return None,None,{},'transport_unavailable'
    return get

def inventory(provider,get,deadline,clock):
    rows=[];cursor=None;seen=set();pages=0
    try:
        for pages in range(1,21):
            remaining=deadline-clock()
            if remaining<=0:return None,pages,'deadline'
            if provider=='boat':path='/sandboxes'+('?cursor='+urllib.parse.quote(cursor,safe='')if cursor else'')
            elif provider=='e2b':path='/v2/sandboxes'+('?nextToken='+urllib.parse.quote(cursor,safe='')if cursor else'')
            else:path='/sandbox'+('?cursor='+urllib.parse.quote(cursor,safe='')if cursor else'')
            status,value,headers,error=get(provider,path,min(15,remaining))
            if status!=200 or error:return None,pages,error or'http_unavailable'
            if provider=='boat':
                if (not isinstance(value,dict)or not isinstance(value.get('sandboxes'),list)or not isinstance(value.get('pageInfo'),dict)
                    or'nextCursor'not in value['pageInfo']):return None,pages,'unknown_inventory_shape'
                items=value['sandboxes'];next_cursor=value['pageInfo']['nextCursor']
            elif provider=='e2b':
                if not isinstance(value,list):return None,pages,'unknown_inventory_shape'
                items=value;next_cursor=next((v for k,v in headers.items()if k.lower()=='x-next-token'),None)
            else:
                if not isinstance(value,dict)or not isinstance(value.get('items'),list)or'nextCursor'not in value:return None,pages,'unknown_inventory_shape'
                items=value['items'];next_cursor=value['nextCursor']
            for item in items:
                id=item.get('sandboxID')if provider=='e2b'else item.get('id');state=item.get('state')
                if not S.identifier(id)or not isinstance(state,str)or not re.fullmatch('[A-Za-z0-9_-]{1,80}',state):return None,pages,'unknown_resource_shape'
                # Only safe exact ownership evidence is inspected; none of the
                # raw names/labels/metadata or provider bodies are emitted.
                metadata=[item.get('name'),*(item.get('metadata')or{}).values(),*(item.get('labels')or{}).values()]
                rows.append({'id':id,'state':state,'scopeValues':[x for x in metadata if isinstance(x,str)]})
            if len(rows)>2048 or len({r['id']for r in rows})!=len(rows):return None,pages,'ambiguous_or_overbound_inventory'
            if next_cursor is None or next_cursor=='':return sorted(rows,key=lambda r:r['id']),pages,None
            if not isinstance(next_cursor,str)or len(next_cursor)>2048 or next_cursor in seen:return None,pages,'pagination_unknown'
            seen.add(next_cursor);cursor=next_cursor
        return None,pages,'inventory_incomplete'
    except Exception:return None,pages,'unknown_inventory_shape'

def observe(plan,scope,get,baseline=None,clock=time.monotonic,wall=lambda:int(time.time()*1000)):
    policy=validate(plan,scope)
    if baseline:
        S.require(baseline.get('kind')=='boxd-cache-other-provider-observation-v1'and baseline.get('runId')==plan['runId']
            and baseline.get('policySha256')==policy and baseline.get('allProvidersVerified')is True,'other_provider_baseline_scope')
        S.require(set(baseline.get('providers',{}))==set(PROVIDERS),'other_provider_baseline_shape')
        for p,value in baseline['providers'].items():
            S.require(value.get('status')=='verified'and isinstance(value.get('unrelatedResources'),list)
                and digest(value['unrelatedResources'])==value.get('unrelatedFingerprint'),'other_provider_baseline_fingerprint')
    started_wall=wall();deadline=clock()+60;proofs={}
    for provider in PROVIDERS:
        rows,pages,error=inventory(provider,get,deadline,clock)
        if rows is None:
            proofs[provider]={'completeInventory':False,'status':'unknown','code':error,'pages':pages,'controlledCount':None,'totalAccountResources':None,'unrelatedPreserved':None};continue
        if any(plan['runId']in value for row in rows for value in row['scopeValues']):
            proofs[provider]={'completeInventory':True,'status':'unknown','code':'unauthorized_case_footprint','pages':pages,'controlledCount':None,'totalAccountResources':len(rows),'unrelatedPreserved':False};continue
        safe=[{'id':r['id'],'state':r['state']}for r in rows];states={s:sum(r['state']==s for r in safe)for s in sorted({r['state']for r in safe})};seed=plan['seedUnrelated'][provider]
        seeded=len(safe)==seed['count']and states==seed['stateCounts']and ids_fingerprint(safe)==seed['idsFingerprint']
        fingerprint=digest(safe);preserved=fingerprint==baseline['providers'][provider]['unrelatedFingerprint']if baseline else seeded
        verified=seeded and preserved
        proofs[provider]={'completeInventory':True,'status':'verified'if verified else'unknown','code':None if verified else'unrelated_inventory_changed',
            'pages':pages,'controlledCount':0 if verified else None,'totalAccountResources':len(safe),'unrelatedResources':safe,
            'unrelatedStateCounts':states,'unrelatedIdsFingerprint':ids_fingerprint(safe),'unrelatedFingerprint':fingerprint,'unrelatedPreserved':preserved}
    complete=all(v['status']=='verified'for v in proofs.values())
    return {'version':1,'kind':'boxd-cache-other-provider-observation-v1','readOnly':True,'runId':plan['runId'],
        'scopeKind':plan['scopeKind'],'scopeSha256':plan['scopeSha256'],'ownershipPolicy':POLICY,'policySha256':policy,
        'packetSha256':plan['scopeSha256']if plan['scopeKind']=='boxd-project-cache-serial-rehearsal-v1'else None,
        'oldestObservedAtMs':started_wall,'observedAtMs':wall(),'providers':proofs,'controlledProviderCounts':{p:v['controlledCount']for p,v in proofs.items()},
        'unrelatedBaselineSha256':{p:v.get('unrelatedFingerprint')if v['status']=='verified'else None for p,v in proofs.items()},
        'allProvidersVerified':complete,'unrelatedBaselinesUnchanged':complete,'boxdVerified':False,
        'authoritySqlOriginalAndApiFieldsVerified':False,'protectedOriginalProvider':'boxd','noProviderMutations':True,
        'baselineCaptured':baseline is None,'internalProviderBuildCancellationCertified':False}

def main():
    os.umask(0o077);p=argparse.ArgumentParser(description=__doc__)
    for key in('plan','scope','output'):p.add_argument('--'+key,type=Path,required=True)
    p.add_argument('--baseline',type=Path);p.add_argument('--baseline-sha256');p.add_argument('--read-only-live',action='store_true')
    p.add_argument('--approved-plan-sha256');p.add_argument('--worker-env',type=Path);a=p.parse_args();raw=S.read(a.plan);plan=S.decode(raw);scope_raw=S.read(a.scope);scope=S.decode(scope_raw)
    S.require(S.digest(scope_raw)==plan['scopeSha256'],'other_provider_scope_sri');validate(plan,scope)
    if not a.read_only_live:print(json.dumps({'status':'offline-only','providerCalls':0,'keysRead':False}));return
    S.require(a.approved_plan_sha256==S.digest(raw)and plan['observerSha256']==S.digest(Path(__file__).read_bytes())
        and plan['secureHelperSha256']==S.digest((HERE/'project-cache-certification.py').read_bytes()),'other_provider_reviewed_sri')
    S.require(type(plan.get('expiresAtMs'))is int and int(time.time()*1000)<plan['expiresAtMs'],'other_provider_expired')
    S.require(a.worker_env is not None,'other_provider_private_credentials');env_raw=S.read(a.worker_env,1048576);keys={}
    for line in env_raw.decode().splitlines():
        if'='in line:
            k,v=line.split('=',1)
            if k in KEYS.values():
                values=shlex.split(v);S.require(len(values)==1 and not any(c in values[0]for c in '\r\n\0'),'other_provider_key_shape');keys[k]=values[0]
    baseline=None
    if a.baseline:
        baseline_raw=S.read(a.baseline,2097152);S.require(S.digest(baseline_raw)==a.baseline_sha256,'other_provider_baseline_sri');baseline=S.decode(baseline_raw)
    result=observe(plan,scope,transport(keys),baseline)
    result['preservedBaselineProofSha256']=a.baseline_sha256 if baseline else None
    S.require(S.read(a.worker_env,1048576)==env_raw,'other_provider_config_changed')
    S.write_new(a.output,result)
    print(json.dumps({'status':'readonly-other-providers-observed','runId':result['runId'],'allProvidersVerified':result['allProvidersVerified'],
        'controlledProviderCounts':result['controlledProviderCounts'],'unrelatedBaselineSha256':result['unrelatedBaselineSha256'],'providerMutations':0}))
    if not result['allProvidersVerified']:raise SystemExit(1)

if __name__=='__main__':
    try:main()
    except (S.Rejected,ValueError,OSError,KeyError,TypeError):raise SystemExit('other_provider_observation_unconfirmed_no_effects')from None
