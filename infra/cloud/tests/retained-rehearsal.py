#!/usr/bin/env python3
"""Full import SQL against two owned local clusters and fabricated legacy rows."""
import contextlib
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('retained',Path(__file__).resolve().parents[1]/'retained-rehearsal.py')
r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)

class RetainedRehearsal(unittest.TestCase):
    def test_actual_dump_restore_mapping_ciphertext_financial_rows_and_quarantine(self):
        m=r.migration
        with tempfile.TemporaryDirectory(prefix='zuse-fabricated-export-') as tmp,m.owned_cluster() as (url,source_root):
            directory=Path(tmp);directory.chmod(0o700)
            baseline=(m.database.MIGRATIONS/'0000_application.sql').read_text()
            m.execute(url,baseline)
            data,_=m.fixtures(source_root/'objects')
            for table in m.TABLES:
                for value in data[table] if isinstance(data[table],list) else [data[table]]:
                    m.seed_row(url,'public',table,value)
            m.seed_row(url,'public','api_cloud_billing_outbox',dict(outbox_id='fixture-pending-payment',period_id='period_fixture_stable',account_id='fixture-owner',provider='stripe',amount_cents=9007199254740993,idempotency_key='never-charge-again'))
            m.execute(url,"SELECT setval('api_agent_activity_id_seq',175,false);")
            dump=directory/'legacy-public.dump'
            with dump.open('wb') as output:
                subprocess.run(['pg_dump','--no-password','--format=custom','--no-owner','--no-acl','--table=public.api_*'],env=m.database.connection_environment(url),stdout=output,stderr=subprocess.PIPE,check=True)
            dump.chmod(0o600)
            manifest=dict(mode='private-consistent-export',sourceSchema='public',sourceTableCount=66,sourceReference=m.REFERENCE,sourceFingerprint='a'*64,sourceWritesAllowed=False,writerCutoverAllowed=False,file=dump.name,bytes=dump.stat().st_size,sha256=hashlib.sha256(dump.read_bytes()).hexdigest())
            p=directory/'manifest.json';p.write_text(json.dumps(manifest));p.chmod(0o600)
            @contextlib.contextmanager
            def cluster(image,operation):
                self.assertEqual(image,'postgres@sha256:'+'b'*64)
                with m.owned_cluster() as (target_url,_):yield m.database.connection_environment(target_url)
            with patch.object(r,'cluster',cluster):result=r.rehearse(directory,'a'*64,'postgres@sha256:'+'b'*64)
            self.assertEqual(result['retainedTableCount'],42)
            self.assertEqual(result['quarantineTableCount'],24)
            self.assertEqual(result['quarantinedRowCount'],1)
            self.assertTrue(result['stableIdsCiphertextAndAllSourceColumnsPreserved'])
            self.assertTrue(result['sequenceLastValueAndCalledPreserved'])
            self.assertEqual(result['ownedSequencesPreserved'],1)
            self.assertFalse(result['writerCutoverPerformed'])
            self.assertFalse(result['nativeExecutionAdmitted'])
            self.assertFalse(result['completeEndToEndMigrationCertified'])
            self.assertFalse(result['objectPayloadsCopied'])
            self.assertEqual((directory/'rehearsal-report.json').stat().st_mode&0o777,0o600)
    def test_exact_toc_filter_preserves_every_constraint_and_rejects_unknown_or_partial_triggers(self):
        triggers=[f"{i+10}; 2620 {i+100} TRIGGER public {table} {name} fixture_owner\n" for i,(table,name) in enumerate(sorted(r.LEGACY_PROCEDURAL_TRIGGERS))]
        constraints=["1; 1259 8 TABLE public api_cloud_projects fixture_owner\n", "2; 2606 9 FK CONSTRAINT public api_cloud_workspaces project_fk fixture_owner\n", "3; 2606 10 CONSTRAINT public api_cloud_projects project_pk fixture_owner\n", "4; 1259 11 INDEX public lookup_idx fixture_owner\n", "5; 0 0 SEQUENCE SET public api_agent_activity_id_seq fixture_owner\n"]
        selected,removed=r.select_restore_toc(''.join(constraints+triggers))
        self.assertEqual(len(removed),4)
        for line in constraints:self.assertIn(line,selected)
        self.assertEqual(sum(line.startswith('; quarantined ') for line in selected.splitlines()),4)
        with self.assertRaisesRegex(ValueError,'unknown_procedural_trigger'):
            r.select_restore_toc('9; 2620 9 TRIGGER public api_cloud_projects unreviewed_constraint_trigger owner\n')
        with self.assertRaisesRegex(ValueError,'partial_procedural_trigger_inventory'):r.select_restore_toc(triggers[0])
        self.assertEqual(r.select_restore_toc(''.join(constraints))[1],[])

    def test_multi_row_crypto_projection_is_one_json_document(self):
        m=r.migration
        with m.owned_cluster() as (url,_):
            m.execute(url,"CREATE TABLE public.api_cloud_github_users(account_id text,sealed_credentials text);INSERT INTO public.api_cloud_github_users VALUES('fixture-a','opaque-a'),('fixture-b','opaque-b'),('fixture-c','opaque-c');")
            with patch.object(r,'encryption_rows_proof',side_effect=lambda rows,path:{'checked':len(rows),'accounts':[x['accountId'] for x in rows]}):
                result=r.encryption_proof(m.database.connection_environment(url),None)
            self.assertEqual(result,{'checked':3,'accounts':['fixture-a','fixture-b','fixture-c']})

    @unittest.skipUnless(importlib.util.find_spec('cryptography'), 'Host cryptography checked separately on Hetzner')
    def test_optional_crypto_tool_failure_does_not_erase_structural_fixture(self):
        # The proof subprocess itself returns counts instead of raising for a
        # missing provider key/tool; the full real-PG fixture above covers evidence.
        import base64,os
        sp=importlib.util.spec_from_file_location('crypto',Path(r.__file__).with_name('legacy-encryption-proof.py'))
        crypto=importlib.util.module_from_spec(sp);sp.loader.exec_module(crypto)
        from cryptography.hazmat.primitives.ciphers.aead import AESGCM
        key=os.urandom(32);iv=os.urandom(12);account='fixture-owner'
        sealed=AESGCM(key).encrypt(iv,b'fictional-private-value',b'zuse-cloud-api-v1\ngithub-user\nfixture-owner')
        enc=lambda v:base64.urlsafe_b64encode(v).decode().rstrip('=')
        row={'accountId':account,'sealed':json.dumps({'version':1,'iv':enc(iv),'ciphertext':enc(sealed)})}
        result=crypto.proof({'key':enc(key),'rows':[row,dict(row,accountId='foreign-owner')]})
        self.assertEqual(result,{'metadataOnly':True,'checked':2,'opened':1,'failed':1,'keyMatchesAllSamples':False})
        self.assertNotIn('fictional-private-value',json.dumps(result))

    def test_container_requires_content_addressed_image_before_any_process(self):
        with patch.object(r.subprocess,'run') as process:
            with self.assertRaisesRegex(ValueError,'pinned_postgres_image'):
                with r.cluster('postgres:18',Path('/unused')):pass
            process.assert_not_called()

if __name__=='__main__':unittest.main()
