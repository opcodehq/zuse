import importlib.util,json,unittest
from pathlib import Path
from unittest.mock import patch
P=Path(__file__).resolve().parents[1]/'project-cache-rehearsal-provider-others.py'
spec=importlib.util.spec_from_file_location('provider_others',P);M=importlib.util.module_from_spec(spec);spec.loader.exec_module(M)
RUN='a'*32
ROWS={'boat':[{'id':'old-boat','state':'archived'}],'e2b':[{'id':'old-e2b-a','state':'paused'},{'id':'old-e2b-b','state':'paused'}],'daytona':[{'id':'old-daytona','state':'archived'}]}
def plan():
 return dict(kind='boxd-cache-other-provider-observer-v1',readOnly=True,runId=RUN,scopeKind='boxd-project-cache-project-bootstrap-v1',scopeSha256='1'*64,
  ownershipPolicy=M.POLICY,caseProvider='boxd',protectedProvider='boxd',otherProviderAllocationsAuthorized=False,controlledResourceIds={p:[]for p in M.PROVIDERS},
  seedUnrelated={p:dict(count=len(v),stateCounts={s:sum(r['state']==s for r in v)for s in {r['state']for r in v}},idsFingerprint=M.ids_fingerprint(v))for p,v in ROWS.items()})
def scope():return dict(kind='boxd-project-cache-project-bootstrap-v1',runId=RUN,effectsAuthorized=False)
def fixture(rows=None,overrides=None):
 calls=[];rows=rows or ROWS
 def get(provider,path,timeout):
  calls.append((provider,path,timeout))
  if overrides and provider in overrides:return overrides[provider](path)
  values=rows[provider]
  if provider=='boat':return 200,{'sandboxes':values,'pageInfo':{'nextCursor':None}},{},None
  if provider=='e2b':return 200,[dict(sandboxID=r['id'],**{k:v for k,v in r.items()if k!='id'})for r in values],{},None
  return 200,{'items':values,'nextCursor':None},{},None
 return get,calls
def observe(p=None,s=None,rows=None,overrides=None,baseline=None):
 get,calls=fixture(rows,overrides);return M.observe(p or plan(),s or scope(),get,baseline,clock=lambda:10,wall=lambda:100000),calls
class ProviderTests(unittest.TestCase):
 def test_complete_account_inventories_do_not_become_global_controlled_counts(self):
  v,calls=observe();self.assertTrue(v['allProvidersVerified']);self.assertEqual(v['controlledProviderCounts'],dict(boat=0,e2b=0,daytona=0));self.assertEqual([v['providers'][p]['totalAccountResources']for p in M.PROVIDERS],[1,2,1]);self.assertFalse(v['boxdVerified']);self.assertFalse(v['authoritySqlOriginalAndApiFieldsVerified']);self.assertTrue(all(c[1]in('/sandboxes','/v2/sandboxes','/sandbox')for c in calls))
 def test_rebind_scope_keeps_original_unrelated_baseline(self):
  b,_=observe();p=plan();p.update(scopeKind='boxd-project-cache-serial-rehearsal-v1',scopeSha256='2'*64);s=scope();s['kind']=p['scopeKind'];v,_=observe(p,s,baseline=b);self.assertTrue(v['allProvidersVerified']);self.assertFalse(v['baselineCaptured']);self.assertEqual(v['unrelatedBaselineSha256'],b['unrelatedBaselineSha256'])
 def test_changed_resource_or_state_is_unknown_never_recaptured(self):
  b,_=observe()
  for replacement in [[{'id':'new-boat','state':'archived'}],[{'id':'old-boat','state':'running'}]]:
   rows=dict(ROWS,boat=replacement);v,_=observe(rows=rows,baseline=b);self.assertFalse(v['allProvidersVerified']);self.assertIsNone(v['controlledProviderCounts']['boat']);self.assertFalse(v['providers']['boat']['unrelatedPreserved'])
 def test_restricted_and_transport_failures_are_not_zero(self):
  for status,error in [(401,'restricted'),(403,'restricted'),(429,'http_unavailable'),(None,'transport_unavailable')]:
   v,_=observe(overrides={'e2b':lambda _,st=status,e=error:(st,None,{},e)});self.assertIsNone(v['controlledProviderCounts']['e2b']);self.assertFalse(v['allProvidersVerified']);self.assertIsNone(v['providers']['e2b']['totalAccountResources'])
 def test_all_three_pagination_contracts_are_completed(self):
  def boat(path):return(200,{'sandboxes':[]if'cursor='not in path else ROWS['boat'],'pageInfo':{'nextCursor':'next boat'if'cursor='not in path else None}},{},None)
  def e2b(path):return(200,[]if'nextToken='not in path else[{'sandboxID':x['id'],'state':x['state']}for x in ROWS['e2b']],{'X-Next-Token':'next/e2b'}if'nextToken='not in path else{},None)
  def day(path):return(200,{'items':[]if'cursor='not in path else ROWS['daytona'],'nextCursor':'next day'if'cursor='not in path else None},{},None)
  v,calls=observe(overrides=dict(boat=boat,e2b=e2b,daytona=day));self.assertTrue(v['allProvidersVerified']);self.assertTrue(all(v['providers'][p]['pages']==2 for p in M.PROVIDERS));self.assertTrue(any('next%2Fe2b'in c[1]for c in calls));self.assertTrue(any('next%20boat'in c[1]for c in calls))
 def test_unknown_shapes_duplicate_ids_and_nonterminating_pages_fail_closed(self):
  bad=[lambda _:(200,{'sandboxes':[]},{},None),lambda _:(200,{'sandboxes':[ROWS['boat'][0]]*2,'pageInfo':{'nextCursor':None}},{},None),lambda _:(200,{'sandboxes':[],'pageInfo':{'nextCursor':'same'}},{},None)]
  for callback in bad:
   v,_=observe(overrides={'boat':callback});self.assertFalse(v['allProvidersVerified']);self.assertIsNone(v['controlledProviderCounts']['boat'])
 def test_scope_expansion_or_new_owned_other_provider_is_denied_before_reads(self):
  for change in [dict(caseProvider='boat'),dict(otherProviderAllocationsAuthorized=True),dict(controlledResourceIds=dict(boat=['adopt'],e2b=[],daytona=[]))]:
   p=plan();p.update(change);get,calls=fixture()
   with self.assertRaises(M.S.Rejected):M.observe(p,scope(),get)
   self.assertEqual(calls,[])
 def test_case_footprint_cannot_be_adopted_as_unrelated_and_names_are_not_output(self):
  rows=dict(ROWS,boat=[dict(ROWS['boat'][0],name='private-'+RUN,labels={'do-not-output':'private-value'})]);v,_=observe(rows=rows);self.assertIsNone(v['controlledProviderCounts']['boat']);self.assertEqual(v['providers']['boat']['code'],'unauthorized_case_footprint');self.assertNotIn('private-value',json.dumps(v));self.assertNotIn('private-'+RUN,json.dumps(v))
 def test_baseline_scope_fingerprint_and_seed_changes_precede_reads(self):
  baseline,_=observe()
  for mode in ['run','fingerprint','seed']:
   b=json.loads(json.dumps(baseline));p=plan()
   if mode=='run':b['runId']='b'*32
   if mode=='fingerprint':b['providers']['boat']['unrelatedFingerprint']='0'*64
   if mode=='seed':p['seedUnrelated']['boat']['idsFingerprint']='0'*64
   get,calls=fixture()
   with self.assertRaises(M.S.Rejected):M.observe(p,scope(),get,b)
   self.assertEqual(calls,[])
 def test_overall_deadline_yields_unknown_not_an_empty_inventory(self):
  get,_=fixture();t=[0]
  def clock():t[0]+=20;return t[0]
  v=M.observe(plan(),scope(),get,clock=clock);self.assertFalse(v['allProvidersVerified']);self.assertIsNone(v['controlledProviderCounts']['daytona']);self.assertEqual(v['providers']['daytona']['code'],'deadline')
 def test_real_transport_only_uses_get_and_never_serializes_keys_or_provider_bodies(self):
  calls=[]
  class Response:
   status=200;headers={}
   def __enter__(self):return self
   def __exit__(self,*args):pass
   def read(self,_):return b'[]'
  class Opener:
   def open(self,request,timeout):calls.append(request);return Response()
  with patch.object(M.urllib.request,'build_opener',return_value=Opener()):
   get=M.transport(dict(BOAT_API_KEY='private-boat',E2B_API_KEY='private-e2b',DAYTONA_API_KEY='private-day'));get('boat','/sandboxes',1);get('e2b','/v2/sandboxes',1);get('daytona','/sandbox',1)
  self.assertEqual(len(calls),3);self.assertTrue(all(r.get_method()=='GET'and r.data is None for r in calls));self.assertEqual(calls[0].full_url,'https://boat.dev/api/v1/sandboxes');self.assertEqual(calls[1].full_url,'https://api.e2b.app/v2/sandboxes');self.assertEqual(calls[2].full_url,'https://app.daytona.io/api/sandbox')
if __name__=='__main__':unittest.main()
