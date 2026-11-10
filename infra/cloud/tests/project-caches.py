#!/usr/bin/env python3
"""Optional cache migration, privilege and realtime fences on isolated PostgreSQL."""
import importlib.util
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[3]
spec=importlib.util.spec_from_file_location('cache_db_tests',Path(__file__).with_name('database.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
db=m.database
class ProjectCaches(m.DatabaseBootstrapTest):
    # Reuse local cluster setup, not the inherited unrelated release test cases.
    def test_optional_cache_schema_certificates_and_progress(self):
        self.assertNotIn('0028_project_environment_caches.sql',db.FILES)
        db.execute(self.url,db.render('staging'))
        migration=(ROOT/'crates/cloud/migrations/0028_project_environment_caches.sql').read_text()
        db.execute(self.url,'BEGIN; SET LOCAL search_path=zuse_staging,pg_catalog;'+migration+' COMMIT;')
        p=ROOT/'infra/cloud/single-host/provision.py';s=importlib.util.spec_from_file_location('cache_provision',p);provision=importlib.util.module_from_spec(s);s.loader.exec_module(provision)
        name=self.url.split('/')[-1].split('?')[0];db.execute(self.url,provision.role_sql('staging',name,'synthetic-only-password',True))
        app=self.url.replace('zuse_test@','zuse_cloud_staging:synthetic-only-password@')
        result=db.execute(app,"SELECT json_build_object('publish',has_table_privilege(current_user,'cloud_project_cache_capabilities','UPDATE'),'lock',has_function_privilege(current_user,'zuse_rust_project_cache_capability_lock(text,text)','EXECUTE'),'work',has_table_privilege(current_user,'zuse_rust_project_cache_operations','INSERT'));")[-1]
        self.assertEqual(result,dict(publish=False,lock=True,work=True))
        public=db.execute(self.url,"SELECT json_build_object('publicExecute',has_function_privilege('public','zuse_staging.zuse_rust_project_cache_capability_lock(text,text)','EXECUTE'));")[-1]
        self.assertFalse(public['publicExecute'])
        db.execute(app,"INSERT INTO cloud_environment_versions VALUES('owner','project',1,'project','{}',repeat('a',64),0); INSERT INTO zuse_rust_project_preparation_requests(request_id,owner_id,actor_id,project_id,environment_revision,recipe_digest,repository_identity,requested_ref,state,next_attempt_at,created_at,updated_at) VALUES('request','owner','owner','project',1,repeat('a',64),'github.com/example/project','main','queued',0,0,0);")
        before=db.execute(app,"SELECT json_build_object('n',count(*)) FROM zuse_rust_account_event_outbox;")[-1]['n']
        db.execute(app,"UPDATE zuse_rust_project_preparation_requests SET lease_id='private-test-lease',lease_expires_at=100,revision=revision+1,updated_at=1 WHERE request_id='request';")
        after=db.execute(app,"SELECT json_build_object('n',count(*)) FROM zuse_rust_account_event_outbox;")[-1]['n'];self.assertEqual(before,after)
        db.execute(app,"UPDATE zuse_rust_project_preparation_requests SET state='blocked',last_error_code='managed_base_cache_runtime_required',updated_at=2 WHERE request_id='request';")
        after=db.execute(app,"SELECT json_build_object('n',count(*),'safe',bool_and(NOT(payload ? 'leaseId') AND NOT(payload ? 'recipe'))) FROM zuse_rust_account_event_outbox;")[-1];self.assertEqual(after['n'],before+1);self.assertTrue(after['safe'])
        db.execute(self.url,"SET search_path=zuse_staging,pg_catalog; INSERT INTO cloud_managed_bases(provider,release_id,artifact_id,architecture,capabilities,state,created_at) VALUES('boxd','cache-fixture','artifact','x86_64','{}','verified',0);")
        db.execute(app,"INSERT INTO zuse_rust_project_cache_entries(cache_key,owner_id,project_id,provider,architecture,base_release_id,base_artifact_id,base_runtime_sha256,identity,recipe_digest,dependency_digest,source_commit,state,created_at) SELECT repeat(k,64),'owner','project','boxd','x86_64','cache-fixture','artifact',repeat('a',64),'{}',repeat('a',64),repeat('b',64),repeat('c',40),'preparing',0 FROM unnest(ARRAY['a','b']) k; INSERT INTO zuse_rust_project_cache_operations(operation_id,request_id,cache_key,owner_id,actor_id,project_id,provider,state,phase,plan,next_attempt_at,created_at,updated_at) VALUES('op-a','request',repeat('a',64),'owner','owner','project','boxd','uncertain','allocate','{}',0,0,0),('op-b','request',repeat('b',64),'owner','owner','project','boxd','queued','allocate','{}',0,0,0);")
        with self.assertRaisesRegex(ValueError,'zuse_schema_database_operation_failed'):
            db.execute(app,"UPDATE zuse_rust_project_cache_operations SET state='running' WHERE operation_id='op-b';")
        count=db.execute(app,"SELECT json_build_object('active',count(*)) FROM zuse_rust_project_cache_operations WHERE state IN ('running','uncertain','cleaning');")[-1]
        self.assertEqual(count['active'],1)

if __name__=='__main__':
    unittest.main(defaultTest='ProjectCaches.test_optional_cache_schema_certificates_and_progress')
