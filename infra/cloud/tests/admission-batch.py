#!/usr/bin/env python3
"""Isolated SQL proof for the UNATTACHED admission write prototype.

This tests atomic SQL only. It does not certify read-side authorization, runtime
readiness, production latency, or integration into the creation endpoint.
"""
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import unittest

spec = importlib.util.spec_from_file_location('rehearsal', Path(__file__).resolve().parents[1] / 'migration-rehearsal.py')
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
SOURCE = r.ROOT / 'crates/cloud/src/runtime/admission_batch.rs'
TABLES = ('api_cloud_workspaces', 'api_cloud_workspace_launch_intents',
          'zuse_rust_task_admissions', 'zuse_rust_control_owners',
          'zuse_rust_allocations', 'zuse_rust_jobs')


class AdmissionBatchPrototypeTests(unittest.TestCase):
    def test_atomic_ciphertext_launch_owner_allocation_and_job_with_rollback(self):
        source = SOURCE.read_text()
        write = re.search(r'const WRITE: &str = r#"(.*?)"#;', source, re.S).group(1)
        # The prototype is deliberately not callable from the runtime HTTP path.
        # Read/write endpoint integration is validated by Rust admission fixtures.
        self.assertIn("'workspace-allocate'", write)
        with r.owned_cluster() as (url, root):
            r.execute(url, 'CREATE SCHEMA prototype;SET search_path=prototype,pg_catalog;' + '\n'.join(m['sql'] for m in r.database.migrations()))
            data, _ = r.fixtures(root / 'objects')
            for table in ('api_cloud_projects', 'api_cloud_project_builds'):
                r.seed_row(url, 'prototype', table, data[table])
            plan = dict(workspaceId='fresh', ownerId='fixture-owner', projectId='fixture-project',
                        buildId='fixture-build', managedReleaseId=None, provider='box',
                        chatId='chat-fresh', sessionId='session-fresh', branch='task/fresh', baseRef='main',
                        wrappedTranscriptKey='opaque-wrapped-key', idempotencyKey='request-fresh',
                        requestConfig={'runtimeGeneration': 1, 'gatewayEpoch': 1},
                        commandId='command-fresh', turnId='turn-fresh',
                        launchCiphertext='opaque-launch-envelope', admissionCiphertext='opaque-history-envelope',
                        launchExpiresAt=90000, operationId='allocation-fresh', snapshotId='snapshot-fresh',
                        machineName='machine-fresh', storageIncarnationId='storage-fresh',
                        sealedBootToken='opaque-boot-envelope', bootTokenHash='boot-hash', bootExpiresAt=80000,
                        jobId='job-fresh', now=1000)
            prefix = 'SET search_path=prototype,pg_catalog;PREPARE batch(jsonb) AS ' + write + ';'
            invoke = lambda value: 'EXECUTE batch(' + r.database.literal(json.dumps(value)) + '::jsonb);'
            first = r.execute(url, prefix + 'BEGIN;' + invoke(plan) + 'COMMIT;')[-1]
            self.assertEqual(first['workspace_id'], 'fresh')
            self.assertEqual(first['revision'], 1)
            facts = dict(owner='fixture-owner', actor='fixture-owner', now=1000,
                         idempotencyKey='request-fresh', deleteDigest='missing', projectId='fixture-project',
                         provider='boxd', connectionId='missing', agent='codex', stagingAllowance=True,
                         candidateRelease=None, environmentRevision=0, launchDigest='missing',
                         workspaceId='workspace_0123456789abcdef', branch=None)
            read = lambda value: 'SELECT zuse_rust_admission_read_batch('+r.database.literal(json.dumps(value))+'::jsonb);'
            prior = r.execute(url, 'SET search_path=prototype,pg_catalog;BEGIN;'+read(facts)+'ROLLBACK;')[-1]
            self.assertEqual(prior['prior']['workspace_id'], 'fresh')
            self.assertNotIn('project', prior)  # Idempotent retry does not need a current connection/base.
            fresh = r.execute(url, 'SET search_path=prototype,pg_catalog;BEGIN;'+read({**facts, 'idempotencyKey':'next'})+'ROLLBACK;')[-1]
            self.assertEqual(fresh['project']['project_id'], 'fixture-project')
            self.assertEqual(fresh['branch'], 'cloud-0123456789ab')
            self.assertFalse(fresh['branchInUse'])
            self.assertIsNone(fresh['connectionOwner'])
            for table in TABLES:
                self.assertEqual(len(r.rows(url, 'prototype', table)), 1, table)
            job = r.rows(url, 'prototype', 'zuse_rust_jobs')[0]
            self.assertEqual((job['kind'], job['max_attempts'], job['payload']),
                             ('workspace-allocate', 120, {'operationId': 'allocation-fresh'}))
            history = r.rows(url, 'prototype', 'zuse_rust_task_admissions')[0]
            self.assertEqual(history['ciphertext'], 'opaque-history-envelope')
            # A final CTE unique-key failure rolls back earlier workspace/history inserts.
            failed = {**plan, 'workspaceId': 'conflict', 'idempotencyKey': 'request-conflict',
                      'branch': 'task/conflict', 'chatId': 'chat-conflict', 'sessionId': 'session-conflict', 'commandId': 'command-conflict',
                      'operationId': 'allocation-conflict', 'machineName': 'machine-conflict'}
            with self.assertRaisesRegex(ValueError, 'database_failure'):
                r.execute(url, prefix + 'BEGIN;' + invoke(failed) + 'COMMIT;')
            for table in TABLES:
                self.assertEqual(len(r.rows(url, 'prototype', table)), 1, table)
            # Successful statement plus explicit transaction rollback also leaves no partial work.
            rolled = {**failed, 'jobId': 'job-rollback'}
            r.execute(url, prefix + 'BEGIN;' + invoke(rolled) + 'ROLLBACK;')
            for table in TABLES:
                self.assertEqual(len(r.rows(url, 'prototype', table)), 1, table)
            # Statement-start snapshots must not hide a create committed while
            # this read waits for the account advisory lock.
            raced = {**rolled, 'jobId':'job-race', 'idempotencyKey':'request-race'}
            sql = prefix + "BEGIN;SELECT pg_advisory_xact_lock(hashtextextended('cloud-create:fixture-owner',0));SELECT json_build_object('locked',true);SELECT pg_sleep(0.25);" + invoke(raced) + 'COMMIT;'
            child = subprocess.Popen(['psql','-X','-qAt','-v','ON_ERROR_STOP=1'],
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                     env=r.database.connection_environment(url), text=True)
            try:
                child.stdin.write(sql)
                child.stdin.close()
                while True:
                    line = child.stdout.readline()
                    self.assertTrue(line or child.poll() is None)
                    if '"locked"' in line:
                        break
                observed = r.execute(url, 'SET search_path=prototype,pg_catalog;BEGIN;' + read({**facts,'idempotencyKey':'request-race'}) + 'ROLLBACK;')[-1]
                self.assertEqual(observed['prior']['workspace_id'], 'conflict')
                self.assertEqual(child.wait(timeout=10), 0)
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=10)
                child.stdout.close()
                child.stderr.close()



if __name__ == '__main__':
    unittest.main()
