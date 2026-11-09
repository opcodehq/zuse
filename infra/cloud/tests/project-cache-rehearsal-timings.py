import importlib.util,json,os,subprocess,sys,tempfile,unittest
from pathlib import Path
P=Path(__file__).resolve().parents[1]/'project-cache-rehearsal-timings.py'
spec=importlib.util.spec_from_file_location('case_timings',P);M=importlib.util.module_from_spec(spec);spec.loader.exec_module(M)
B=dict(workspaceId='workspace_case',operationId='resume_case',generation=1,storageIncarnationId='disk_case')
def trace():
 return dict(version=1,binding=B,events=[dict(stage=s,elapsedMs=n)for s,n in [('wake-observed',0),('authorize-start',0),('authorized',469),('grant-start',470),('granted',2211),('transport-not-required',2211),('resumed-ack-start',2216),('resumed-acknowledged',2658),('core-reopened',2660)]])
def packet():return dict(kind='boxd-project-cache-serial-rehearsal-v1',effectsAuthorized=False,runId='case',source=dict(projectId='project_case'),bounds=dict(protectedWorkspaceId='original'),tasks={s:dict(idempotencyKey='key_'+s)for s in ('miss','hit','restore')})
def task():
 return dict(action='create-hit',method='POST',body=dict(projectId='project_case',idempotencyKey='key_hit'),startedAtMs=90),dict(status=201,value=dict(workspace=dict(workspaceId=B['workspaceId'],projectId='project_case',providerId='boxd',createdAt=100,startupTimings=dict(requestedAt=100))))
class TimingTests(unittest.TestCase):
 def test_actual_wake_phase_values_never_double_count_core(self):
  v=M.wake_timing(B,trace());self.assertEqual([v[k]for k in ('authorizationMs','grantMs','resumedAckMs','coreReopenedSinceWakeMs','coreReopenAfterAckMs')],[469,1741,442,2660,2]);self.assertIsNone(v['compositeSumMs']);self.assertTrue(v['cumulativeCoreIncludesEarlierStages'])
 def test_missing_stages_are_unknown_not_zero(self):
  t=trace();t['events']=t['events'][:1];v=M.wake_timing(B,t)
  for k in ('authorizationMs','grantMs','resumedAckMs','coreReopenedSinceWakeMs'):self.assertIsNone(v[k])
 def test_foreign_operation_disk_generation_and_workspace_rejected(self):
  for k in B:
   t=trace();t['binding']=dict(B,**{k:'foreign'})
   with self.assertRaises(M.S.Rejected):M.wake_timing(B,t)
 def test_repeated_stage_is_ambiguous_and_bad_monotonic_trace_rejected(self):
  t=trace();t['events'].append(dict(stage='core-reopened',elapsedMs=2670));v=M.wake_timing(B,t);self.assertIsNone(v['coreReopenedSinceWakeMs']);self.assertEqual(v['ambiguousRepeatedStages'],['core-reopened'])
  t=trace();t['events'][-1]['elapsedMs']=1
  with self.assertRaises(M.S.Rejected):M.wake_timing(B,t)
 def test_collector_wrappers_preserve_binding(self):
  self.assertEqual(M.wake_timing(B,dict(guest=dict(wakeTrace=trace()))),M.wake_timing(B,dict(checks=dict(wakeTiming=trace()))))
 def test_latest_cumulative_provider_snapshot_is_not_summed(self):
  i,o=task();phase=dict(callCount=2,lastDurationMs=7,totalDurationMs=11,lastCompletedAtMs=120)
  row=dict(id=B['workspaceId'],startupTimings=dict(allocatedAt=110,enrolledAt=130,repositoryReadyAt=150,providerPhases=dict(providerInspect=phase)))
  v=M.task_timing(packet(),'hit',i,o,[row,row]);self.assertEqual(v['startup']['providerPhases']['providerInspect']['totalDurationMs'],11);self.assertEqual(v['startup']['requestedToRepositoryReadyMs'],50);self.assertIsNone(v['startup']['agentStartedAtMs']);self.assertIsNone(v['admission']['httpRoundtripDurationMs'])
 def test_missing_requested_timestamp_and_uncertain_create_do_not_become_metrics(self):
  i,o=task();o['value']['workspace']['startupTimings']={};v=M.task_timing(packet(),'hit',i,o);self.assertIsNone(v['admission']['routeRequestedAtMs']);self.assertIsNone(M.task_timing(packet(),'hit',i,dict(uncertain=True))['startup'])
 def test_foreign_task_snapshot_idempotency_or_stream_scope_rejected(self):
  i,o=task()
  with self.assertRaises(M.S.Rejected):M.task_timing(packet(),'hit',i,o,[dict(id='foreign')])
  i['body']['idempotencyKey']='wrong'
  with self.assertRaises(M.S.Rejected):M.task_timing(packet(),'hit',i,o)
  i,o=task()
  with self.assertRaises(M.S.Rejected):M.task_timing(packet(),'hit',i,o,visible=dict(observedLiveText=True),visible_scope=dict(workspaceId='foreign',commandId='launch:foreign'))
 def test_first_visible_requires_actual_live_proof_not_completion(self):
  i,o=task();scope=dict(workspaceId=B['workspaceId'],commandId='launch:'+B['workspaceId'])
  for live,expected in [(False,None),(True,80)]:
   v=M.task_timing(packet(),'hit',i,o,visible=dict(observedLiveText=live,firstVisiblePartAtMs=180),visible_scope=scope);self.assertEqual(v['firstVisible']['requestedToFirstVisibleMs'],expected)
 def test_api_numeric_samples_without_exclusive_envelope_unattributed(self):
  records=[dict(target='zuse_cloud::resume_timing',timestamp=1100,fields=dict(message='memory_resume_authority_connection',pool_wait_us=400,workspace_read_us=800))]
  v=M.api_timing_observations(B,'a'*64,records);self.assertIsNone(v['attributedObservations']);self.assertEqual(v['unattributedSampleCount'],1)
  e=dict(binding=B,apiProcessSha256='a'*64,startedAtMs=1000,finishedAtMs=1200,exclusiveCaseRequestWindowVerified=True)
  v=M.api_timing_observations(B,'a'*64,records,e);self.assertEqual(v['attributedObservations'][0]['numericMicroseconds']['pool_wait_us'],400);self.assertIsNone(v['attributedObservations'][0]['requestPhase']);self.assertIsNone(v['aggregateDurationMs'])
  e['apiProcessSha256']='b'*64
  with self.assertRaises(M.S.Rejected):M.api_timing_observations(B,'a'*64,records,e)
 def test_api_outside_window_or_missing_timestamp_not_attributed(self):
  e=dict(binding=B,apiProcessSha256='a'*64,startedAtMs=1000,finishedAtMs=1200,exclusiveCaseRequestWindowVerified=True)
  records=[dict(target='zuse_cloud::resume_timing',timestamp=t,fields=dict(message='memory_resume_authority_fence',candidate_check_us=5))for t in (900,None)]
  v=M.api_timing_observations(B,'a'*64,records,e);self.assertIsNone(v['attributedObservations']);self.assertEqual(v['unattributedSampleCount'],2)
 def test_cli_hash_bound_saved_journal_and_tampered_reference(self):
  with tempfile.TemporaryDirectory()as td:
   d=Path(td)
   def put(name,value):
    p=d/name;p.write_text(json.dumps(value));p.chmod(0o600);return p
   p=put('packet.json',packet());i,o=task();put('create-hit.intent.json',i);put('create-hit.outcome.json',o)
   t=put('trace.json',trace());manifest=dict(runId='case',packetSha256=M.S.digest(p.read_bytes()),wake=dict(binding=B,trace=dict(path=str(t),sha256=M.S.digest(t.read_bytes()))));m=put('inputs.json',manifest)
   def run(output):return subprocess.run([sys.executable,str(P),'--packet',str(p),'--packet-sha256',M.S.digest(p.read_bytes()),'--state',str(d),'--inputs',str(m),'--inputs-sha256',M.S.digest(m.read_bytes()),'--output',str(d/output)],capture_output=True,text=True)
   r=run('result.json');self.assertEqual(r.returncode,0,r.stderr);v=json.loads((d/'result.json').read_text());self.assertEqual(v['wake']['coreReopenedSinceWakeMs'],2660);self.assertEqual(v['remoteCalls'],0);self.assertIsNone(v['p95']);self.assertEqual(len(v['inputReferences']),3)
   t.write_text('{}');r=run('rejected.json');self.assertNotEqual(r.returncode,0);self.assertFalse((d/'rejected.json').exists())
if __name__=='__main__':unittest.main()
