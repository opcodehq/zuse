#!/usr/bin/env python3
"""Real isolated PostgreSQL bootstrap tests. Never reads deployment credentials."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "database.py"
spec = importlib.util.spec_from_file_location("cloud_database", SCRIPT)
database = importlib.util.module_from_spec(spec)
spec.loader.exec_module(database)


class DatabaseBootstrapTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not all(shutil.which(binary) for binary in ("initdb", "pg_ctl", "psql")):
            raise unittest.SkipTest("PostgreSQL client/server tools required")
        cls.temporary = tempfile.TemporaryDirectory(prefix="zuse-schema-test-")
        cls.root = Path(cls.temporary.name)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            cls.port = sock.getsockname()[1]
        cls.clean_env = {key: value for key, value in os.environ.items() if not key.startswith("PG")}
        subprocess.run(["initdb", "-D", str(cls.root / "data"), "--auth=trust", "--username=zuse_test", "--no-locale"],
                       env=cls.clean_env, check=True, capture_output=True)
        subprocess.run(["pg_ctl", "-D", str(cls.root / "data"), "-l", str(cls.root / "server.log"),
                        "-o", f"-h 127.0.0.1 -p {cls.port} -k {cls.root}", "-w", "start"],
                       env=cls.clean_env, check=True, capture_output=True)
        cls.admin = f"postgresql://zuse_test@127.0.0.1:{cls.port}/postgres?sslmode=disable"

    @classmethod
    def tearDownClass(cls):
        try:
            subprocess.run(["pg_ctl", "-D", str(cls.root / "data"), "-m", "fast", "-w", "stop"],
                           env=cls.clean_env, check=True, capture_output=True)
        finally:
            cls.temporary.cleanup()

    def setUp(self):
        import uuid
        name = "bootstrap_" + uuid.uuid4().hex
        database.execute(self.admin, f'CREATE DATABASE "{name}";')
        self.url = f"postgresql://zuse_test@127.0.0.1:{self.port}/{name}?sslmode=disable"

    def test_release_manifest_contains_command_and_recreation_after_admission(self):
        expected=("0036_harness_bundles.sql","0037_user_images.sql","0038_app_role_grants.sql","0041_user_image_repositories.sql","0042_machine_versions_grants.sql","0043_work_wakeups.sql","0044_worker_due_index.sql","0045_auth_wakeups.sql","0046_plugin_wakeups.sql","0048_executor_wakeups.sql","0049_change_hints.sql","0050_archive_machine_deletion.sql","0051_archive_machine_due_cap.sql","0052_one_environment_credential.sql","0053_provider_machine_reconciliation.sql","0054_machine_minute_plans.sql","0055_device_command_approvals.sql","0056_bridge_synced_checkout.sql","0057_computer_request_hints.sql","0058_pull_request_owners.sql")
        self.assertEqual(database.FILES[-len(expected):],expected)
        self.assertEqual(len(database.FILES),53)
        self.assertIn("0026_task_command_admission.sql",database.FILES)
        self.assertNotIn("0022_computer_recreations.sql",database.FILES)
        self.assertNotIn("0022_recreation.sql",database.FILES)

    def test_detach_removes_only_matching_requests_and_session_grants(self):
        database.execute(self.url, database.render("staging"))
        rows = database.execute(self.url, """
            SET search_path TO zuse_staging;
            ALTER TABLE api_cloud_workspaces
                DROP CONSTRAINT api_cloud_workspaces_project_id_api_cloud_projects_project_id_fk,
                DROP CONSTRAINT api_cloud_workspaces_build_id_api_cloud_project_builds_build_id_fk;
            INSERT INTO api_cloud_workspaces(workspace_id,account_id,project_id,build_id,provider,
                chat_id,initial_session_id,branch,base_ref,state,desired_state,status_code,idempotency_key,
                next_action_at,created_at,updated_at,last_activity_at)
                SELECT w,o,'p','b','boxd',w,'chat',w,'main','ready','ready','ready',w,1,1,1,1
                FROM (VALUES ('w','a'),('w2','b')) AS hosts(w,o);
            INSERT INTO zuse_rust_task_command_policies
                SELECT workspace_id,account_id,1,'{}',1,1 FROM api_cloud_workspaces;
            INSERT INTO zuse_rust_task_command_sessions(workspace_id,session_id,thread_id,state,created_at)
                SELECT workspace_id,'chat','thread','active',1 FROM api_cloud_workspaces;
            INSERT INTO api_bridge_computers VALUES
                ('a','mac','key1','Mac','macos'),('a','linux','key2','Linux','linux'),
                ('b','mac','key3','Mac','macos');
            INSERT INTO api_bridge_attachments VALUES
                ('a','w','chat','mac'), ('a','w','chat','linux'), ('b','w2','chat','mac');
            INSERT INTO api_bridge_session_grants SELECT owner_id,workspace_id,chat_id,enrollment_id FROM api_bridge_attachments;
            INSERT INTO api_bridge_always_grants VALUES ('a','mac');
            INSERT INTO api_bridge_requests
                (request_id,owner_id,workspace_id,chat_id,enrollment_id,computer_name,
                 os_user,shell,cwd,command,timeout_ms)
                SELECT gen_random_uuid(),owner_id,workspace_id,chat_id,enrollment_id,
                    'host','me','sh','/tmp','pwd',1000 FROM api_bridge_attachments;
            DELETE FROM api_bridge_attachments
                WHERE owner_id='a' AND workspace_id='w' AND chat_id='chat' AND enrollment_id='mac';
            SELECT json_build_object(
                'requests',(SELECT count(*) FROM api_bridge_requests),
                'sessions',(SELECT count(*) FROM api_bridge_session_grants),
                'always',(SELECT count(*) FROM api_bridge_always_grants),
                'detachedPending',EXISTS(SELECT FROM api_bridge_requests WHERE owner_id='a' AND enrollment_id='mac'));
        """)
        self.assertEqual(rows[-1], {"requests": 2, "sessions": 2, "always": 1, "detachedPending": False})

        # 0056 is additive: existing attachments have no synced folder, and
        # the column only takes a bounded path.
        synced = database.execute(self.url, """
            SET search_path TO zuse_staging;
            SELECT json_build_object(
                'nullable',(SELECT is_nullable FROM information_schema.columns
                    WHERE table_schema='zuse_staging' AND table_name='api_bridge_attachments' AND column_name='synced_cwd'),
                'unset',(SELECT count(*) FROM api_bridge_attachments WHERE synced_cwd IS NULL),
                'all',(SELECT count(*) FROM api_bridge_attachments));
        """)
        self.assertEqual(synced[-1]["nullable"], "YES")
        self.assertEqual(synced[-1]["unset"], synced[-1]["all"])
        with self.assertRaises(ValueError):
            database.execute(self.url, "SET search_path TO zuse_staging; UPDATE api_bridge_attachments SET synced_cwd='';")

        after_archive = database.execute(self.url, """
            SET search_path TO zuse_staging;
            UPDATE zuse_rust_task_command_sessions SET state='archived' WHERE workspace_id='w';
            DELETE FROM api_bridge_computers WHERE owner_id='a' AND enrollment_id='mac';
            SELECT json_build_object('requests',(SELECT count(*) FROM api_bridge_requests),
                'sessions',(SELECT count(*) FROM api_bridge_session_grants),
                'always',(SELECT count(*) FROM api_bridge_always_grants));
        """)
        self.assertEqual(after_archive[-1], {"requests": 1, "sessions": 1, "always": 0})

    def test_each_desktop_app_keeps_one_credential_row(self):
        database.execute(self.url, database.render("production"))
        migration = (database.MIGRATIONS / "0052_one_environment_credential.sql").read_text()
        seed = """SET search_path TO zuse_production;
DROP INDEX api_environment_credentials_environment_unique;
INSERT INTO api_environments(environment_id,account_id,provider_kind,linked_at,environment_public_key,http_base_url,ws_base_url) VALUES
 ('env-a','user-a','desktop',1,'{}','h','w'),('env-b','user-b','desktop',1,'{}','h','w'),('env-c','user-c','desktop',1,'{}','h','w');
INSERT INTO api_environment_credentials(credential_id,environment_id,account_id,credential_hash,created_at,revoked_at) VALUES
 ('a1','env-a','user-a','h-a1',10,NULL),('a2','env-a','user-a','h-a2',30,NULL),('a3','env-a','user-a','h-a3',20,NULL),
 ('b1','env-b','user-b','h-b1',10,5),('b2','env-b','user-b','h-b2',5,NULL),
 ('c1','env-c','user-c','h-c1',7,NULL);
"""
        database.execute(self.url, seed + migration)
        rows = subprocess.run(["psql", self.url, "-X", "-q", "-t", "-A", "-c",
            "SELECT string_agg(environment_id||'='||credential_hash, ',' ORDER BY environment_id) FROM zuse_production.api_environment_credentials"],
            env=self.clean_env, check=True, capture_output=True, text=True).stdout.strip()
        self.assertEqual(rows, "env-a=h-a2,env-b=h-b2,env-c=h-c1")
        duplicate = subprocess.run(["psql", self.url, "-X", "-q", "-v", "ON_ERROR_STOP=1", "-c",
            "INSERT INTO zuse_production.api_environment_credentials VALUES ('a4','env-a','user-a','h-a4',40,NULL)"],
            env=self.clean_env, capture_output=True, text=True)
        self.assertNotEqual(duplicate.returncode, 0)
        self.assertIn("api_environment_credentials_environment_unique", duplicate.stderr)

    def test_work_triggers_notify_only_their_schema_after_commit(self):
        database.execute(self.url, database.render("production"))
        database.execute(self.url, database.render("staging"))
        sql = """SET search_path TO zuse_production;
LISTEN "zuse_cloud_work:zuse_production";
INSERT INTO zuse_rust_jobs(job_id,idempotency_key,kind,resource_id,resource_generation,payload,due_at,created_at,updated_at) VALUES('j1','k1','lifecycle','w1',1,'{}',1,1,1);
UPDATE zuse_rust_jobs SET state='running',lease_token='t',lease_expires_at=5 WHERE job_id='j1';
SELECT 'after-claim';
UPDATE zuse_rust_jobs SET state='completed',lease_token=NULL,lease_expires_at=NULL WHERE job_id='j1';
SELECT 'after-complete';
INSERT INTO zuse_staging.zuse_rust_jobs(job_id,idempotency_key,kind,resource_id,resource_generation,payload,due_at,created_at,updated_at) VALUES('j2','k2','lifecycle','w1',1,'{}',1,1,1);
SELECT 'after-other-schema';
BEGIN;
INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,created_at) VALUES('o','workspace','w1',1),('o','workspace','w2',1);
SELECT 'inside-transaction';
COMMIT;
INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,created_at,updated_at) VALUES('c1','o','k','d','codex','device-code','pending',1,1);
INSERT INTO cloud_auth_operations(operation_id,owner_id,actor_id,connection_id,idempotency_key,state,expires_at,created_at,updated_at) VALUES('a1','o','o','c1','k','queued',9,1,1);
SELECT 'after-signin';
UPDATE cloud_auth_operations SET state='authorizing',lease_id='l',lease_expires_at=5 WHERE operation_id='a1';
SELECT 'after-auth-claim';
"""
        result = subprocess.run(["psql", "-X", "--no-align", "--tuples-only", "-v", "ON_ERROR_STOP=1"], input=sql,
                                env=database.connection_environment(self.url), capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = [line for line in result.stdout.splitlines() if line.strip()]
        notices = [line for line in lines if line.startswith("Asynchronous notification")]
        # Connection changes also feed the account-event outbox; look at the
        # job and outbox section first, then at the credential worker's.
        section = lines.index("COMMIT") + 2
        auth = [line for line in notices if 'payload "auth"' in line]
        notices = [line for line in lines[:section] if line in notices]
        self.assertEqual(len(notices), 2, result.stdout)
        self.assertIn('payload "jobs"', notices[0])
        self.assertIn('payload "account_events"', notices[1])
        # A new sign-in wakes the credential worker once; its claim does not.
        self.assertEqual(len(auth), 1, result.stdout)
        self.assertLess(lines.index(auth[0]), lines.index("after-signin"))
        # Claims, completions and the other schema stay silent; the outbox
        # notification arrives once, after commit.
        self.assertLess(lines.index(notices[0]), lines.index("after-claim"))
        self.assertLess(lines.index("inside-transaction"), lines.index(notices[1]))

    def test_archive_deadline_schedules_machine_deletion_and_wakes_worker(self):
        database.execute(self.url, database.render("production"))
        # Only the workspace row matters here; skip its project/build fixtures.
        sql = """SET search_path TO zuse_production;
ALTER TABLE api_cloud_workspaces DROP CONSTRAINT api_cloud_workspaces_project_id_api_cloud_projects_project_id_fk, DROP CONSTRAINT api_cloud_workspaces_build_id_api_cloud_project_builds_build_id_fk;
INSERT INTO api_cloud_workspaces(workspace_id,account_id,project_id,build_id,provider,provider_sandbox_id,chat_id,initial_session_id,branch,base_ref,state,desired_state,status_code,idempotency_key,next_action_at,created_at,updated_at,last_activity_at) VALUES('w1','o','p','b','boxd','m1','c','s','main','main','paused','paused','paused','k',1,1,1,1);
LISTEN "zuse_cloud_work:zuse_production";
UPDATE api_cloud_workspaces SET state='archived',desired_state='archived',archive_requested_at=100,archive_delete_at=604800100 WHERE workspace_id='w1';
SELECT 'archived:'||machine_delete_due_at FROM api_cloud_workspaces;
UPDATE api_cloud_workspaces SET machine_delete_due_at=604900000 WHERE workspace_id='w1';
SELECT 'backoff:'||machine_delete_due_at FROM api_cloud_workspaces;
UPDATE api_cloud_workspaces SET status_code='archived' WHERE workspace_id='w1';
SELECT 'unrelated:'||machine_delete_due_at FROM api_cloud_workspaces;
UPDATE api_cloud_workspaces SET state='paused',desired_state='paused',archive_requested_at=NULL,archive_delete_at=NULL WHERE workspace_id='w1';
SELECT 'unarchived:'||coalesce(machine_delete_due_at::text,'none') FROM api_cloud_workspaces;
UPDATE api_cloud_workspaces SET state='archived',desired_state='archived',archive_requested_at=200,archive_delete_at=864000200 WHERE workspace_id='w1';
SELECT 'ten-day-writer:'||machine_delete_due_at||':'||archive_delete_at FROM api_cloud_workspaces;
"""
        result = subprocess.run(["psql", "-X", "--no-align", "--tuples-only", "-v", "ON_ERROR_STOP=1"], input=sql,
                                env=database.connection_environment(self.url), capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = [line for line in result.stdout.splitlines() if line.strip()]
        self.assertIn("archived:604800100", lines)
        self.assertIn("backoff:604900000", lines)
        self.assertIn("unrelated:604900000", lines)
        self.assertIn("unarchived:none", lines)
        # A previous-release writer's ten-day deadline is capped at seven days.
        self.assertIn("ten-day-writer:604800200:864000200", lines)
        wakes = [line for line in lines if 'payload "archive_retention"' in line]
        # Each archive wakes the worker once; its own back-off stays silent.
        self.assertEqual(len(wakes), 2, result.stdout)
        self.assertLess(lines.index(wakes[0]), lines.index("archived:604800100"))

    def test_billing_and_machine_image_changes_append_one_hint_per_transaction(self):
        database.execute(self.url, database.render("production"))
        sql = """SET search_path TO zuse_production;
BEGIN;
INSERT INTO api_cloud_billing_ledger(entry_id,period_id,account_id,kind,amount_micros,source_id,occurred_at,created_at) VALUES('l1','p1','owner','provider-cost',1,'s1',1,1),('l2','p1','owner','provider-cost',1,'s2',1,1);
INSERT INTO api_entitlements(entitlement_id,account_id,kind,provider,status,created_at,updated_at) VALUES('e1','owner','cloud-workspace','polar','active',1,1);
COMMIT;
UPDATE api_entitlements SET updated_at=2 WHERE entitlement_id='e1';
UPDATE api_entitlements SET status='grace' WHERE entitlement_id='e1';
INSERT INTO cloud_machine_versions(owner_id,revision,actor_id,recipe,digest,created_at) VALUES('owner',1,'owner','{}',repeat('a',64),1);
SELECT kind||':'||resource_id||':'||payload::text FROM zuse_rust_account_event_outbox ORDER BY outbox_id;
"""
        result = subprocess.run(["psql", "-X", "--no-align", "--tuples-only", "-v", "ON_ERROR_STOP=1"], input=sql,
                                env=database.connection_environment(self.url), capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        rows = [line for line in result.stdout.splitlines() if ":" in line]
        # One billing hint for the batch, none for a timestamp-only update,
        # one for the status change; hints never carry payloads.
        self.assertEqual(rows, ["billing:p1:{}", "billing:e1:{}", "machine-image:owner:{}"], result.stdout)

    def test_fresh_staging_and_production_are_empty_isolated_and_idempotent(self):
        for environment in ("staging", "production"):
            first = database.execute(self.url, database.render(environment))[-1]
            second = database.execute(self.url, database.render(environment))[-1]
            self.assertEqual(first, second)
            self.assertEqual(first["migrationCount"], len(database.FILES))
            self.assertGreater(first["applicationTableCount"], 40)
        result = database.execute(self.url, """SELECT json_build_object(
            'staging', (SELECT count(*) FROM zuse_staging.api_cloud_workspaces),
            'production', (SELECT count(*) FROM zuse_production.api_cloud_workspaces),
            'publicTables', (SELECT count(*) FROM pg_tables WHERE schemaname='public'));
        """)[-1]
        self.assertEqual(result, {"staging": 0, "production": 0, "publicTables": 0})
        env = database.connection_environment(self.url, self.clean_env)
        env["ZUSE_CLOUD_DATABASE_URL"] = self.url
        checked = subprocess.run(["python3", str(SCRIPT), "--environment", "staging", "check"],
                                 env=env, check=True, text=True, capture_output=True)
        self.assertEqual(json.loads(checked.stdout)["environment"], "staging")

    def test_application_roles_are_isolated_from_other_environment_and_ledgers(self):
        provision_spec = importlib.util.spec_from_file_location("cloud_provision", SCRIPT.parent / "single-host/provision.py")
        provision = importlib.util.module_from_spec(provision_spec)
        provision_spec.loader.exec_module(provision)
        for environment in ("staging", "production"):
            database.execute(self.url, database.render(environment))
            name = self.url.split("/")[-1].split("?")[0]
            database.execute(self.url, provision.role_sql(environment, name, "synthetic-test-password", True))
        for environment in ("staging", "production"):
            role = f"zuse_cloud_{environment}"
            other = "zuse_production" if environment == "staging" else "zuse_staging"
            url = self.url.replace("zuse_test@", role + ":synthetic-test-password@")
            result = database.execute(url, f"SELECT json_build_object('schema',current_schema(),'otherUsage',has_schema_privilege(current_user,'{other}','USAGE'),'ledgerWrite',has_table_privilege(current_user,'zuse_cloud_schema_migrations','UPDATE'),'workspaceWrite',has_table_privilege(current_user,'api_cloud_workspaces','INSERT'),'baseWrite',has_table_privilege(current_user,'cloud_managed_bases','INSERT,UPDATE,DELETE'),'baseLockExecutable',has_function_privilege(current_user,current_schema()||'.zuse_rust_managed_base_admission_lock(text,text)','EXECUTE')); ")[-1]
            self.assertEqual(result, {"schema": f"zuse_{environment}", "otherUsage": False, "ledgerWrite": False, "workspaceWrite": True, "baseWrite": False, "baseLockExecutable": True})
            rights = database.execute(url, """SELECT json_build_object(
                'editCommand',has_column_privilege(current_user,'api_bridge_requests','command','UPDATE'),
                'decide',has_column_privilege(current_user,'api_bridge_requests','decision','UPDATE'),
                'syncedFolder',has_column_privilege(current_user,'api_bridge_attachments','synced_cwd','UPDATE'));""")[-1]
            self.assertEqual(rights, {"editCommand": False, "decide": True, "syncedFolder": True})
        key = json.loads(provision.signing_key())
        self.assertEqual(key["crv"], "Ed25519")
        self.assertEqual(len(key["d"]), 43)

    def test_checksum_drift_is_rejected(self):
        database.execute(self.url, database.render("staging"))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in database.FILES:
                shutil.copyfile(database.MIGRATIONS / name, root / name)
            with (root / database.FILES[0]).open("a") as target:
                target.write("\n-- drift\n")
            with self.assertRaisesRegex(ValueError, "zuse_schema_checksum_mismatch"):
                database.execute(self.url, database.render("staging", root))

    def test_brokeboy_optional_install_is_explicit_idempotent_and_checked(self):
        database.execute(self.url,database.render("staging"))
        role_spec=importlib.util.spec_from_file_location("billing_provision",SCRIPT.parent / "single-host/provision.py")
        provision=importlib.util.module_from_spec(role_spec)
        role_spec.loader.exec_module(provision)
        name=self.url.split("/")[-1].split("?")[0]
        role=database.execute(self.url,"SELECT json_build_object('present',EXISTS(SELECT 1 FROM pg_roles WHERE rolname='zuse_cloud_staging'));")[-1]
        database.execute(self.url,provision.role_sql("staging",name,"synthetic-test-password",not role["present"]))
        before=database.execute(self.url,"SELECT json_build_object('present',to_regclass('zuse_staging.brokeboy_customers') IS NOT NULL);")[-1]
        self.assertFalse(before["present"])
        for _ in range(2):
            database.execute(self.url,database.render_brokeboy("staging"))
        result=database.execute(self.url,"SET search_path=zuse_staging,pg_catalog; SELECT json_build_object('count',(SELECT count(*) FROM zuse_cloud_schema_migrations WHERE name='0032_brokeboy.sql'),'migrations',(SELECT json_agg(json_build_object('name',name,'sha256',sha256)) FROM zuse_cloud_schema_migrations));")[-1]
        self.assertEqual(result["count"],1)
        self.assertIn("0033_shared_credits.sql", [m["name"] for m in result["migrations"]])
        self.assertIn("0047_billing_wakeups.sql", [m["name"] for m in result["migrations"]])
        trigger=database.execute(self.url,"SELECT json_build_object('present',EXISTS(SELECT 1 FROM pg_trigger WHERE tgname='zuse_cloud_billing_hold_wake' AND tgrelid='zuse_staging.api_cloud_billing_periods'::regclass));")[-1]
        self.assertTrue(trigger["present"])
        self.assertTrue(database.manifest_matches("staging",result["migrations"]))
        grants=database.execute(self.url,"SELECT json_build_object('billing',has_table_privilege('zuse_cloud_staging','zuse_staging.brokeboy_customers','INSERT,UPDATE,DELETE'),'compute',has_table_privilege('zuse_cloud_staging','zuse_staging.zuse_brokeboy_compute_charges','INSERT,UPDATE,DELETE'),'requests',has_table_privilege('zuse_cloud_staging','zuse_staging.zuse_brokeboy_model_requests','INSERT,UPDATE,DELETE'),'ledger',has_table_privilege('zuse_cloud_staging','zuse_staging.zuse_cloud_schema_migrations','UPDATE'));")[-1]
        self.assertEqual(grants,{"billing":True,"compute":True,"requests":True,"ledger":False})
        database.execute(self.url,"UPDATE zuse_staging.zuse_cloud_schema_migrations SET sha256=repeat('0',64) WHERE name='0032_brokeboy.sql';")
        with self.assertRaisesRegex(ValueError,"zuse_schema_manifest_mismatch"):
            database.execute(self.url,database.render_brokeboy("staging"))

    def test_failed_migration_rolls_back_entire_schema(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in database.FILES:
                shutil.copyfile(database.MIGRATIONS / name, root / name)
            with (root / database.FILES[-1]).open("a") as target:
                target.write("\nSELECT 1/0;\n")
            with self.assertRaisesRegex(ValueError, "zuse_schema_database_operation_failed"):
                database.execute(self.url, database.render("staging", root))
        result = database.execute(self.url, "SELECT json_build_object('present',EXISTS(SELECT 1 FROM pg_namespace WHERE nspname='zuse_staging'));")[-1]
        self.assertFalse(result["present"])

    def test_existing_unmanaged_schema_is_not_adopted(self):
        database.execute(self.url, "CREATE SCHEMA zuse_staging; CREATE TABLE zuse_staging.existing(body text); INSERT INTO zuse_staging.existing VALUES('preserve');")
        with self.assertRaisesRegex(ValueError, "zuse_schema_existing_tables_require_import_review"):
            database.execute(self.url, database.render("staging"))
        result = database.execute(self.url, "SELECT json_build_object('body',body) FROM zuse_staging.existing;")[-1]
        self.assertEqual(result["body"], "preserve")

    def test_remote_tls_required_and_inherited_pg_settings_are_removed(self):
        with self.assertRaisesRegex(ValueError, "zuse_schema_verified_tls_required"):
            database.connection_environment("postgres://user:secret@remote.example/db?sslmode=require")
        env = database.connection_environment("postgres://user:s%40fe@remote.example/db?sslmode=verify-full", {
            "PGSERVICE": "wrong", "PGOPTIONS": "-c search_path=public", "DATABASE_URL": "unrelated", "PATH": "/bin"})
        self.assertEqual(env["PGPASSWORD"], "s@fe")
        self.assertNotIn("PGSERVICE", env)
        self.assertNotIn("PGOPTIONS", env)
        self.assertNotIn("DATABASE_URL", env)


if __name__ == "__main__":
    unittest.main()
