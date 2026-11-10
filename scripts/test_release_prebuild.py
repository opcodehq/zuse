"""Release reuse must be complete, immutable, and avoid duplicate compilation."""
import unittest
from unittest.mock import Mock

from release_prebuild import EXPECTED, find_prebuild


def run(run_id=42, status='completed', conclusion='success', commit='abc', branch='main', event='push'):
    return dict(id=run_id, status=status, conclusion=conclusion, head_sha=commit,
                head_branch=branch, event=event)


def artifacts(missing=(), expired=()):
    return {'artifacts': [{'name': name, 'expired': name in expired}
                          for name in EXPECTED if name not in missing]}


class PrebuildTests(unittest.TestCase):
    def lookup(self, responses, **kwargs):
        request = Mock(side_effect=responses)
        result = find_prebuild('owner/repo', 'abc', request=request, **kwargs)
        return result, request

    def test_reuses_complete_successful_exact_commit(self):
        result, request = self.lookup([{'workflow_runs': [run()]}, artifacts()])
        self.assertEqual(result, '42')
        self.assertIn('head_sha=abc', request.call_args_list[0].args[0])

    def test_rejects_wrong_commit_branch_or_event(self):
        for other in [run(commit='def'), run(branch='other'), run(event='pull_request')]:
            result, request = self.lookup([{'workflow_runs': [other]}])
            self.assertEqual(result, '')
            self.assertEqual(request.call_count, 1)

    def test_missing_or_expired_web_or_native_artifact_requires_build(self):
        for name in EXPECTED:
            for response in [artifacts(missing={name}), artifacts(expired={name})]:
                result, _ = self.lookup([{'workflow_runs': [run()]}, response])
                self.assertEqual(result, '')

    def test_waits_for_active_prebuild_then_reuses_it(self):
        sleep = Mock()
        result, _ = self.lookup([
            {'workflow_runs': [run(status='in_progress', conclusion=None)]},
            run(status='in_progress', conclusion=None), run(), artifacts()
        ], sleep=sleep)
        self.assertEqual(result, '42')
        sleep.assert_called_once_with(30)

    def test_failed_or_cancelled_prebuild_falls_back(self):
        for outcome in ['failure', 'cancelled']:
            result, _ = self.lookup([
                {'workflow_runs': [run(status='queued', conclusion=None)]},
                run(conclusion=outcome)
            ])
            self.assertEqual(result, '')

    def test_previous_success_is_reused_despite_new_pending_attempt(self):
        result, _ = self.lookup([
            {'workflow_runs': [run(43, status='queued', conclusion=None), run()]}, artifacts()
        ])
        self.assertEqual(result, '42')

    def test_timeout_fails_instead_of_launching_duplicate_build(self):
        request = Mock(side_effect=[{'workflow_runs': [run(status='queued', conclusion=None)]},
                                    run(status='in_progress', conclusion=None)])
        with self.assertRaisesRegex(TimeoutError, 'still running'):
            find_prebuild('owner/repo', 'abc', request=request,
                          clock=Mock(side_effect=[0, 10]), timeout=10)

    def test_commit_mismatch_while_waiting_fails_closed(self):
        with self.assertRaisesRegex(ValueError, 'commit changed'):
            self.lookup([{'workflow_runs': [run(status='queued', conclusion=None)]}, run(commit='def')])


if __name__ == '__main__':
    unittest.main()
