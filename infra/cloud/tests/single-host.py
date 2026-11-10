"""Deployment isolation checks. COMPOSE_TEST_BIN enables real Compose rendering."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

MODULE = Path(__file__).resolve().parents[1] / "single-host" / "deploy.py"
spec = importlib.util.spec_from_file_location("single_host", MODULE)
deploy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deploy)


class SingleHostTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "private"
        deploy.initialize(self.root, "api.example.com", "stage.example.com", "example/cloud:commit")
        for name in deploy.ENVIRONMENTS:
            path = self.root / name / "runtime.env"
            path.write_text(path.read_text().replace("ZUSE_CLOUD_DATABASE_URL=\n",
                f"ZUSE_CLOUD_DATABASE_URL=postgresql://user:secret@db.example.com/zuse_{name}?sslmode=verify-full\n"))

    def test_private_config_and_no_overwrite(self):
        for name in deploy.ENVIRONMENTS:
            self.assertEqual((self.root / name / "runtime.env").stat().st_mode & 0o777, 0o600)
            self.assertNotIn("POSTGRES_PASSWORD", deploy.read_env(self.root / name / "deploy.env"))
        before = (self.root / "production" / "runtime.env").read_bytes()
        with self.assertRaises(ValueError):
            deploy.initialize(self.root, "api.example.com", "stage.example.com", "new:release")
        self.assertEqual(before, (self.root / "production" / "runtime.env").read_bytes())

    def test_startup_preflight_never_drains_jobs_and_accepts_other_providers(self):
        runtime = self.root / "staging" / "runtime.env"
        runtime.write_text(runtime.read_text().replace(
            "WORKOS_CLIENT_ID=REPLACE_WITH_ENVIRONMENT_CLIENT_ID", "WORKOS_CLIENT_ID=client_fixture"
        ).replace("API_ISSUER=REPLACE_WITH_CONTROL_PLANE_ISSUER", "API_ISSUER=https://stage.example.com")
            + "\nCLOUD_DATA_ENCRYPTION_KEY=fixture\nRELAY_MINT_PRIVATE_JWK=fixture\n")
        worker = self.root / "staging" / "worker.env"
        # A Boat-only deployment must not require a Boxd credential.
        worker.write_text(worker.read_text().replace("BOAT_API_KEY=\n", "BOAT_API_KEY=fixture\n"))
        with patch("sys.argv", [str(MODULE), "--config-root", str(self.root), "up", "staging"]), \
                patch.object(deploy.subprocess, "run") as run:
            deploy.main()
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(len(commands), 2)
        self.assertEqual(commands[0][-6:], ["run", "--rm", "--no-deps", "worker", "worker", "--check"])
        self.assertFalse(any("--once" in command for command in commands))
        self.assertIn("--detach", commands[1])

    def test_github_secrets_are_api_only_and_readable_by_container_user(self):
        from types import SimpleNamespace
        self.assertTrue(deploy.api_secret_is_readable(SimpleNamespace(st_uid=10001,st_gid=10001,st_mode=0o400)))
        self.assertTrue(deploy.api_secret_is_readable(SimpleNamespace(st_uid=0,st_gid=10001,st_mode=0o440)))
        self.assertFalse(deploy.api_secret_is_readable(SimpleNamespace(st_uid=0,st_gid=0,st_mode=0o600)))
        self.assertFalse(deploy.api_secret_is_readable(SimpleNamespace(st_uid=10001,st_gid=10001,st_mode=0o444)))
        path=self.root / "staging" / "runtime.env"
        path.write_text(path.read_text()+"\nZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS=test-owner\n")
        with self.assertRaisesRegex(ValueError,"API-only"):
            deploy.compose(self.root,"staging")
        path.write_text(path.read_text().replace("ZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS=test-owner", ""))
        api=self.root / "staging" / "api.env"
        api.write_text("ZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS=test-owner\nGITHUB_APP_ID=123\nGITHUB_APP_SLUG=staging\nGITHUB_APP_CLIENT_ID=client\nZUSE_CLOUD_GITHUB_ORIGIN=https://stage.example.com\nGITHUB_APP_PRIVATE_KEY_FILE=/run/zuse/github/app.pem\nGITHUB_APP_CLIENT_SECRET_FILE=/run/zuse/github/oauth-client-secret\nGITHUB_WEBHOOK_SECRET_FILE=/run/zuse/github/webhook-secret\n")
        directory=self.root / "staging" / "github-api";directory.mkdir()
        for name in ("app.pem","oauth-client-secret","webhook-secret"):
            secret=directory/name;secret.write_text("fixture");secret.chmod(0o400)
        with patch.object(deploy,"api_secret_is_readable",return_value=True):
            command,_=deploy.compose(self.root,"staging")
        self.assertIn(str(deploy.HERE/"github-api.yaml"),command)
        production,_=deploy.compose(self.root,"production")
        self.assertNotIn(str(deploy.HERE/"github-api.yaml"),production)

    def test_account_event_signals_are_opt_in_private_and_environment_isolated(self):
        from types import SimpleNamespace
        self.assertTrue(deploy.signal_directory_is_private(SimpleNamespace(st_uid=10001,st_mode=0o40700)))
        self.assertFalse(deploy.signal_directory_is_private(SimpleNamespace(st_uid=0,st_mode=0o40700)))
        self.assertFalse(deploy.signal_directory_is_private(SimpleNamespace(st_uid=10001,st_mode=0o40755)))
        runtime=self.root/"staging"/"runtime.env"
        runtime.write_text(runtime.read_text()+"\nZUSE_CLOUD_ACCOUNT_EVENTS=true\n")
        with self.assertRaisesRegex(ValueError,"private"):
            deploy.compose(self.root,"staging")
        signals=self.root/"staging"/"event-signals";signals.mkdir(mode=0o700)
        with patch.object(deploy,"signal_directory_is_private",return_value=True):
            command,_=deploy.compose(self.root,"staging")
        self.assertIn(str(deploy.HERE/"account-events.yaml"),command)
        production,_=deploy.compose(self.root,"production")
        self.assertNotIn(str(deploy.HERE/"account-events.yaml"),production)
        signals.rmdir();signals.symlink_to(self.root/"production",target_is_directory=True)
        with self.assertRaisesRegex(ValueError,"private"):
            deploy.compose(self.root,"staging")

    def test_shell_cannot_redirect_environment_and_shared_database_is_rejected(self):
        with patch.dict(os.environ, {"ZUSE_ENV": "production", "APP_MEMORY": "99g",
                                     "COMPOSE_PROFILES": "billing", "COMPOSE_FILE": "/unexpected.yaml"}):
            command, env = deploy.compose(self.root, "staging")
        self.assertIn("zuse-staging", command)
        for key in ("ZUSE_ENV", "APP_MEMORY", "COMPOSE_PROFILES", "COMPOSE_FILE"):
            self.assertNotIn(key, env)
        self.assertEqual(env["ZUSE_CONFIG_DIR"], str(self.root / "staging"))
        path = self.root / "staging" / "runtime.env"
        path.write_text(path.read_text().replace("zuse_staging", "zuse_production").replace("user:secret", "different:password"))
        with self.assertRaisesRegex(ValueError, "separate external databases"):
            deploy.compose(self.root, "staging")

    def test_same_database_requires_distinct_expected_schemas_and_roles(self):
        for name in deploy.ENVIRONMENTS:
            path = self.root / name / "runtime.env"
            text = path.read_text()
            lines = [line for line in text.splitlines() if not line.startswith("ZUSE_CLOUD_DATABASE_URL=")]
            lines.append(f"ZUSE_CLOUD_DATABASE_URL=postgresql://zuse_cloud_{name}:secret@db.example.com/postgres?sslmode=verify-full&options=-csearch_path%3Dzuse_{name}%2Cpg_catalog")
            path.write_text("\n".join(lines) + "\n")
        deploy.compose(self.root, "staging")
        path = self.root / "staging" / "runtime.env"
        path.write_text(path.read_text().replace("zuse_cloud_staging:", "zuse_cloud_production:"))
        with self.assertRaises(ValueError):
            deploy.compose(self.root, "staging")

    def test_invalid_domains_rejected_before_writes(self):
        for host in ("https://api.example.com", "api.example.com/path", "api.example.com\nother", "*.example.com"):
            with self.assertRaises(ValueError):
                deploy.initialize(self.root / "invalid", host, "stage.example.com", "example:commit")
        self.assertFalse((self.root / "invalid").exists())

    def test_authority_profile_is_explicit_and_environment_scoped(self):
        command, _ = deploy.compose(self.root, "staging")
        self.assertNotIn("--profile", command)
        path = self.root / "staging" / "runtime.env"
        path.write_text(path.read_text().replace("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS=\n",
                                               "ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS=test-user\n"))
        command, _ = deploy.compose(self.root, "staging")
        self.assertEqual(command[-2:], ["--profile", "authority"])
        command, _ = deploy.compose(self.root, "production")
        self.assertNotIn("--profile", command)

    def test_optional_ssd_scratch_selects_stage_overlay_only_after_preflight(self):
        path = self.root / "staging" / "deploy.env"
        with path.open("a") as target:
            target.write(f"VERIFICATION_SCRATCH_DIR={self.root}/staging/verification-scratch-fs/private\nVERIFICATION_SCRATCH_LIMIT_BYTES=805306368\n")
        with patch.object(deploy, "verification_scratch", return_value={"hardQuota": True}) as checked:
            command, _ = deploy.compose(self.root, "staging")
        self.assertEqual(checked.call_args.args[:2], (self.root, "staging"))
        self.assertIn(str(deploy.HERE / "verification-scratch.yaml"), command)
        with patch.object(deploy, "verification_scratch", side_effect=ValueError("verification_scratch_not_mounted")):
            with self.assertRaisesRegex(ValueError, "verification_scratch_not_mounted"):
                deploy.compose(self.root, "staging")
        command, _ = deploy.compose(self.root, "production")
        self.assertNotIn(str(deploy.HERE / "verification-scratch.yaml"), command)

    @unittest.skipUnless(os.environ.get("COMPOSE_TEST_BIN"), "Set COMPOSE_TEST_BIN for Compose rendering")
    def test_real_compose_has_isolated_api_workers_and_no_database(self):
        models = {}
        enabled = {}
        for name in (*deploy.ENVIRONMENTS, "edge"):
            if name != "edge":
                with (self.root / name / "runtime.env").open("a") as target:
                    target.write('RELAY_MINT_PRIVATE_JWK={"d":"a$literal"}\n')
            command, env = deploy.compose(self.root, name)
            env["literal"] = "SHOULD_NOT_EXPAND"
            result = subprocess.run([os.environ["COMPOSE_TEST_BIN"], *command[2:], "config", "--format", "json"],
                                    env=env, capture_output=True, check=True, text=True)
            models[name] = json.loads(result.stdout)
            if name != "edge":
                result = subprocess.run([os.environ["COMPOSE_TEST_BIN"], *command[2:], "--profile", "authority", "config", "--format", "json"],
                                        env=env, capture_output=True, check=True, text=True)
                enabled[name] = json.loads(result.stdout)
        for name in deploy.ENVIRONMENTS:
            model = models[name]
            self.assertEqual(model["name"], f"zuse-{name}")
            self.assertEqual(set(model["services"]), {"api", "worker"})
            self.assertEqual(set(enabled[name]["services"]), {"api", "worker", "auth-worker"})
            self.assertFalse(model.get("volumes"))
            self.assertEqual(model["networks"]["edge"]["name"], f"zuse-{name}-edge")
            gateway = model["services"]["api"]
            self.assertEqual(gateway["command"], ["serve"])
            self.assertIn(f"zuse-{name}-api", gateway["networks"]["edge"]["aliases"])
            key = gateway["environment"]["RELAY_MINT_PRIVATE_JWK"].replace("$$", "$")
            self.assertEqual(key, '{"d":"a$literal"}')
            self.assertGreater(int(gateway["mem_limit"]), 0)
            self.assertEqual(gateway["ports"][0]["host_ip"], "127.0.0.1")
            worker = model["services"]["worker"]
            self.assertEqual(worker["command"], ["worker"])
            self.assertFalse(worker.get("ports"))
            self.assertIn("BOXD_API_KEY", worker["environment"])
            self.assertNotIn("BOXD_API_KEY", gateway["environment"])
            authority = enabled[name]["services"]["auth-worker"]
            self.assertEqual(authority["profiles"], ["authority"])
            self.assertEqual(authority["command"], ["auth-worker"])
            self.assertNotIn("BOXD_API_KEY", authority["environment"])
            self.assertNotIn("DAYTONA_API_KEY", authority["environment"])
            self.assertFalse(authority.get("ports"))
            mount = gateway["volumes"][0]
            self.assertEqual(mount["type"], "bind")
            self.assertTrue(mount["read_only"])
            self.assertFalse(mount.get("bind", {}).get("create_host_path", False))
            self.assertEqual(mount["target"], "/etc/zuse/cloud/database-ca.crt")
        ingress = models["edge"]
        ports = {p["published"] for p in ingress["services"]["caddy"]["ports"]}
        self.assertEqual(ports, {"80", "443"})
        self.assertEqual({n["name"] for n in ingress["networks"].values()},
                         {"zuse-staging-edge", "zuse-production-edge"})
        self.assertTrue(all(n["enable_ipv6"] for n in ingress["networks"].values()))


if __name__ == "__main__":
    unittest.main()
