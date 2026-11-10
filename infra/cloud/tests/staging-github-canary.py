#!/usr/bin/env python3
"""Offline protocol/private-profile tests; no real login or GitHub calls."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('canary', Path(__file__).resolve().parents[1] / 'staging-github-canary.py')
canary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(canary)


class Transport:
    def __init__(self):
        self.calls = []
        self.queue = []

    def request(self, method, url, **kwargs):
        self.calls.append((method, url, kwargs))
        return self.queue.pop(0)


class CanaryTest(unittest.TestCase):
    def prepare(self, root, http):
        result = canary.Canary(root, 'user_fixture', http, lambda: 1000)
        canary.device.write_private(Path(root) / 'session.json', {
            'subject': 'user_fixture', 'clientId': canary.device.CLIENT_ID,
            'expiresAt': 1300, 'verified': True, 'accessToken': 'fixture-private-token',
            'refreshToken': 'fixture-private-refresh'})
        return result

    def checks(self, http):
        http.queue.extend([(200, {'actor': {'accountId': 'user_fixture'}, 'ownerId': 'user_fixture'}),
                           (200, {'nativeAuthority': True, 'user': None, 'installations': [], 'repositories': []})])

    def test_begin_is_private_and_reopening_retains_the_same_operation(self):
        with tempfile.TemporaryDirectory() as root:
            http = Transport()
            flow = self.prepare(root, http)
            self.checks(http)
            operation = '01234567-89ab-4def-8abc-0123456789ab'
            private_url = 'https://api-rust-stag.zuse.sh/v1/cloud/github/connect?state=' + 'a' * 64
            http.queue.append((200, {'url': private_url, 'operationId': operation, 'expiresAt': 1200000}))
            result = flow.run('begin')
            self.assertEqual(result['state'], 'awaiting_browser')
            self.assertNotIn(private_url, json.dumps(result))
            self.assertNotIn('fixture-private-token', json.dumps(result))
            path = Path(result['browserUrlPath'])
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.checks(http)
            http.queue.append((200, {'state': 'authorizing'}))
            reopened = flow.run('begin')
            self.assertEqual(reopened['operationId'], operation)
            self.assertEqual(sum(method == 'POST' for method, _, _ in http.calls), 1)

    def test_foreign_browser_url_and_wrong_owner_fail_closed(self):
        with tempfile.TemporaryDirectory() as root:
            http = Transport()
            flow = self.prepare(root, http)
            self.checks(http)
            http.queue.append((200, {'url': 'https://foreign.example/v1/cloud/github/connect?state=' + 'a' * 64,
                                    'operationId': '01234567-89ab-4def-8abc-0123456789ab', 'expiresAt': 1200000}))
            with self.assertRaises(canary.device.Failure):
                flow.run('begin')
            self.assertFalse((Path(root) / 'github-browser-url').exists())
            session = canary.device.read_private(Path(root) / 'session.json')
            session['subject'] = 'user_foreign'
            canary.device.write_private(Path(root) / 'session.json', session)
            with self.assertRaises(canary.device.Failure):
                flow.run('check')
            self.assertEqual(len(http.calls), 3)

    def test_explicit_add_after_completion_gets_new_selection_but_pending_reuses(self):
        with tempfile.TemporaryDirectory() as root:
            http=Transport(); flow=self.prepare(root,http)
            old='01234567-89ab-4def-8abc-0123456789ab'
            new='11234567-89ab-4def-8abc-0123456789ab'
            canary.device.write_private(Path(root)/'github-operation.json',{'operationId':old,'ownerId':'user_fixture','expiresAt':1200000})
            self.checks(http); http.queue.append((200,{'state':'complete'})); self.checks(http)
            url='https://api-rust-stag.zuse.sh/v1/cloud/github/connect?state='+'b'*64
            http.queue.append((200,{'url':url,'operationId':new,'expiresAt':1300000}))
            self.assertEqual(flow.run('open_installation_link'),url)
            self.assertEqual(sum(m=='POST' for m,_,_ in http.calls),1)
            self.checks(http); http.queue.append((200,{'state':'choosing'}))
            self.assertEqual(flow.run('open_installation_link'),url)
            self.assertEqual(sum(m=='POST' for m,_,_ in http.calls),1)
            self.assertEqual(canary.device.read_private(Path(root)/'github-operation.json')['expiresAt'],1300000)

    def test_plain_begin_complete_does_not_silently_request_another_selection(self):
        with tempfile.TemporaryDirectory() as root:
            http=Transport(); flow=self.prepare(root,http)
            canary.device.write_private(Path(root)/'github-operation.json',{'operationId':'01234567-89ab-4def-8abc-0123456789ab','ownerId':'user_fixture','expiresAt':1200000})
            self.checks(http); http.queue.append((200,{'state':'complete'})); self.checks(http)
            self.assertEqual(flow.run('begin')['state'],'complete')
            self.assertFalse(any(m=='POST' for m,_,_ in http.calls))

    def test_private_link_refuses_symlink_without_opening_or_restarting(self):
        with tempfile.TemporaryDirectory() as root:
            http=Transport(); flow=self.prepare(root,http)
            canary.device.write_private(Path(root)/'github-operation.json',{'operationId':'01234567-89ab-4def-8abc-0123456789ab','ownerId':'user_fixture','expiresAt':1200000})
            (Path(root)/'github-browser-url').symlink_to(Path(root)/'foreign')
            self.checks(http);http.queue.append((200,{'state':'choosing'}))
            with self.assertRaises(OSError):flow.run('open_installation_link')
            self.assertFalse(any(m=='POST' for m,_,_ in http.calls))


if __name__ == '__main__':
    unittest.main()
