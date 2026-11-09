#!/usr/bin/env python3
"""Actual isolated PostgreSQL event-outbox semantics; never a deployed database."""
import importlib.util
from pathlib import Path
import re
import select
import subprocess
import unittest
import uuid

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('event_test_database',HERE/'database.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
database=fixture.database
MIGRATION=HERE.parents[2]/'crates/cloud/migrations/0017_account_events.sql'

class AccountEventOutboxTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fixture.DatabaseBootstrapTest.setUpClass()
        cls.admin=fixture.DatabaseBootstrapTest.admin
    @classmethod
    def tearDownClass(cls):fixture.DatabaseBootstrapTest.tearDownClass()
    def setUp(self):
        name='events_'+uuid.uuid4().hex
        database.execute(self.admin,'CREATE DATABASE "'+name+'";')
        self.url=self.admin.replace('/postgres?','/'+name+'?')
        database.execute(self.url,database.render('staging'))
        if MIGRATION.name not in database.FILES:
            self.sql(MIGRATION.read_text())
    def sql(self,sql):
        return database.execute(self.url,'BEGIN; SET LOCAL search_path TO zuse_staging,pg_catalog;\n'+sql+'\nCOMMIT;')
    def workspace(self):
        self.sql("""
INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at)
VALUES('p','owner','example/app','https://github.com/example/app','App','main','public','anonymous','digest','ready','p',1,1);
INSERT INTO api_cloud_project_builds(build_id,project_id,account_id,provider,template_version,configuration_digest,state,idempotency_key,next_action_at,created_at,updated_at)
VALUES('b','p','owner','boxd','v1','digest','ready','b',1,1,1);
INSERT INTO api_cloud_workspaces(workspace_id,account_id,project_id,build_id,provider,chat_id,initial_session_id,branch,base_ref,state,desired_state,status_code,idempotency_key,next_action_at,created_at,updated_at,last_activity_at)
VALUES('w','owner','p','b','boxd','chat','session','task','main','queued','ready','queued','w',1,1,1,1);
""")
    def test_workspace_outbox_is_atomic_compact_and_ignores_heartbeat_only_writes(self):
        self.workspace()
        self.sql("UPDATE api_cloud_workspaces SET last_activity_at=9,updated_at=10,request_config='{\"secret\":\"must-not-enter-event\"}' WHERE workspace_id='w';")
        row=self.sql("SELECT json_build_object('count',count(*),'leaks',bool_or(payload::text LIKE '%must-not-enter-event%')) FROM zuse_rust_account_event_outbox WHERE kind='workspace';")[-1]
        self.assertEqual(row,{'count':1,'leaks':False})
        self.sql("SAVEPOINT failed_domain; UPDATE api_cloud_workspaces SET state='provisioning',revision=revision+1 WHERE workspace_id='w'; ROLLBACK TO SAVEPOINT failed_domain;")
        self.assertEqual(self.sql("SELECT json_build_object('count',count(*)) FROM zuse_rust_account_event_outbox WHERE kind='workspace';")[-1]['count'],1)
        self.sql("UPDATE api_cloud_workspaces SET state='provisioning',revision=revision+1 WHERE workspace_id='w'; DELETE FROM api_cloud_workspaces WHERE workspace_id='w';")
        row=self.sql("SELECT json_build_object('count',count(*),'deleted',bool_or(payload->>'deleted'='true')) FROM zuse_rust_account_event_outbox WHERE kind='workspace';")[-1]
        self.assertEqual(row,{'count':3,'deleted':True})
    def test_refresh_leases_do_not_publish_credential_or_token_events(self):
        self.sql("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,credential_ciphertext,created_at,updated_at) VALUES('c','owner','c','digest','codex','device-code','connected','encrypted-token',1,1);")
        self.sql("UPDATE cloud_agent_connections SET refresh_lease_id='private-lease',refresh_lease_expires_at=500,updated_at=2 WHERE connection_id='c';")
        row=self.sql("SELECT json_build_object('count',count(*),'leaks',bool_or(payload::text LIKE '%token%' OR payload::text LIKE '%lease%')) FROM zuse_rust_account_event_outbox;")[-1]
        self.assertEqual(row,{'count':1,'leaks':False})
        self.sql("UPDATE cloud_agent_connections SET state='revoked',version=version+1 WHERE connection_id='c';")
        self.assertEqual(self.sql("SELECT json_build_object('count',count(*)) FROM zuse_rust_account_event_outbox;")[-1]['count'],2)
    def test_environment_and_github_transitions_publish_only_safe_invalidations(self):
        self.workspace()
        self.sql("INSERT INTO cloud_environment_versions(owner_id,project_id,revision,source,recipe,digest,created_at) VALUES('owner','p',1,'project','{\"secrets\":\"never-publish-this\"}','digest',1);")
        self.sql("INSERT INTO zuse_rust_github_users(actor_id,sealed_credentials,state,updated_at) VALUES('actor','never-publish-this','connected',1); INSERT INTO zuse_rust_github_scopes(owner_id) VALUES('owner');")
        self.sql("INSERT INTO zuse_rust_github_operations(operation_id,state_hash,actor_id,owner_id,actor_epoch,scope_epoch,phase,sealed_pending,expires_at,created_at) VALUES('op','hash','actor','owner',0,0,'created','never-publish-this',100,1);")
        self.sql("UPDATE zuse_rust_github_operations SET browser_hash='private-browser-cookie' WHERE operation_id='op'; UPDATE zuse_rust_github_operations SET phase='authorizing' WHERE operation_id='op';")
        rows=self.sql("SELECT json_build_object('environments',count(*) FILTER(WHERE kind='environment'),'github',count(*) FILTER(WHERE kind='github'),'leaks',bool_or(payload::text LIKE '%never-publish-this%' OR payload::text LIKE '%private-browser-cookie%'),'boundActor',bool_or(resource_id='op' AND payload->>'actorId'='actor')) FROM zuse_rust_account_event_outbox;")[-1]
        self.assertEqual(rows,{'environments':2,'github':3,'leaks':False,'boundActor':True})

    def test_outbox_contains_no_database_notification_or_dynamic_external_calls(self):
        body=MIGRATION.read_text().lower()
        # Exclude comments, whose purpose is to document the rejected mechanism.
        executable='\n'.join(line for line in body.splitlines() if not line.lstrip().startswith('--'))
        self.assertNotIn('pg_notify',executable)
        self.assertNotIn('notify ',executable)
        self.assertNotIn('http_',executable)

    def test_late_domain_commit_receives_a_new_cursor_after_earlier_delivery(self):
        source=(HERE.parents[2]/'crates/cloud/src/events/store.rs').read_text()
        dispatch=re.search(r'r#"(\s*WITH picked AS MATERIALIZED.*?\n)"#',source,re.S).group(1).replace('$1','128')+';'
        writer=subprocess.Popen(['psql','-X','--no-password','--no-align','--tuples-only','--quiet','-v','ON_ERROR_STOP=1'],env=database.connection_environment(self.url),stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        try:
            writer.stdin.write("BEGIN; SET LOCAL search_path TO zuse_staging,pg_catalog; INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,created_at) VALUES('owner','connection','committed-later',1); SELECT 'staged';\n");writer.stdin.flush()
            self.assertTrue(select.select([writer.stdout],[],[],5)[0],'Writer never staged')
            self.assertEqual(writer.stdout.readline().strip(),'staged')
            self.sql("INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,created_at) VALUES('owner','connection','committed-before',2);")
            self.sql(dispatch)
            first=self.sql("SELECT json_build_object('cursor',max(sequence),'id',min(resource_id)) FROM zuse_rust_account_events WHERE owner_id='owner';")[-1]
            self.assertEqual(first,{'cursor':1,'id':'committed-before'})
            writer.stdin.write('COMMIT;\n\\q\n');writer.stdin.flush();writer.wait(timeout=5)
            self.assertEqual(writer.returncode,0)
            self.sql(dispatch)
            # A delivery retry after a lost reply consumes no additional cursor.
            self.sql(dispatch)
            result=self.sql("SELECT json_build_object('ids',(SELECT json_agg(resource_id ORDER BY sequence) FROM zuse_rust_account_events WHERE owner_id='owner'),'cursor',(SELECT next_sequence FROM zuse_rust_account_event_cursors WHERE owner_id='owner'),'pending',(SELECT count(*) FROM zuse_rust_account_event_outbox));")[-1]
            self.assertEqual(result,{'ids':['committed-before','committed-later'],'cursor':3,'pending':0})
        finally:
            if writer.poll() is None:writer.terminate();writer.wait(timeout=5)
            writer.stdin.close();writer.stdout.close();writer.stderr.close()

if __name__=='__main__':unittest.main()
