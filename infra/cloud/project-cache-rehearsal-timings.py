#!/usr/bin/env python3
"""Extract single-case numeric timings from existing, hash-bound receipts only.

No remote, DB, provider, guest or model calls. Missing observations stay null.
Repository readiness is not agent readiness; cumulative Core-since-wake is
never added to its preceding authentication/grant/ACK components.
"""
import argparse
import datetime
import importlib.util
import json
import math
from pathlib import Path

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('cache_timing_secure',HERE/'project-cache-certification.py')
S=importlib.util.module_from_spec(spec);spec.loader.exec_module(S)
STAGES={'wake-observed','authorize-start','authorized','grant-start','granted','grant-not-configured',
    'transport-not-required','transport-reset-start','transport-reset-complete','resumed-ack-start',
    'resumed-acknowledged','core-reopened','authorization-not-admitted','connection-not-admitted','transport-recovery-required'}
API_EVENTS={'memory_resume_authority_connection':('pool_wait_us','workspace_read_us'),
    'memory_resume_authority_fence':('candidate_check_us','transaction_begin_us','workspace_lock_read_us')}
PROVIDER_PHASES={'providerLookup','providerInspect','providerRuntimeReadiness','providerCreate','initialSourceVerification','initialBootstrapHelper','initialBootstrapProofValidation','launchArtifactVerification','launchBinaryVerification','bootCredentialUpload','runtimeLaunchCommand'}

def number(v):return v if type(v)in(int,float)and 0<=v<=2**53 and math.isfinite(v)else None
def delta(end,start):
    end,start=number(end),number(start)
    return end-start if end is not None and start is not None and end>=start else None
def binding(expected,actual):
    S.require(isinstance(actual,dict)and all(actual.get(k)==v for k,v in expected.items()),'timing_foreign_scope')

def wake_timing(expected,document):
    trace=document.get('guest',{}).get('wakeTrace')or document.get('checks',{}).get('wakeTiming')or document.get('wakeTrace')or document
    S.require(trace.get('version')==1,'timing_trace_version');binding(expected,trace.get('binding'))
    events=trace.get('events');S.require(isinstance(events,list)and len(events)<=16,'timing_trace_bound')
    stages={};last=0
    for event in events:
        stage=event.get('stage');elapsed=number(event.get('elapsedMs'))
        S.require(stage in STAGES and elapsed is not None and elapsed>=last,'timing_trace_event');last=elapsed
        stages.setdefault(stage,[]).append(elapsed)
    unique=lambda name:stages[name][0]if len(stages.get(name,[]))==1 else None
    core=unique('core-reopened');ack=unique('resumed-acknowledged')
    return dict(clock='guest-monotonic-milliseconds',binding=expected,
        authorizationMs=delta(unique('authorized'),unique('authorize-start')),
        grantMs=delta(unique('granted'),unique('grant-start')),
        resumedAckMs=delta(ack,unique('resumed-ack-start')),
        transportResetMs=delta(unique('transport-reset-complete'),unique('transport-reset-start')),
        coreReopenedSinceWakeMs=core,coreReopenAfterAckMs=delta(core,ack),
        cumulativeCoreIncludesEarlierStages=True,compositeSumMs=None,
        grantNotConfigured='grant-not-configured'in stages,transportNotRequired='transport-not-required'in stages,
        ambiguousRepeatedStages=sorted(k for k,v in stages.items()if len(v)>1),
        qualification='Core-since-wake is cumulative. Components are separate observations, never added to that total; repeated stages are ambiguous, not summed.')

def timestamp_ms(value):
    if type(value)in(int,float):return number(value)
    if not isinstance(value,str):return None
    try:
        t=datetime.datetime.fromisoformat(value.replace('Z','+00:00'))
        return t.timestamp()*1000 if t.tzinfo else None
    except ValueError:return None

def api_timing_observations(expected,api_sha,records,envelope=None):
    S.require(isinstance(records,list)and len(records)<=256,'timing_api_log_bound')
    attributed=False
    if envelope is not None:
        binding(expected,envelope.get('binding'))
        S.require(S.is_hash(api_sha)and envelope.get('apiProcessSha256')==api_sha,'timing_api_process_scope')
        start=number(envelope.get('startedAtMs'));end=number(envelope.get('finishedAtMs'))
        S.require(start is not None and end is not None and end>=start and end-start<=180000,'timing_api_request_window')
        attributed=envelope.get('exclusiveCaseRequestWindowVerified')is True
    result=[];unattributed=0
    for record in records:
        if record.get('target')!='zuse_cloud::resume_timing':continue
        fields=record.get('fields',{});event=fields.get('message')
        if event not in API_EVENTS:continue
        for k,v in expected.items():
            if k in fields:S.require(fields[k]==v,'timing_foreign_api_log')
            if k in record:S.require(record[k]==v,'timing_foreign_api_log')
        at=timestamp_ms(record.get('timestamp'))
        if not attributed or at is None or not start<=at<=end:
            unattributed+=1;continue
        # Query and Resumed ACK share these names. Never relabel a sample as
        # authentication or ACK, and never combine it with guest phase totals.
        result.append(dict(event=event,observedAtMs=at,numericMicroseconds={k:number(fields.get(k))for k in API_EVENTS[event]},requestPhase=None))
    return dict(attributedObservations=result or None,unattributedSampleCount=unattributed,
        phaseAssignment=None,aggregateDurationMs=None,
        qualification='Numeric-only API records require exact process/binding/exclusive-request-window evidence. Query versus ACK attribution is not inferred.')

def task_timing(packet,label,intent,outcome,snapshots=(),visible=None,visible_scope=None):
    if outcome is None or outcome.get('uncertain')is True:return dict(workspaceId=None,admission=None,startup=None,firstVisible=None,receiptUncertain=bool(outcome))
    w=outcome.get('value',{}).get('workspace');S.require(isinstance(w,dict),'timing_create_receipt')
    expected=packet['tasks'][label]
    S.require(intent.get('action')=='create-'+label and intent.get('method')=='POST'
        and intent.get('body',{}).get('projectId')==packet['source']['projectId']
        and intent['body'].get('idempotencyKey')==expected['idempotencyKey']
        and w.get('projectId')==packet['source']['projectId']and w.get('providerId')=='boxd'
        and 200<=outcome.get('status',0)<300,'timing_task_scope')
    workspace=w['workspaceId'];S.require(workspace!=packet['bounds']['protectedWorkspaceId'],'timing_protected_workspace')
    merged={};provider={};preparation={}
    # Saved snapshots are chronological inputs. Preserve the latest known field
    # or cumulative provider counter; do not sum the same counter across copies.
    for row in [w,*snapshots]:
        S.require((row.get('workspaceId')or row.get('id'))==workspace,'timing_foreign_workspace')
        if row.get('projectId')is not None:S.require(row['projectId']==packet['source']['projectId'],'timing_foreign_project')
        timings=row.get('startupTimings')or{}
        for k,v in timings.items():
            if number(v)is not None:merged[k]=v
        for k,v in (timings.get('providerPhases')or{}).items():
            if k in PROVIDER_PHASES and isinstance(v,dict):
                provider[k]={n:number(v.get(n))for n in('callCount','lastDurationMs','totalDurationMs','lastCompletedAtMs')}
                provider[k].update(scope='worker-attempt',clock='monotonic-duration')
        for k,v in (timings.get('preparationPhases')or{}).items():
            if k in ('repository','credentials','environment','agent')and isinstance(v,dict):preparation[k]={n:number(v.get(n))for n in('startedAtMs','lastObservedAtMs','observedDurationMs')}
    requested=merged.get('requestedAt');first=None
    if visible is not None:
        binding({'workspaceId':workspace,'commandId':'launch:'+workspace},visible_scope)
        if visible.get('workspaceId')is not None:S.require(visible['workspaceId']==workspace,'timing_foreign_stream')
        if visible.get('observedLiveText')is True:first=number(visible.get('firstVisiblePartAtMs'))
    return dict(workspaceId=workspace,admission=dict(operatorIntentAtMs=number(intent.get('startedAtMs')),
        routeRequestedAtMs=requested,httpRoundtripDurationMs=number(outcome.get('durationMs')),
        operatorIntentToRouteRequestedMs=delta(requested,intent.get('startedAtMs')),
        qualification='Intent is not response/admission completion. Frozen journal without durationMs does not measure HTTP roundtrip; file mtimes are never substitutes.'),
        startup=dict(requestedToAllocatedMs=delta(merged.get('allocatedAt'),requested),
            allocatedToEnrolledMs=delta(merged.get('enrolledAt'),merged.get('allocatedAt')),
            requestedToRepositoryReadyMs=delta(merged.get('repositoryReadyAt'),requested),
            providerPhases=provider or None,preparationPhases=preparation or None,agentStartedAtMs=merged.get('agentStartedAt'),harnessReadyAtMs=None),
        firstVisible=dict(observedAtMs=first,requestedToFirstVisibleMs=delta(first,requested),
            qualification='Actual live observer wall timestamp only; not a model-token latency or repository/agent readiness timestamp.'))

def main():
    p=argparse.ArgumentParser(description=__doc__)
    for n in('packet','state','output'):p.add_argument('--'+n,type=Path,required=True)
    p.add_argument('--packet-sha256',required=True);p.add_argument('--inputs',type=Path);p.add_argument('--inputs-sha256');a=p.parse_args();raw=S.read(a.packet,2097152)
    S.require(S.digest(raw)==a.packet_sha256,'timing_packet_sri');packet=S.decode(raw)
    S.require(packet.get('kind')=='boxd-project-cache-serial-rehearsal-v1'and packet.get('effectsAuthorized')is False,'timing_packet')
    if a.inputs:
        raw_inputs=S.read(a.inputs);S.require(S.digest(raw_inputs)==a.inputs_sha256,'timing_inputs_manifest_sri');inputs=S.decode(raw_inputs)
    else:inputs=dict(runId=packet['runId'],packetSha256=a.packet_sha256)
    binding({'runId':packet['runId'],'packetSha256':a.packet_sha256},inputs)
    refs=[]
    def read_ref(ref):
        S.require(len(refs)<64,'timing_input_count');b=S.read(Path(ref['path']),2097152);S.require(S.digest(b)==ref['sha256'],'timing_input_sri');refs.append(ref);return S.decode(b)
    def journal(path):
        b=S.read(path);refs.append(dict(path=str(path),sha256=S.digest(b)));return S.decode(b)
    result={}
    for label in('miss','hit','restore'):
        i=a.state/('create-'+label+'.intent.json');o=a.state/('create-'+label+'.outcome.json')
        intent=journal(i)if i.exists()else{};outcome=journal(o)if o.exists()else None;extra=inputs.get('tasks',{}).get(label,{})
        snapshots=[]
        S.require(len(extra.get('workspaceSnapshots',[]))<=16,'timing_snapshot_bound')
        for ref in extra.get('workspaceSnapshots',[]):
            v=read_ref(ref);snapshots.append(v.get('workspace',v))
        visible=read_ref(extra['firstVisible'])if extra.get('firstVisible')else None
        result[label]=task_timing(packet,label,intent,outcome,snapshots,visible,extra.get('firstVisibleScope'))
    wake=None;logs=None
    if inputs.get('wake'):
        item=inputs['wake'];scope=item['binding'];hit=result['hit']['workspaceId'];S.require(scope.get('workspaceId')==hit,'timing_wake_task_scope')
        S.require(set(scope)=={'workspaceId','operationId','generation','storageIncarnationId'}and scope['generation']==1,'timing_wake_binding')
        wake=wake_timing(scope,read_ref(item['trace']))if item.get('trace')else None
        if item.get('apiLogs'):
            logs=api_timing_observations(scope,(packet.get('futureApiBinaryBinding')or{}).get('sha256'),read_ref(item['apiLogs']),item.get('apiEnvelope'))
    S.write_new(a.output,dict(version=1,status='single-case-saved-timings-extracted',readOnly=True,runId=packet['runId'],
        packetSha256=a.packet_sha256,extractorSha256=S.digest(Path(__file__).read_bytes()),secureHelperSha256=S.digest((HERE/'project-cache-certification.py').read_bytes()),inputsManifestSha256=a.inputs_sha256,tasks=result,wake=wake,apiResumeTimingObservations=logs,inputReferences=refs,
        sampleCount=1,p95=None,pureModelTokenLatencyMs=None,remoteCalls=0,
        qualification='Unknown stages remain null. Startup, HTTP/observer wall timings and guest monotonic timings have separate clocks and are not added.'))
    print(json.dumps({'status':'single-case-saved-timings-extracted','remoteCalls':0,'sampleCount':1,'p95':None}))

if __name__=='__main__':
    try:main()
    except (S.Rejected,ValueError,OSError,KeyError,TypeError):raise SystemExit('saved_case_timing_inputs_rejected')from None
