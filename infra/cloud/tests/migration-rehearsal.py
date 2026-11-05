#!/usr/bin/env python3
"""Synthetic migration and read-only audit tests; no deployment credentials."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("migration_rehearsal", Path(__file__).resolve().parents[1] / "migration-rehearsal.py")
rehearsal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rehearsal)


class MigrationRehearsalTests(unittest.TestCase):
    def test_real_pg_roles_ciphertext_financial_ids_objects_and_rollback(self):
        with patch.dict(os.environ,{"PGHOST":"must-not-connect.invalid","PGDATA":"/production/not-allowed","ZUSE_CLOUD_DATABASE_URL":"postgres://must-not-connect.invalid/production"}):
            report = rehearsal.rehearse()
        self.assertTrue(report["fixturePassed"])
        self.assertEqual(report["sourceTableInventory"]["sourceTableCount"],66)
        self.assertEqual(report["quarantinedTableCount"],24)
        self.assertEqual(report["quarantinedFixtureRowCount"],5)
        self.assertTrue(report["quarantinedRowsPreservedAndInactive"])
        self.assertIn("api_cloud_github_installations",report["sourceTableDigests"])
        self.assertIn("api_provider_lifecycle_pairs",report["sourceTableDigests"])
        self.assertIn("api_stripe_subscription_migrations",report["quarantineTableDigests"])
        self.assertEqual(report["importedTableCount"], len(rehearsal.TABLES))
        self.assertTrue(report["stableIdentitiesAndCiphertextPreserved"])
        self.assertTrue(report["isolatedRoleWriterFencingVerified"])
        self.assertTrue(report["cleanRollbackVerified"])
        self.assertTrue(report["rollbackAfterTargetWritesBlocked"])
        self.assertTrue(report["sequenceHighWaterMarkPreserved"])
        self.assertTrue(report["rollbackAfterSequenceConsumptionBlocked"])
        self.assertTrue(report["largeIntegerFinancialAmountPreserved"])
        self.assertEqual(report["copiedFixtureObjectCount"], 2)
        self.assertEqual(report["nativeControlOwnersImported"], 0)
        self.assertEqual(report["providerJobsImported"], 0)
        self.assertFalse(report["nativeGuestExecutionAdmitted"])
        self.assertFalse(report["realRetainedDatasetVerified"])
        self.assertTrue(report["targetMigrationManifest"])

    def test_complete_inventory_and_system_ca_preserve_verified_tls(self):
        inventory = rehearsal.table_inventory()
        self.assertEqual(len(inventory["retained"]),42)
        self.assertEqual(len(inventory["quarantined"]),24)
        self.assertFalse(set(inventory["retained"]) & set(inventory["quarantined"]))
        with tempfile.TemporaryDirectory() as directory:
            bundle=Path(directory)/"system-ca.pem";bundle.write_text("fixture public CA")
            from types import SimpleNamespace
            with patch.object(rehearsal.database.ssl,"get_default_verify_paths",return_value=SimpleNamespace(cafile=str(bundle))):
                env=rehearsal.database.connection_environment("postgresql://reader:private@database.invalid/legacy?sslmode=verify-full&sslrootcert=system",{})
            self.assertEqual(env["PGSSLROOTCERT"],str(bundle))
            self.assertEqual(env["PGSSLMODE"],"verify-full")
            with patch.object(rehearsal.database.ssl,"get_default_verify_paths",return_value=SimpleNamespace(cafile=None)):
                with self.assertRaisesRegex(ValueError,"system_ca_unavailable"):
                    rehearsal.database.connection_environment("postgresql://reader:private@database.invalid/legacy?sslrootcert=system",{})

    def test_cli_cannot_select_production_or_accept_a_write_target(self):
        script = str(Path(rehearsal.__file__))
        for args in (("--environment", "production", "rehearse"),
                     ("--environment", "staging", "rehearse", "--target-url", "postgres://must-not-connect")):
            result = subprocess.run(["python3", script, *args], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
        result = subprocess.run(["python3", script, "--environment", "staging", "plan"], capture_output=True, text=True,
                                env={**os.environ, "ZUSE_CLOUD_DATABASE_URL": "postgres://must-not-connect"})
        self.assertEqual(result.returncode, 0)
        self.assertFalse(json.loads(result.stdout)["productionWritesAllowed"])

    def test_nonempty_target_never_overwrites_existing_identity(self):
        with rehearsal.owned_cluster() as (url, root):
            rehearsal.execute(url,"CREATE SCHEMA legacy;CREATE SCHEMA target;CREATE TABLE legacy.api_workspace_settings(owner_id text PRIMARY KEY,revision integer,values jsonb);CREATE TABLE target.api_workspace_settings(LIKE legacy.api_workspace_settings INCLUDING ALL);INSERT INTO legacy.api_workspace_settings VALUES('incoming',1,'{}');INSERT INTO target.api_workspace_settings VALUES('retained-target',8,'{}');")
            with self.assertRaisesRegex(ValueError,"target_contains_rows"):
                rehearsal.copy_rows(url,"legacy","target",{"api_workspace_settings":["owner_id","revision","values"]})
            rows=rehearsal.rows(url,"target","api_workspace_settings")
            self.assertEqual(rows,[{"owner_id":"retained-target","revision":8,"values":{}}])

    def test_readonly_audit_exports_only_counts_and_requires_private_file(self):
        with rehearsal.owned_cluster() as (url, root):
            rehearsal.execute(url, "CREATE SCHEMA legacy_audit; CREATE TABLE legacy_audit.api_cloud_workspaces(private_plaintext text); INSERT INTO legacy_audit.api_cloud_workspaces VALUES('fixture-content-must-never-be-returned');")
            path = root / "source-url"
            path.write_text(url)
            path.chmod(0o600)
            result = rehearsal.audit(path, "legacy_audit")
            self.assertEqual(result["counts"], {"api_cloud_workspaces": 1})
            self.assertFalse(result["customerRowsExported"])
            self.assertFalse(result["migrationExecuted"])
            self.assertNotIn("fixture-content-must-never-be-returned", json.dumps(result))
            path.write_text("postgresql://reader:synthetic@must-not-connect.invalid/database?sslmode=disable")
            with self.assertRaisesRegex(ValueError,"verified_tls_required"):
                rehearsal.audit(path,"public")
            path.write_text(url)
            path.chmod(0o644)
            with self.assertRaisesRegex(ValueError, "private_source_file_required"):
                rehearsal.audit(path, "legacy_audit")
            path.chmod(0o600)
            link = root / "source-url-link"
            link.symlink_to(path)
            with self.assertRaisesRegex(ValueError, "private_source_file_required"):
                rehearsal.audit(link, "legacy_audit")
            with self.assertRaisesRegex(ValueError, "invalid_identifier"):
                rehearsal.audit(path, "public;DROP SCHEMA public")


if __name__ == "__main__":
    unittest.main()
