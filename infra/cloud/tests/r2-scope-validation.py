#!/usr/bin/env python3
import importlib.util,io,json,tempfile,unittest,os
from pathlib import Path
spec=importlib.util.spec_from_file_location('scope',Path(__file__).resolve().parents[1]/'r2-scope-validation.py');scope=importlib.util.module_from_spec(spec);spec.loader.exec_module(scope)
KEY='PRIVATE_PRODUCTION_OBJECT_KEY_NEVER_PRINT'
KNOWN={'key':KEY,'etag':'"verified-source-etag"','bytes':327}
class S3Error(Exception):
 def __init__(self,status,code):self.response={'Error':{'Code':code},'ResponseMetadata':{'HTTPStatusCode':status}}
class Control:
 def __init__(self,status=200):self.calls=[];self.status=status
 def head_object(self,**args):
  self.calls.append(('HEAD',args['Bucket'],args['Key']));assert args=={'Bucket':scope.PRODUCTION,'Key':KEY}
  if self.status!=200:raise S3Error(self.status,str(self.status))
  return {'ETag':KNOWN['etag'],'ContentLength':KNOWN['bytes']}
class Candidate:
 def __init__(self,prod=403,auth=True,prod_code='AccessDenied',lost_put=False,put_missing=False,lost_delete=False,delete_missing=False,corrupt=False,collision=False):
  self.calls=[];self.data={};self.prod=prod;self.auth=auth;self.prod_code=prod_code;self.lost_put=lost_put;self.put_missing=put_missing;self.lost_delete=lost_delete;self.delete_missing=delete_missing;self.corrupt=corrupt;self.collision=collision
 def head_object(self,Bucket,Key):
  self.calls.append(('HEAD',Bucket,Key))
  if Bucket==scope.PRODUCTION:
   assert Key==KEY
   if self.prod!=200:raise S3Error(self.prod,self.prod_code)
   return {'ETag':KNOWN['etag'],'ContentLength':KNOWN['bytes']}
  assert Bucket==scope.STAGING and scope.own_key(Key)
  if not self.auth:raise S3Error(403,'InvalidAccessKeyId')
  if self.collision and Key not in self.data:self.data[Key]=b'preexisting collision'
  if Key not in self.data:raise S3Error(404,'NoSuchKey')
  return {'ContentLength':len(self.data[Key])}
 def put_object(self,**args):
  bucket=args['Bucket'];key=args['Key'];assert bucket==scope.STAGING and scope.own_key(key);self.calls.append(('PUT',bucket,key));assert args['IfNoneMatch']=='*' and args['ContentLength']==256
  if not self.put_missing:self.data[key]=args['Body'] if not self.corrupt else b'foreign data'
  if self.lost_put:raise TimeoutError('must never print endpoint or authorization')
 def get_object(self,Bucket,Key):
  assert Bucket==scope.STAGING and scope.own_key(Key);self.calls.append(('GET',Bucket,Key));return {'Body':io.BytesIO(self.data[Key])}
 def delete_object(self,Bucket,Key):
  assert Bucket==scope.STAGING and scope.own_key(Key);self.calls.append(('DELETE',Bucket,Key))
  if not self.delete_missing:self.data.pop(Key,None)
  if self.lost_delete:raise TimeoutError('private details')
class Scope(unittest.TestCase):
 def execute(self,candidate,control=None):
  control=control or Control()
  with tempfile.TemporaryDirectory() as directory:
   path=Path(directory)/'scope.intent.json';report=scope.validate(candidate,control,KNOWN,path,'candidate-fingerprint');journal=json.loads(path.read_text());self.assertEqual(path.stat().st_mode&0o777,0o600)
   text=json.dumps(report);self.assertNotIn(KEY,text);self.assertNotIn('syntheticKey',text);self.assertNotIn('etag',text.lower());self.assertEqual(report['productionWrites'],0);self.assertEqual(report['productionGets'],0);self.assertFalse(report['activeCredentialChanged']);self.assertFalse(report['servicesRestarted'])
   self.assertTrue(all(c[0]=='HEAD' for c in control.calls));self.assertTrue(all(c[0]=='HEAD' for c in candidate.calls if c[1]==scope.PRODUCTION))
   return report,journal
 def test_exact_scope_positive_roundtrip_and_known_existing_prod_head_denial(self):
  client=Candidate();report,journal=self.execute(client);self.assertEqual(report['status'],'passed');self.assertEqual(report['productionHeads'],2);self.assertEqual(report['cleanupHeadStatus'],404);self.assertTrue(report['cleanupVerified']);self.assertEqual([c[0] for c in client.calls].count('PUT'),1);self.assertEqual([c[0] for c in client.calls].count('DELETE'),1);self.assertFalse(client.data)
 def test_overbroad_credential_fails_before_any_stage_write(self):
  client=Candidate(prod=200);report,_=self.execute(client);self.assertEqual(report['status'],'rejected');self.assertEqual(report['errorCode'],'production_head_not_denied');self.assertFalse(any(c[0] in ('PUT','DELETE') for c in client.calls))
 def test_missing_prod_object_and_404_do_not_prove_bucket_scope(self):
  for client,control in [(Candidate(prod=404,prod_code='NoSuchKey'),Control()),(Candidate(),Control(404))]:
   with self.subTest(control=control.status):
    report,_=self.execute(client,control);self.assertEqual(report['status'],'rejected');self.assertFalse(any(c[0]=='PUT' for c in client.calls))
 def test_invalid_auth_is_not_confused_with_scope_denial(self):
  for client in (Candidate(auth=False),Candidate(prod_code='SignatureDoesNotMatch')):
   report,_=self.execute(client);self.assertEqual(report['status'],'rejected');self.assertFalse(any(c[0]=='PUT' for c in client.calls))
 def test_lost_put_is_observed_without_put_replay_then_cleaned(self):
  client=Candidate(lost_put=True);report,journal=self.execute(client);self.assertEqual(report['status'],'passed');self.assertEqual(journal['putReply'],'uncertain');self.assertEqual([c[0] for c in client.calls].count('PUT'),1);self.assertFalse(client.data)
 def test_unobserved_put_stays_unverified_even_after_head404(self):
  client=Candidate(lost_put=True,put_missing=True);report,journal=self.execute(client);self.assertEqual(report['status'],'rejected');self.assertFalse(report['cleanupVerified']);self.assertEqual(journal['phase'],'inspection-required');self.assertEqual([c[0] for c in client.calls].count('PUT'),1)
 def test_lost_delete_is_inspected_without_delete_replay(self):
  client=Candidate(lost_delete=True);report,journal=self.execute(client);self.assertEqual(report['status'],'passed');self.assertEqual(journal['deleteReply'],'uncertain');self.assertEqual([c[0] for c in client.calls].count('DELETE'),1)
 def test_unresolved_delete_never_claims_cleanup(self):
  report,_=self.execute(Candidate(lost_delete=True,delete_missing=True));self.assertEqual(report['status'],'rejected');self.assertFalse(report['cleanupVerified'])
 def test_foreign_collision_and_wrong_bytes_are_never_deleted(self):
  for client in (Candidate(collision=True),Candidate(corrupt=True)):
   report,_=self.execute(client);self.assertEqual(report['status'],'rejected');self.assertFalse(any(c[0]=='DELETE' for c in client.calls));self.assertTrue(client.data)
 def test_original_operation_is_never_replayed(self):
  with tempfile.TemporaryDirectory() as directory:
   path=Path(directory)/'intent.json';client=Candidate();scope.validate(client,Control(),KNOWN,path,'fp');calls=list(client.calls)
   with self.assertRaises(FileExistsError):scope.validate(client,Control(),KNOWN,path,'fp')
   self.assertEqual(client.calls,calls)
 def test_cleanup_rejects_foreign_bucket_or_prefix_before_requests(self):
  for patch in ({'stagingBucket':scope.PRODUCTION},{'syntheticKey':'workspaces/customer-object'}):
   client=Candidate();journal={'stagingBucket':scope.STAGING,'syntheticKey':'scope-verification/'+'a'*32+'/roundtrip.bin',**patch}
   with self.assertRaisesRegex(scope.Rejected,'cleanup_scope_invalid'):scope.cleanup(client,journal,lambda:None)
   self.assertFalse(client.calls)
 def test_prepare_copies_only_known_public_settings_without_changing_active_file(self):
  with tempfile.TemporaryDirectory() as directory:
   root=Path(directory);active=root/'storage.env';active.write_text("CLOUD_TRANSCRIPT_R2_ENDPOINT='https://example.r2.cloudflarestorage.com'\nCLOUD_TRANSCRIPT_R2_BUCKET=zuse-rust-staging\nCLOUD_TRANSCRIPT_R2_ACCESS_KEY_ID=old-private-id\nCLOUD_TRANSCRIPT_R2_SECRET_ACCESS_KEY=old-private-secret\n");active.chmod(0o600);before=active.read_bytes();candidate=root/'candidate.env';scope.prepare(active,candidate)
   self.assertEqual(active.read_bytes(),before);self.assertNotIn('private',candidate.read_text());self.assertEqual(candidate.stat().st_mode&0o777,0o600)
 def test_private_config_rejects_symlinks_modes_duplicates_and_wrong_bucket(self):
  with tempfile.TemporaryDirectory() as directory:
   path=Path(directory)/'storage.env';path.write_text('anything');path.chmod(0o644)
   with self.assertRaisesRegex(scope.Rejected,'private_file_unsafe'):scope.private_read(path)
   path.chmod(0o600);link=path.with_name('link');link.symlink_to(path)
   with self.assertRaises(OSError):scope.private_read(link)
if __name__=='__main__':unittest.main()
