#!/usr/bin/env python3
"""Offline tests: no owner session, keys, sockets, database or providers."""
import importlib.util
from pathlib import Path
import unittest
from types import SimpleNamespace

path = Path(__file__).resolve().parents[1]/'staging-v2-acceptance.py'
spec = importlib.util.spec_from_file_location('acceptance', path)
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)

CONFIG = dict(apiOrigin=m.API, owner='user_fixture', projectId='project_fixture',
              connectionId='connection_fixture', model='fixture-model',
              candidateReleaseId='base_fixture', candidateSha256='a'*64,
              provider='boxd', agent='codex', baseRef='main')

class DriverTest(unittest.TestCase):
    def driver(self):
        calls = []; store = {}
        d = m.Driver(SimpleNamespace(), dict(CONFIG), Path('/unused'), None)
        d.read = lambda key: store.get(key)
        d.write = lambda key, value: store.__setitem__(key, value)
        d.api = lambda *args: calls.append(args) or (200, {'durablyAdmitted': True})
        d.prepare()
        d.go = dict(approved=True, owner=CONFIG['owner'], provider='boxd',
                    candidateReleaseId=CONFIG['candidateReleaseId'],
                    candidateSha256=CONFIG['candidateSha256'], expiresAt=10**12,
                    runId=store['run']['runId'])
        store['create-result'] = {'workspace': {'workspaceId':'workspace_fixture'}, 'initialSessionId':'session_fixture'}
        return d, store, calls

    def test_offline_validation_rejects_prod_unpinned_candidate_and_unsafe_ref(self):
        self.assertTrue(m.validate_config(CONFIG)['credentialsRead'] is False)
        for key, value in [('apiOrigin','https://api-rust.zuse.sh'), ('candidateSha256','missing'), ('baseRef','../main')]:
            with self.assertRaises(m.Failure): m.validate_config(dict(CONFIG, **{key:value}))

    def test_go_pins_exact_run_owner_provider_artifact_and_expiry(self):
        d, store, calls = self.driver()
        for key in ('runId', 'owner', 'candidateSha256', 'candidateReleaseId', 'provider'):
            original = d.go[key]; d.go[key] = 'different'
            with self.assertRaises(m.Failure):d.effect('bad', '/fixture', {})
            self.assertFalse(calls); d.go[key] = original
        d.go['expiresAt'] = 0
        with self.assertRaises(m.Failure):d.effect('bad', '/fixture', {})
        self.assertFalse(calls)

    def test_intent_written_before_post_lost_reply_never_replayed(self):
        d, store, calls = self.driver()
        def lost(*args):
            self.assertIn('intent-followup', store)
            calls.append(args)
            raise OSError('not exposed')
        d.api = lost
        with self.assertRaisesRegex(m.Failure, 'outcome_uncertain'):d.step('followup')
        self.assertEqual(store['outcome-followup'], {'uncertain':True})
        with self.assertRaisesRegex(m.Failure,'inspect_original'):d.step('followup')
        self.assertEqual(len(calls),1)

    def test_native_run_uses_stable_mid_relative_cwd_no_local_assets(self):
        d, store, calls = self.driver()
        d.step('followup')
        request = store['intent-followup']['body']; action = request['action']
        self.assertEqual(request['commandId'],store['run']['commandIds']['followup'])
        self.assertEqual(action['messageId'],action['payload']['messageId'])
        self.assertEqual(action['payload']['request']['cwd'],'.')
        self.assertEqual(action['payload']['request']['harness'],'codex')
        self.assertEqual(action['payload']['request']['attachments'],[])
        self.assertEqual(action['payload']['kind'],'run')
        self.assertEqual(request['wake'],'preserve-sleep')
        self.assertEqual(len(calls),1)

    def test_followup_binds_the_selected_agent_policy(self):
        for agent,harness in [('claude','claude-code'),('codex','codex'),('grok','grok')]:
            d,store,calls=self.driver(); d.cfg['agent']=agent; d.step('followup')
            self.assertEqual(store['intent-followup']['body']['action']['payload']['request']['harness'],harness)

    def test_queue_revision_cas_stop_is_distinct_resume_and_remove(self):
        d, store, calls = self.driver()
        qid=store['run']['queueIds']['add']
        d.queue=lambda:{'paused':True,'items':[{'id':qid,'revision':7}]}
        d.step('edit'); d.step('remove'); d.step('stop'); d.step('resume-queue')
        self.assertEqual(store['intent-edit']['body']['action']['revision'],7)
        self.assertEqual(store['intent-remove']['body']['action']['type'],'delete')
        self.assertEqual(store['intent-stop']['body']['action']['payload'],{'kind':'interrupt'})
        self.assertEqual(store['intent-resume-queue']['body']['action'],{'type':'pause-queue','paused':False})

    def test_expired_go_still_allows_exact_recorded_resource_cleanup_only(self):
        d, store, calls = self.driver()
        d.go['expiresAt'] = 0
        d.delete()
        self.assertEqual(store['intent-delete']['body']['workspaceId'], 'workspace_fixture')
        with self.assertRaisesRegex(m.Failure, 'go_expired'): d.step('followup')
        self.assertEqual(len(calls), 1)

    def test_ticket_never_uses_persisting_effect_helper(self):
        # Guard the deliberate distinction: owner JWT and temporary gateway bearer
        # must never enter generic request/response journals or argv.
        import ast
        tree=ast.parse(path.read_text())
        method=next(n for n in ast.walk(tree) if isinstance(n,ast.FunctionDef) and n.name=='gateway')
        effect_calls=[n for n in ast.walk(method) if isinstance(n,ast.Call) and isinstance(n.func,ast.Attribute) and n.func.attr=='effect']
        self.assertEqual(effect_calls,[])

if __name__=='__main__':unittest.main()
