"""Prove that source timestamp restoration reuses only matching Cargo inputs."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import ci_source_cache


class SourceCacheTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="zuse-ci-cache-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.env = {k: v for k, v in os.environ.items()
                    if not k.startswith(("CARGO_", "RUST"))}
        self.env["RUSTC_WRAPPER"] = ""
        (self.root / "Cargo.toml").write_text(
            '[package]\nname="source-cache-fixture"\nversion="0.1.0"\nedition="2024"\n')
        (self.root / "src").mkdir()
        self.source = self.root / "src/lib.rs"
        self.source.write_text("pub fn value() -> u8 { 1 }\n")
        subprocess.run(["git", "init", "-q"], cwd=self.root, check=True)
        subprocess.run(["git", "add", "Cargo.toml", "src/lib.rs"], cwd=self.root, check=True)

    def cargo(self):
        return subprocess.run(["cargo", "build", "--offline", "--message-format=json"],
                              cwd=self.root, env=self.env, capture_output=True, text=True)

    def test_unchanged_checkout_reuses_compiled_artifacts(self):
        self.assertEqual(self.cargo().returncode, 0)
        ci_source_cache.save(self.root)
        before = self.source.stat().st_mtime_ns
        os.utime(self.source, ns=(before + 2_000_000_000, before + 2_000_000_000))
        ci_source_cache.restore(self.root)
        self.assertEqual(self.source.stat().st_mtime_ns, before)
        result = self.cargo()
        self.assertEqual(result.returncode, 0, result.stderr)
        artifacts = [json.loads(line) for line in result.stdout.splitlines()
                     if json.loads(line).get("reason") == "compiler-artifact"]
        self.assertTrue(artifacts)
        self.assertTrue(all(artifact["fresh"] for artifact in artifacts))

    def test_changed_source_still_rebuilds_and_catches_errors(self):
        self.assertEqual(self.cargo().returncode, 0)
        ci_source_cache.save(self.root)
        self.source.write_text('compile_error!("changed source must compile");\n')
        before = self.source.stat().st_mtime_ns
        ci_source_cache.restore(self.root)
        self.assertEqual(self.source.stat().st_mtime_ns, before)
        result = self.cargo()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("changed source must compile", result.stdout)

    def test_malformed_receipt_is_a_cache_miss(self):
        receipt = self.root / ci_source_cache.RECEIPT
        receipt.parent.mkdir()
        receipt.write_text("not JSON")
        self.assertEqual(ci_source_cache.restore(self.root), 0)

    def test_receipt_cannot_change_untracked_files(self):
        ci_source_cache.save(self.root)
        outside = self.root / "untracked.txt"
        outside.write_text("untracked")
        before = outside.stat().st_mtime_ns
        receipt_path = self.root / ci_source_cache.RECEIPT
        receipt = json.loads(receipt_path.read_text())
        receipt["files"]["untracked.txt"] = {
            "sha256": ci_source_cache.hashlib.sha256(outside.read_bytes()).hexdigest(),
            "mtime_ns": 1,
        }
        receipt_path.write_text(json.dumps(receipt))
        ci_source_cache.restore(self.root)
        self.assertEqual(outside.stat().st_mtime_ns, before)


if __name__ == "__main__":
    unittest.main()
