import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from cargo_dev import ROOT, build_environment, icon_fingerprint, target_directory


class CargoDevTests(unittest.TestCase):
    def test_worktrees_share_target_but_independent_repositories_do_not(self):
        with tempfile.TemporaryDirectory(prefix="zuse build ") as temporary:
            base = Path(temporary)
            repo, other, worktree = (base / name for name in ("repo", "other", "worktree"))
            for directory in (repo, other):
                subprocess.run(["git", "init", "-q", str(directory)], check=True)
            subprocess.run([
                "git", "-C", str(repo), "-c", "user.name=Test", "-c",
                "user.email=test@example.com", "-c", "commit.gpgsign=false",
                "commit", "-qm", "initial", "--allow-empty",
            ], check=True)
            subprocess.run(["git", "-C", str(repo), "worktree", "add", "--detach",
                            str(worktree)], check=True, capture_output=True)
            env = {"XDG_CACHE_HOME": str(base / "cache")}
            self.assertEqual(target_directory(repo, env), target_directory(worktree, env))
            self.assertNotEqual(target_directory(repo, env), target_directory(other, env))
            self.assertTrue(target_directory(repo, env).is_relative_to(base / "cache"))

    def test_explicit_target_and_ci_keep_their_locations(self):
        root = Path("/checkout")
        self.assertEqual(target_directory(root, {"CI": "true"}), root / "target")
        self.assertEqual(target_directory(root, {"CI": "true", "CARGO_TARGET_DIR": "/custom cache"}),
                         Path("/custom cache"))

    def test_fingerprint_tracks_key_rotation_without_recording_secret(self):
        env = build_environment(env={"CI": "true", "HUGEICONS_LICENSE_KEY": "first-secret"})
        self.assertNotIn("first-secret", env["HUGEICONS_BUILD_KEY"])
        self.assertNotEqual(env["HUGEICONS_BUILD_KEY"], icon_fingerprint({"HUGEICONS_LICENSE_KEY": "second-secret"}))
        self.assertEqual(icon_fingerprint({}), icon_fingerprint({"HUGEICONS_LICENSE_KEY": " \n"}))

    def test_unchanged_icons_stay_fresh_across_cargo_commands(self):
        # A tiny independent workspace exercises Cargo's actual freshness rules
        # without fetching the application or its native platform dependencies.
        with tempfile.TemporaryDirectory(prefix="zuse icons ") as temporary:
            root = Path(temporary)
            shutil.copytree(ROOT / "crates/icons", root / "crates/icons")
            (root / "scripts").mkdir()
            shutil.copy(ROOT / "scripts/prepare-icons.mjs", root / "scripts")
            (root / "Cargo.toml").write_text(
                '[workspace]\nmembers = ["crates/icons", "consumer"]\nresolver = "2"\n'
                '[workspace.package]\nversion = "0.1.0"\nedition = "2024"\n'
                'license = "AGPL-3.0-only"\npublish = false\n'
            )
            (root / "consumer/src").mkdir(parents=True)
            (root / "consumer/Cargo.toml").write_text(
                '[package]\nname = "icon-consumer"\nversion = "0.1.0"\nedition = "2024"\n'
                '[dependencies]\nzuse-icons = { path = "../crates/icons" }\n'
            )
            (root / "consumer/src/lib.rs").write_text('pub fn style() -> &\'static str { zuse_icons::STYLE }\n')
            env = dict(os.environ, CARGO_TARGET_DIR=str(root / "target"), HUGEICONS_LICENSE_KEY="")
            env["HUGEICONS_BUILD_KEY"] = icon_fingerprint(env)
            command = ["cargo", "build", "--offline", "-v", "-p", "icon-consumer"]
            for iteration in range(2):
                result = subprocess.run(command, cwd=root, env=env, text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("Compiling zuse-icons" if iteration == 0 else "Fresh zuse-icons", result.stderr)
                self.assertIn("Compiling icon-consumer" if iteration == 0 else "Fresh icon-consumer", result.stderr)
            # Bare Cargo must still select styles automatically rather than
            # silently reusing a launcher's previous selection.
            del env["HUGEICONS_BUILD_KEY"]
            result = subprocess.run(command, cwd=root, env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Compiling zuse-icons", result.stderr)


if __name__ == "__main__":
    unittest.main()
