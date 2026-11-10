#!/usr/bin/env python3
"""Snapshot tooling tests use only fabricated metadata and fake dump bytes."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

spec=importlib.util.spec_from_file_location('legacy_export',Path(__file__).resolve().parents[1]/'legacy-export.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
URL='postgresql://fixture_reader:never-print-this@fixture.invalid/legacy?sslmode=verify-full'

class LegacyExport(unittest.TestCase):
    def metadata(self):
        i=m.rehearsal.table_inventory()
        return dict(serverVersionNumber='180006',tables=i['retained']+i['quarantined'],tableAndToastBytes=4096,indexBytes=2048,approximateRows=7)
    def describe(self):
        with patch.object(m.rehearsal,'execute',return_value=[self.metadata()]) as query,patch.object(m.subprocess,'run',return_value=SimpleNamespace(stdout='pg_dump (PostgreSQL) 18.6',returncode=0)):
            p=m.describe(URL)
        self.assertIn('BEGIN READ ONLY',query.call_args.args[1]);self.assertNotIn('never-print-this',json.dumps(p))
        return p
    def test_plan_is_complete_readonly_and_has_no_connection_secret(self):
        p=self.describe();self.assertEqual(p['sourceTableCount'],66);self.assertFalse(p['customerPayloadExported']);self.assertFalse(p['writerCutoverAllowed'])
    def test_export_requires_matching_source_review_before_creating_any_file(self):
        with tempfile.TemporaryDirectory() as d,patch.object(m,'describe',return_value=self.describe()):
            root=Path(d)/'snapshot'
            with self.assertRaisesRegex(ValueError,'exact_source_approval'):m.export(URL,root,'different')
            self.assertFalse(root.exists())
    def test_private_dump_uses_readonly_snapshot_and_never_writes_customer_rows_to_stdout(self):
        plan=self.describe();calls=[]
        def run(argv,**kwargs):
            calls.append((argv,kwargs));kwargs['stdout'].write(b'fixture private dump ciphertext')
            return SimpleNamespace(returncode=0)
        with tempfile.TemporaryDirectory() as d,patch.object(m,'describe',return_value=plan),patch.object(m.subprocess,'run',side_effect=run):
            root=Path(d)/'snapshot';manifest=m.export(URL,root,plan['sourceFingerprint'])
            self.assertEqual(root.stat().st_mode&0o777,0o700)
            self.assertEqual((root/'legacy-public.dump').stat().st_mode&0o777,0o600)
            self.assertEqual((root/'manifest.json').stat().st_mode&0o777,0o600)
            command,kwargs=calls[0];self.assertIn('--serializable-deferrable',command)
            self.assertIn('default_transaction_read_only=on',kwargs['env']['PGOPTIONS'])
            self.assertNotIn('never-print-this',' '.join(command))
            self.assertEqual(manifest['sourceWriter'],'legacy');self.assertEqual(manifest['targetWriter'],'none')
            with self.assertRaisesRegex(ValueError,'new_private_export_directory'):m.export(URL,root,plan['sourceFingerprint'])
    def test_client_version_and_unknown_source_tables_fail_closed(self):
        p=dict(self.describe(),pgDumpMajor=15)
        with tempfile.TemporaryDirectory() as d,patch.object(m,'describe',return_value=p):
            with self.assertRaisesRegex(ValueError,'matching_pg_dump'):m.export(URL,Path(d)/'snapshot',p['sourceFingerprint'])
        bad=self.metadata();bad['tables'].append('api_unknown')
        with patch.object(m.rehearsal,'execute',return_value=[bad]):
            with self.assertRaisesRegex(ValueError,'unclassified_source_table'):m.describe(URL)

if __name__=='__main__':unittest.main()
