"""Exercise temporary-directory isolation across real test process trees."""
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest


RUNNER = Path(__file__).with_name("ci_test_runner.py")


class TestRunnerIsolationTests(unittest.TestCase):
    def probe(self):
        code = '''import json, os, subprocess, sys
from pathlib import Path
root = os.environ['TMPDIR']
child = subprocess.check_output([sys.executable, '-c', 'import os; print(os.environ["TMPDIR"])'], text=True).strip()
Path(root, 'effect-lock').write_text('fixture')
print(json.dumps([root, child]))
'''
        result = subprocess.run([sys.executable, str(RUNNER), sys.executable, "-c", code],
                                check=True, capture_output=True, text=True)
        return json.loads(result.stdout)

    def test_independent_tests_are_isolated_but_children_share_their_test_directory(self):
        with ThreadPoolExecutor(max_workers=2) as pool:
            first, second = list(pool.map(lambda _: self.probe(), range(2)))
        self.assertNotEqual(first[0], second[0])
        for parent, child in [first, second]:
            self.assertEqual(parent, child)
            self.assertFalse(Path(parent).exists(), "test scratch must be cleaned up")

    def test_failure_exit_status_is_preserved(self):
        result = subprocess.run([sys.executable, str(RUNNER), sys.executable,
                                 "-c", "raise SystemExit(17)"])
        self.assertEqual(result.returncode, 17)


if __name__ == "__main__":
    unittest.main()
