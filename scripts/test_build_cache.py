"""Exercise cache cleanup against Cargo in an isolated, disposable workspace."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import build_cache


class BuildCacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="zuse cache's ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "Cargo.toml").write_text(
            '[package]\nname="cache-fixture"\nversion="0.1.0"\nedition="2024"\n')
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("pub fn fixture() {}\n")
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith("CARGO_")}
        subprocess.run(["cargo", "generate-lockfile", "--offline"],
                       cwd=self.root, env=self.env, check=True, capture_output=True)
        self.target = self.root / "target with spaces"
        subprocess.run(["cargo", "check", "--offline", "--target-dir", str(self.target)],
                       cwd=self.root, env=self.env, check=True, capture_output=True)

    def artifact(self, path, content=b"cached artifact"):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        return path

    def test_missing_cache_needs_no_cleanup(self):
        with patch.object(build_cache.subprocess, "run") as clean:
            build_cache.prepare(root=self.root, target=self.root / "never-built", env=self.env)
        clean.assert_not_called()

    def test_under_budget_retains_artifacts(self):
        artifact = self.artifact(self.target / "debug/deps/fixture")
        build_cache.prepare(root=self.root, target=self.target, env=self.env)
        self.assertTrue(artifact.exists())

    def test_oversized_cache_uses_cargo_and_preserves_other_data(self):
        artifact = self.artifact(self.target / "debug/deps/old-fixture")
        release = self.artifact(self.target / "release/fixture")
        browser = self.artifact(self.target / "browser/debug/fixture")
        source = self.root / "src/lib.rs"
        before = source.read_bytes()
        build_cache.prepare(root=self.root, target=self.target, env=self.env, budget=1, clean=True)
        self.assertFalse(artifact.exists())
        self.assertTrue(release.exists())
        self.assertTrue(browser.exists())
        self.assertEqual(source.read_bytes(), before)

    def test_environment_target_is_discovered(self):
        artifact = self.artifact(self.target / "debug/deps/fixture")
        env = dict(self.env, CARGO_TARGET_DIR=str(self.target))
        actual = build_cache.prepare(root=self.root, env=env, budget=1, clean=True)
        self.assertEqual(actual, self.target)
        self.assertFalse(artifact.exists())

    def test_oversized_cache_stops_builds_without_automatic_deletion(self):
        artifact = self.artifact(self.target / "debug/deps/active-fixture")
        with patch.object(build_cache.subprocess, "run") as clean:
            with self.assertRaisesRegex(RuntimeError, "Stop builds.*--clean"):
                build_cache.prepare(root=self.root, target=self.target, env=self.env, budget=1)
        clean.assert_not_called()
        self.assertTrue(artifact.exists())

    @unittest.skipUnless(hasattr(os, "symlink"), "requires symlinks")
    def test_size_does_not_follow_links_or_count_hardlinks_twice(self):
        outside = self.artifact(self.root / "outside", b"x" * 1024)
        artifact = self.artifact(self.target / "debug/deps/fixture", b"x" * 10)
        (artifact.parent / "external").symlink_to(outside)
        os.link(artifact, artifact.parent / "same-artifact")
        with tempfile.TemporaryDirectory() as isolated:
            cache = Path(isolated)
            os.link(artifact, cache / "artifact")
            os.link(artifact, cache / "same-artifact")
            (cache / "external").symlink_to(outside)
            self.assertEqual(build_cache.directory_bytes(cache), 10)


if __name__ == "__main__":
    unittest.main()
