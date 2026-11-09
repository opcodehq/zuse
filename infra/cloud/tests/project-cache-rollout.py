#!/usr/bin/env python3
"""Actual isolated optional-schema/operator fences; fixtures are never live evidence."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parents[1]
def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result
BASE = load("cache_rollout_database_tests", Path(__file__).with_name("database.py"))
FIXTURE = load("cache_rollout_synthetic_release", Path(__file__).with_name("project-cache-certification.py"))
TOOL = load("cache_rollout_tool", HERE / "project-cache-rollout.py")
DB = TOOL.DB

class RolloutTests(BASE.DatabaseBootstrapTest):
    def test_explicit_optional_migration_retains_default_manifest_roles_and_original_intent(self):
        self.assertNotIn(TOOL.MIGRATION, DB.FILES)
        for environment in ("staging", "production"):
            DB.execute(self.url, DB.render(environment))
        provision = load("cache_rollout_provision", HERE / "single-host/provision.py")
        name = self.url.rsplit("/", 1)[1].split("?")[0]
        DB.execute(self.url, provision.role_sql("staging", name, "synthetic-only", True))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            release = FIXTURE.fixture(root)
            bound = dict(version=1, caseId="synthetic-cache-case", ownerId="synthetic-owner",
                projectId="synthetic-project", protectedWorkspaceId="synthetic-original",
                protectedMachineId="synthetic-original-machine", protectedGeneration=1,
                protectedRetainedSha256="1" * 64, apiSha256="2" * 64,
                maxActiveMachines=2, maxOwnedMachines=1, maxDurationSeconds=900, cleanupReserveSeconds=180)
            FIXTURE.write(root / "bounds.json", bound)
            with patch.object(TOOL.subprocess, "run", side_effect=AssertionError("default plan must never contact DB/provider")):
                value = TOOL.plan(root / "release.json", root / "bounds.json")
            self.assertEqual(value["workerEnvironmentOverlay"]["ZUSE_CLOUD_PROJECT_PREPARATION_BACKGROUND"], "0")
            self.assertFalse(value["certifiesBaseOrCache"])
            FIXTURE.write(root / "plan.json", value)
            approved = TOOL.sha((root / "plan.json").read_bytes())
            (root / "dsn").write_text(self.url)
            (root / "dsn").chmod(0o600)
            def query(sql):
                return DB.execute(self.url, "SET search_path=zuse_staging,pg_catalog;" + sql)
            query("INSERT INTO cloud_managed_bases(provider,release_id,artifact_id,architecture,capabilities,state,created_at,runtime_manifest) VALUES('boxd','synthetic-base','synthetic-base-snapshot','x86_64','{\"id\":\"boxd\"}','preparing',0," + DB.literal(TOOL.SECURE.canonical(release).decode()) + "::jsonb);")
            query("INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at) VALUES('retained-original-project','synthetic-owner','github.com/example/original','https://github.com/example/original.git','retained','main','private','github-app','retained','connected','retained-project-key',0,0);")
            query("INSERT INTO api_cloud_workspaces(workspace_id,account_id,project_id,managed_release_id,provider,provider_sandbox_id,chat_id,initial_session_id,branch,base_ref,state,desired_state,status_code,idempotency_key,request_config,next_action_at,created_at,updated_at,last_activity_at) VALUES('synthetic-original','synthetic-owner','retained-original-project','synthetic-base','boxd','synthetic-original-machine','chat','session','retained','main','ready','ready','ready','original-key','{\"runtimeGeneration\":1}',0,0,0,0);")
            original = query("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='synthetic-original'")[-1]
            def apply(receipt):
                return TOOL.apply(root / "plan.json", approved, root / "release.json", root / "bounds.json", root / "dsn", root / receipt)
            with self.assertRaises(TOOL.SECURE.Rejected):
                apply("candidate-rejected.json")
            self.assertEqual(TOOL.inspect(root / "plan.json", approved, root / "release.json", root / "bounds.json", root / "dsn", root / "candidate-rejected.json")["status"], "original-schema-not-observed")
            self.assertEqual(query("SELECT json_build_object('n',count(*)) FROM zuse_cloud_schema_migrations")[-1]["n"], len(DB.FILES))
            query("UPDATE cloud_managed_bases SET state='verified'")
            apply("success.json")
            self.assertTrue((root / "success.json").is_file())
            with self.assertRaises(TOOL.SECURE.Rejected):
                apply("success.json")
            self.assertEqual(TOOL.inspect(root / "plan.json", approved, root / "release.json", root / "bounds.json", root / "dsn", root / "success.json")["status"], "original-schema-commit-observed")
            ledger = query("SELECT json_build_object('rows',json_agg(json_build_object('name',name,'sha256',sha256))) FROM zuse_cloud_schema_migrations")[-1]["rows"]
            self.assertTrue(DB.manifest_matches("staging", ledger))
            self.assertFalse(DB.manifest_matches("staging", ledger + [{"name": "unknown", "sha256": "0" * 64}]))
            wrong = [dict(row, sha256="0" * 64) if row["name"] == TOOL.MIGRATION else row for row in ledger]
            self.assertFalse(DB.manifest_matches("staging", wrong))
            DB.execute(self.url, DB.render("staging"))
            import os
            checked=subprocess.run(["python3","-B",str(HERE/"database.py"),"--environment","staging","check"],env=dict(os.environ,ZUSE_CLOUD_DATABASE_URL=self.url),capture_output=True,text=True)
            self.assertEqual(checked.returncode,0,checked.stderr)
            self.assertEqual(len(json.loads(checked.stdout)["migrations"]),len(DB.FILES)+1)
            production = DB.execute(self.url, "SELECT json_build_object('n',count(*),'cache',to_regclass('zuse_production.cloud_project_cache_capabilities') IS NOT NULL) FROM zuse_production.zuse_cloud_schema_migrations")[-1]
            self.assertEqual(production, {"n": len(DB.FILES), "cache": False})
            app = self.url.replace("zuse_test@", "zuse_cloud_staging:synthetic-only@")
            privileges = DB.execute(app, "SELECT json_build_object('capWrite',has_table_privilege(current_user,'cloud_project_cache_capabilities','UPDATE'),'work',has_table_privilege(current_user,'zuse_rust_project_cache_operations','INSERT'),'lock',has_function_privilege(current_user,'zuse_rust_project_cache_capability_lock(text,text)','EXECUTE'))")[-1]
            self.assertEqual(privileges, {"capWrite": False, "work": True, "lock": True})
            self.assertEqual(query("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='synthetic-original'")[-1], original)
            # The separately bounded pilot admits an exact PREPARING candidate;
            # it neither certifies nor promotes it, even when schema exists.
            candidate = dict(release, runtimeSha256="a" * 64)
            FIXTURE.write(root / "candidate.json", candidate)
            now = 1_800_000_000_000
            pin = dict(ownerId=bound["ownerId"], provider="boxd", releaseId=candidate["releaseId"],
                artifactId=candidate["artifactId"], candidateSha256=TOOL.sha(TOOL.SECURE.canonical(candidate)),
                manifestSha256=TOOL.sha(TOOL.SECURE.canonical(candidate["manifest"])), expiresAt=now + 1_800_000)
            pilot = dict(version=1, pilotId="synthetic-pilot", ownerId=bound["ownerId"], actorId=bound["ownerId"],
                projectId=bound["projectId"], repositoryIdentity="github.com/example/public",
                candidateSha256=pin["candidateSha256"], baseReleaseId=candidate["releaseId"],
                baseArtifactId=candidate["artifactId"], baseArtifactVersion="1", runtimeSha256="a" * 64,
                architecture="x86_64", expiresAt=now + 900_000,
                protectedWorkspaceId=bound["protectedWorkspaceId"], protectedMachineId=bound["protectedMachineId"],
                maxActiveMachines=2, maxOwnedMachines=1,
                inputs=[dict(recipeDigest="d" * 64, recipeSource="project", sourceCommit="e" * 40, dependencyDigest="f" * 64)],
                mechanismCapability=TOOL.SECURE.load(root / "capability.json"))
            FIXTURE.write(root / "pilot.json", pilot)
            FIXTURE.write(root / "pin.json", pin)
            with patch.object(TOOL.time, "time", return_value=now / 1000):
                pilot_plan = TOOL.plan(root / "candidate.json", root / "bounds.json", root / "pilot.json", root / "pin.json")
                self.assertEqual(pilot_plan["baseAdmission"], "staging-pilot")
                self.assertFalse(pilot_plan["certifiesBaseOrCache"])
                FIXTURE.write(root / "pilot-plan.json", pilot_plan)
                pilot_approved = TOOL.sha((root / "pilot-plan.json").read_bytes())
                query("UPDATE cloud_managed_bases SET state='preparing',runtime_manifest=NULL,candidate_manifest=" + DB.literal(TOOL.SECURE.canonical(candidate).decode()) + "::jsonb")
                TOOL.apply(root / "pilot-plan.json", pilot_approved, root / "candidate.json", root / "bounds.json", root / "dsn", root / "pilot-success.json", root / "pilot.json", root / "pin.json")
                self.assertEqual(query("SELECT json_build_object('state',state,'certified',runtime_manifest IS NOT NULL) FROM cloud_managed_bases")[-1], dict(state="preparing", certified=False))
            with patch.object(TOOL.time, "time", return_value=(now + 1_000_000) / 1000):
                with self.assertRaises(TOOL.SECURE.Rejected):
                    TOOL.plan(root / "candidate.json", root / "bounds.json", root / "pilot.json", root / "pin.json")
                self.assertEqual(TOOL.inspect(root / "pilot-plan.json", pilot_approved, root / "candidate.json", root / "bounds.json", root / "dsn", root / "pilot-success.json", root / "pilot.json", root / "pin.json")["status"], "original-schema-commit-observed")
            query("UPDATE cloud_managed_bases SET state='verified',candidate_manifest=NULL,runtime_manifest=" + DB.literal(TOOL.SECURE.canonical(release).decode()) + "::jsonb")
            # A lost COMMIT is inspected only; the original receipt path cannot
            # resubmit even though optional migration is itself idempotent.
            real = TOOL.subprocess.run
            def lost(*args, **kwargs):
                result = real(*args, **kwargs)
                self.assertEqual(result.returncode, 0)
                raise subprocess.TimeoutExpired(args[0], 90)
            with patch.object(TOOL.subprocess, "run", side_effect=lost):
                with self.assertRaises(subprocess.TimeoutExpired):
                    apply("lost.json")
            with self.assertRaises(TOOL.SECURE.Rejected):
                apply("lost.json")
            self.assertEqual(TOOL.inspect(root / "plan.json", approved, root / "release.json", root / "bounds.json", root / "dsn", root / "lost.json")["status"], "original-schema-commit-observed")

if __name__ == "__main__":
    unittest.main(defaultTest="RolloutTests.test_explicit_optional_migration_retains_default_manifest_roles_and_original_intent")
