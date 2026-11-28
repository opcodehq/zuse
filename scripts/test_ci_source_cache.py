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
        # Preserve the runner's Rust installation location and selected toolchain.
        self.env = {k: v for k, v in os.environ.items()
                    if not k.startswith("CARGO_")
                    and k not in ("RUSTFLAGS", "RUSTC_WORKSPACE_WRAPPER")}
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
        initial = self.cargo()
        self.assertEqual(initial.returncode, 0, initial.stderr)
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
        initial = self.cargo()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        ci_source_cache.save(self.root)
        self.source.write_text('compile_error!("changed source must compile");\n')
        before = self.source.stat().st_mtime_ns
        ci_source_cache.restore(self.root)
        self.assertEqual(self.source.stat().st_mtime_ns, before)
        result = self.cargo()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("changed source must compile", result.stdout)

    def test_directory_watched_by_build_script_stays_fresh(self):
        (self.root / "assets").mkdir()
        (self.root / "assets/input").write_text("original")
        (self.root / "build.rs").write_text(
            'fn main() { println!("cargo:rerun-if-changed=assets"); }\n')
        subprocess.run(["git", "add", "assets", "build.rs"], cwd=self.root, check=True)
        initial = self.cargo()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        ci_source_cache.save(self.root)
        directory = self.root / "assets"
        before = directory.stat().st_mtime_ns
        os.utime(directory, ns=(before + 2_000_000_000, before + 2_000_000_000))
        ci_source_cache.restore(self.root)
        self.assertEqual(directory.stat().st_mtime_ns, before)
        result = self.cargo()
        self.assertEqual(result.returncode, 0, result.stderr)
        artifacts = [json.loads(line) for line in result.stdout.splitlines()
                     if json.loads(line).get("reason") == "compiler-artifact"]
        self.assertTrue(all(artifact["fresh"] for artifact in artifacts))
        # Added inputs, including untracked generated files, invalidate the tree.
        (directory / "new-input").write_text("new")
        changed = directory.stat().st_mtime_ns
        ci_source_cache.restore(self.root)
        self.assertEqual(directory.stat().st_mtime_ns, changed)
        result = self.cargo()
        self.assertEqual(result.returncode, 0, result.stderr)
        artifacts = [json.loads(line) for line in result.stdout.splitlines()
                     if json.loads(line).get("reason") == "compiler-artifact"]
        self.assertTrue(any(not artifact["fresh"] for artifact in artifacts))

    def test_malformed_receipt_is_a_cache_miss(self):
        receipt = self.root / ci_source_cache.RECEIPT
        receipt.parent.mkdir()
        receipt.write_text("not JSON")
        self.assertEqual(ci_source_cache.restore(self.root), 0)

    def test_legacy_receipt_preserves_untracked_input_deletion(self):
        (self.root / "assets").mkdir()
        (self.root / "assets/input").write_text("original")
        generated = self.root / "assets/generated"
        generated.write_text("generated")
        (self.root / "build.rs").write_text(
            'fn main() { println!("cargo:rerun-if-changed=assets"); '
            'std::fs::read("assets/generated").expect("missing generated input"); }\n')
        subprocess.run(["git", "add", "assets/input", "build.rs"], cwd=self.root, check=True)
        self.assertEqual(self.cargo().returncode, 0)
        ci_source_cache.save(self.root)
        receipt_path = self.root / ci_source_cache.RECEIPT
        receipt = json.loads(receipt_path.read_text())
        del receipt["directories"]
        receipt_path.write_text(json.dumps(receipt))
        directory = self.root / "assets"
        generated.unlink()
        changed = directory.stat().st_mtime_ns + 2_000_000_000
        os.utime(directory, ns=(changed, changed))
        ci_source_cache.restore(self.root)
        self.assertEqual(directory.stat().st_mtime_ns, changed)
        result = self.cargo()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing generated input", result.stderr)

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
