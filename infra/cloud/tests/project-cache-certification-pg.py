#!/usr/bin/env python3
"""Real isolated local PostgreSQL fences; all certificates here are synthetic."""
import importlib.util,json,os,tempfile,unittest,subprocess
from unittest.mock import patch
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def load(n,p):
 s=importlib.util.spec_from_file_location(n,p);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
base=load('local_database_tests',Path(__file__).with_name('database.py'))
f=load('cache_cert_test_fixture',Path(__file__).with_name('project-cache-certification.py'));tool=f.m
class PgTests(base.DatabaseBootstrapTest):
 def test_operator_transaction_and_replay_failure_fences(self):
  with tempfile.TemporaryDirectory() as td:
   root=Path(td);release=f.fixture(root);plan=tool.make_plan(root/'release.json',root/'base-report.json',root);f.write(root/'plan.json',plan);approved=tool.digest((root/'plan.json').read_bytes())
   (root/'dsn').write_text(self.url);(root/'dsn').chmod(0o600)
   db=base.database;lit=db.literal
   ddl="""CREATE SCHEMA zuse_staging;SET search_path=zuse_staging,pg_catalog;
CREATE TABLE zuse_cloud_schema_identity(singleton boolean PRIMARY KEY,environment text);INSERT INTO zuse_cloud_schema_identity VALUES(true,'staging');
CREATE TABLE cloud_managed_bases(provider text,release_id text,artifact_id text,architecture text,state text,runtime_manifest jsonb,PRIMARY KEY(provider,release_id));
"""
   # Use the actual capability DDL, not a relaxed replacement.
   cap=(ROOT.parents[1]/'crates/cloud/migrations/0028_project_environment_caches.sql').read_text().split('CREATE TABLE zuse_rust_project_preparation_requests')[0]
   db.execute(self.url,ddl+cap)
   db.execute(self.url,"SET search_path=zuse_staging,pg_catalog;INSERT INTO cloud_managed_bases VALUES('boxd','synthetic-base','synthetic-base-snapshot','x86_64','verified',"+lit(tool.canonical(release).decode())+"::jsonb);")
   def apply(name,url=None):return tool.apply(root/'plan.json',approved,root/'release.json',root/'base-report.json',root,url or root/'dsn',root/(name+'.json'))
   def query(sql):return db.execute(self.url,'SET search_path=zuse_staging,pg_catalog;'+sql)
   # The actual verified row must be present and byte/semantic bindings exact.
   for name,change,restore in [
    ('candidate',"state='preparing'","state='verified'"),
    ('retired',"state='retired'","state='verified'"),
    ('artifact',"artifact_id='other'","artifact_id='synthetic-base-snapshot'"),
    ('manifest',"runtime_manifest='{}'","runtime_manifest="+lit(tool.canonical(release).decode())+"::jsonb")]:
    query('UPDATE cloud_managed_bases SET '+change)
    with self.subTest(name=name),self.assertRaisesRegex(tool.Rejected,'verified_base_required'):apply(name)
    self.assertEqual(query("SELECT json_build_object('n',count(*)) FROM cloud_project_cache_capabilities")[-1]['n'],0)
    query('UPDATE cloud_managed_bases SET '+restore)
   query("UPDATE zuse_cloud_schema_identity SET environment='production'")
   with self.assertRaisesRegex(tool.Rejected,'staging_identity'):apply('wrong-environment')
   query("UPDATE zuse_cloud_schema_identity SET environment='staging'")
   # Even an accidentally over-granted application role may not publish.
   db.execute(self.url,"CREATE ROLE zuse_cloud_staging LOGIN;GRANT USAGE ON SCHEMA zuse_staging TO zuse_cloud_staging;GRANT SELECT,INSERT,UPDATE ON ALL TABLES IN SCHEMA zuse_staging TO zuse_cloud_staging;")
   app=self.url.replace('zuse_test@','zuse_cloud_staging@');(root/'app-dsn').write_text(app);(root/'app-dsn').chmod(0o600)
   with self.assertRaisesRegex(tool.Rejected,'operator_role'):apply('application-role',root/'app-dsn')
   original_read=tool.read;release_reads=[]
   def race_release(path,*args):
    if Path(path)==root/'release.json':
     release_reads.append(1)
     if len(release_reads)>1:return tool.canonical(dict(release,artifactId='swapped'))
    return original_read(path,*args)
   with patch.object(tool,'read',side_effect=race_release):positive=apply('operator-success')
   self.assertEqual(len(release_reads),1);self.assertEqual(positive['status'],'certified')
   row=query("SELECT json_build_object('receipt',receipt,'runtime',runtime_sha256,'state',state) FROM cloud_project_cache_capabilities")[-1]
   self.assertEqual(row['receipt'],plan['receipt']);self.assertEqual(row['state'],'certified')
   # Existing successful/uncertain original intent is never blindly executed.
   with self.assertRaisesRegex(tool.Rejected,'inspect_original_apply'):apply('operator-success')
   with self.assertRaisesRegex(tool.Rejected,'inspect_original_apply'):apply('candidate')
   query("UPDATE cloud_project_cache_capabilities SET receipt=jsonb_set(receipt,'{preparationCaptureVersion}','0')")
   with self.assertRaisesRegex(tool.Rejected,'replay_mismatch'):apply('mismatched-existing')
   self.assertEqual(query("SELECT json_build_object('v',receipt->'preparationCaptureVersion') FROM cloud_project_cache_capabilities")[-1]['v'],0)
   # Lost COMMIT observation cannot cause another write. Reconcile original via
   # SELECT; the immutable capability and operator intent retain exact identity.
   self.assertTrue((root/'mismatched-existing.json.intent.json').exists())
   self.assertFalse((root/'mismatched-existing.json').exists())
   query("DELETE FROM cloud_project_cache_capabilities")
   real_run=tool.subprocess.run
   def commit_then_lose(*args,**kwargs):
    assert not any(k in kwargs['env'] for k in ('BOXD_API_KEY','BOAT_API_KEY','ZUSE_CLOUD_DATABASE_URL'))
    result=real_run(*args,**kwargs);assert result.returncode==0
    raise subprocess.TimeoutExpired(args[0],30)
   with patch.object(tool.subprocess,'run',side_effect=commit_then_lose):
    with self.assertRaises(subprocess.TimeoutExpired):apply('lost-commit')
   self.assertFalse((root/'lost-commit.json').exists());self.assertTrue((root/'lost-commit.json.intent.json').exists())
   observed=tool.inspect_original(root/'plan.json',approved,root/'release.json',root/'base-report.json',root,root/'dsn',root/'lost-commit.json')
   self.assertEqual(observed['status'],'original-commit-observed');self.assertTrue(observed['readOnly'])
   with patch.object(tool.subprocess,'run',side_effect=AssertionError('No write retry')):
    with self.assertRaisesRegex(tool.Rejected,'inspect_original_apply'):apply('lost-commit')
   query("UPDATE cloud_managed_bases SET state='retired'")
   self.assertEqual(tool.inspect_original(root/'plan.json',approved,root/'release.json',root/'base-report.json',root,root/'dsn',root/'lost-commit.json')['status'],'original-not-ready')
if __name__=='__main__':unittest.main(defaultTest='PgTests.test_operator_transaction_and_replay_failure_fences')
