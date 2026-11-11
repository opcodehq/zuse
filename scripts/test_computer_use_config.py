import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("computer-use-config.py")
spec = importlib.util.spec_from_file_location("computer_use_config", SCRIPT)
config = importlib.util.module_from_spec(spec)
spec.loader.exec_module(config)


class ComputerUseConfiguration(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        self.executable = Path(self.root.name) / "cu with spaces"
        self.executable.write_text("#!/bin/sh\nprintf started > executable-started\nexit 99\n")
        self.executable.chmod(0o700)

    def test_default_is_untrusted_and_has_no_read_or_credential_exemptions(self):
        server = config.configuration(self.executable)["servers"]["opcode-cu"]
        self.assertFalse(server["trusted"])
        self.assertEqual(server["readOnlyTools"], [])
        self.assertEqual(server["connection"]["env_refs"], {})
        self.assertEqual(server["connection"]["args"], ["mcp"])

    def test_trust_and_desktop_environment_are_explicit_references(self):
        with patch.dict(os.environ, {"DISPLAY": "private-display-value"}):
            value = config.configuration(self.executable, True, ["DISPLAY", "DISPLAY"])
        server = value["servers"]["opcode-cu"]
        self.assertTrue(server["trusted"])
        self.assertEqual(server["connection"]["env_refs"], {"DISPLAY": "DISPLAY"})
        self.assertNotIn("private-display-value", json.dumps(value))
        with self.assertRaises(ValueError):
            config.configuration(self.executable, True, ["OPENAI_API_KEY"])

    def test_invalid_executables_fail_closed(self):
        for path in ["cu", Path(self.root.name), Path(self.root.name) / "missing"]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                config.configuration(path)
        self.executable.chmod(0o600)
        with self.assertRaises(ValueError):
            config.configuration(self.executable)

    def test_cli_prints_without_running_cu_or_writing_a_profile(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--cu", str(self.executable), "--trust-desktop"],
            capture_output=True, text=True, check=True, cwd=self.root.name,
        )
        server = json.loads(result.stdout)["servers"]["opcode-cu"]
        self.assertEqual(server["connection"]["executable"], str(self.executable))
        self.assertTrue(server["trusted"])
        self.assertIn("No configuration was written", result.stderr)
        self.assertEqual(list(Path(self.root.name).iterdir()), [self.executable])


if __name__ == "__main__":
    unittest.main()
