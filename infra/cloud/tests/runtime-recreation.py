#!/usr/bin/env python3
"""Real filesystem/flock tests; no provider resources or model inference."""
import copy
import fcntl
import importlib.util
import json
import os
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
def load(name,path):
    spec=importlib.util.spec_from_file_location(name,path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module
helper=load('supervisor_recreation_test',ROOT/'runtime-supervisor.py')
module=load('recreation_test',ROOT/'runtime-recreation.py')
class RecreationTest(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.data=Path(self.temp.name)/'data';self.data.mkdir(mode=0o700)
        self.repo=Path(self.temp.name)/'repo';self.repo.mkdir();(self.repo/'dirty').write_text('keep branch and work')
        self.request=dict(version=1,action='prepare-recreation',operationId='recreate-1',archiveOperationId='archive-1',workspaceId='task',ownerId='owner',generation=3,storageIncarnationId='disk',archiveSha256='a'*64)
        self.supervisor=SimpleNamespace(data=self.data,checkout=self.repo,record=None,recovery_binding=dict(workspaceId='task',generation=3,storageIncarnationId='disk'),recoveries={},repairs={},reap=lambda:None,alive=lambda:False)
        helper.atomic(self.data/'rust-recovery-restored.json',dict(binding=dict(operationId='archive-1',workspaceId='task',rustStorage=dict(accountId='owner',storageIncarnationId='disk')),archiveSha256='a'*64))
        self.environment=dict(workspace='task',incarnation='disk',checkout=str(self.repo),recipe=dict(scripts=dict(initialize='install recorded tools')))
        helper.atomic(self.data/'environment-ready.json',self.environment)
    def prepare(self,request=None,atomic=helper.atomic):return module.prepare(self.supervisor,request or self.request,require=helper.require,atomic=atomic,load_private=helper.load_private)
    def test_retains_recipe_and_idempotent_delivery_never_removes_new_ready_receipt(self):
        receipt=self.prepare();self.assertEqual(receipt['status'],'prepared');self.assertFalse((self.data/'environment-ready.json').exists())
        self.assertEqual(helper.load_private(self.data/'.runtime-recreation/environment.json'),self.environment)
        self.assertEqual((self.repo/'dirty').read_text(),'keep branch and work')
        helper.atomic(self.data/'environment-ready.json',dict(self.environment,reapplied=True))
        self.assertEqual(self.prepare(),receipt)
        self.assertTrue(helper.load_private(self.data/'environment-ready.json')['reapplied'])
    def test_scope_generation_archive_and_busy_process_fail_before_receipt_change(self):
        for field,value in [('workspaceId','other'),('generation',4),('storageIncarnationId','other'),('ownerId','other'),('archiveOperationId','other'),('archiveSha256','b'*64)]:
            request=dict(self.request,**{field:value})
            with self.assertRaises(helper.Rejected):self.prepare(request)
            self.assertTrue((self.data/'environment-ready.json').exists())
        self.supervisor.alive=lambda:True
        with self.assertRaises(helper.Rejected):self.prepare()
        self.supervisor.alive=lambda:False
        self.supervisor.recoveries={'helper':SimpleNamespace(poll=lambda:None)}
        with self.assertRaises(helper.Rejected):self.prepare()
    def test_exclusive_native_data_lock_is_respected(self):
        lock=os.open(self.data/'.workspace-runtime.lock',os.O_RDWR|os.O_CREAT,0o600);self.addCleanup(os.close,lock)
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        with self.assertRaises(helper.Rejected):self.prepare()
        self.assertTrue((self.data/'environment-ready.json').exists())
    def test_lost_final_ack_reconciles_original_rename_without_second_recipe_invalidation(self):
        def lose(path,value):
            if path.name==module.MARKER and value.get('phase')=='ready':raise OSError('synthetic lost completion write')
            helper.atomic(path,value)
        with self.assertRaises(OSError):self.prepare(atomic=lose)
        self.assertFalse((self.data/'environment-ready.json').exists())
        self.assertEqual(self.prepare()['status'],'prepared')
        self.assertEqual(helper.load_private(self.data/'.runtime-recreation/environment.json'),self.environment)
    def test_symlink_and_changed_operation_are_rejected(self):
        self.prepare()
        with self.assertRaises(helper.Rejected):self.prepare(dict(self.request,operationId='other'))
        (self.data/module.MARKER).unlink()
        path=self.data/'environment-ready.json';path.symlink_to(self.repo/'dirty')
        with self.assertRaises(OSError):self.prepare()
        self.assertEqual((self.repo/'dirty').read_text(),'keep branch and work')
if __name__=='__main__':unittest.main()
