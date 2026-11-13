#!/usr/bin/env python3
"""Offline tests for infra/cloud/harness-bundle.py (no network, throwaway keys)."""
import contextlib
import http.server
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import threading
import types
import unittest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "infra/cloud/harness-bundle.py"
FIXTURE = ROOT / "crates/protocol/fixtures/harness-bundle-v1.json"
CONTRACT = ROOT / "crates/protocol/src/cloud_harness_updates.rs"
spec = importlib.util.spec_from_file_location("harness_bundle", SCRIPT)
tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tool)

FAKE_CLI = """#!/bin/sh
[ "$1" = --version ] || exit 2
[ -z "$ZUSE_FIXTURE_CREDENTIAL" ] || exit 3
echo "VERSION (fixture)"
"""


class FakeBucket(http.server.BaseHTTPRequestHandler):
    """Minimal path-style S3: HEAD/GET/PUT, requires SigV4 authorization."""
    objects = {}

    def log_message(self, *args):
        pass

    def authorized(self):
        if not self.headers.get("Authorization", "").startswith("AWS4-HMAC-SHA256 Credential=test-access/"):
            self.send_response(403)
            self.end_headers()
            return False
        return True

    def do_HEAD(self):
        if self.authorized():
            body = self.objects.get(self.path)
            self.send_response(200 if body is not None else 404)
            self.send_header("Content-Length", str(len(body) if body is not None else 0))
            self.end_headers()

    def do_GET(self):
        if self.authorized():
            body = self.objects.get(self.path)
            self.send_response(200 if body is not None else 404)
            self.send_header("Content-Length", str(len(body or b"")))
            self.end_headers()
            self.wfile.write(body or b"")

    def do_PUT(self):
        if self.authorized():
            self.objects[self.path] = self.rfile.read(int(self.headers["Content-Length"]))
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()


class HarnessBundleTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.fixture = json.loads(FIXTURE.read_text())

    def run_tool(self, *args, okay=True, env=None):
        result = subprocess.run(["python3", str(SCRIPT), *map(str, args)], capture_output=True, text=True,
                                env={**os.environ, **(env or {})})
        self.assertEqual(result.returncode == 0, okay, result.stderr + result.stdout)
        return result

    def key(self):
        path = self.directory / "key.pem"
        subprocess.run(["openssl", "genpkey", "-algorithm", "ed25519", "-out", str(path)], check=True,
                       capture_output=True)
        return path, tool.runtime.public_key_of(path)

    def tree(self, name="tree", version="2.1.312", agent="claude"):
        tree = self.directory / name
        executable = tree / tool.EXECUTABLES[agent]
        executable.parent.mkdir(parents=True)
        executable.write_text(FAKE_CLI.replace("VERSION", version))
        executable.chmod(0o755)
        (tree / "LICENSE").write_text("fixture license\n")
        return tree

    def build(self, tree=None, out="out", version="2.1.312", *extra, okay=True):
        out = self.directory / out
        result = self.run_tool("build-artifact", "--agent", "claude", "--version", version, "--arch", "x86_64",
                               "--tree", tree or self.tree(), "--out", out, *extra, okay=okay)
        return out, out / "bundle.json", result

    # -- contract ------------------------------------------------------------

    def test_signing_bytes_are_byte_identical_to_the_rust_fixture(self):
        self.assertEqual(tool.signing_bytes(self.fixture["bundle"]), self.fixture["signingBytes"].encode())
        shuffled = dict(reversed(list(self.fixture["bundle"].items())))
        self.assertEqual(tool.signing_bytes(shuffled), self.fixture["signingBytes"].encode())

    def test_domain_adapters_and_key_order_match_the_rust_contract(self):
        source = CONTRACT.read_text()
        self.assertIn('b"zuse-harness-bundle-v1\\n"', source)
        adapters = dict(re.findall(r'"(\w+)" => Some\("([a-z0-9.-]+)"\)', source))
        self.assertEqual(adapters, tool.ADAPTERS)
        struct = source.split("pub struct SignedHarnessBundle {", 1)[1].split("}", 1)[0]
        fields = re.findall(r"pub (\w+):", struct)
        camel = [re.sub(r"_(\w)", lambda m: m.group(1).upper(), name) for name in fields]
        self.assertEqual(tuple(camel), tool.SIGNED_KEYS)

    def test_fixture_signature_verifies_and_every_tamper_is_detected(self):
        data = tool.signing_bytes(self.fixture["bundle"])
        key, signature = self.fixture["testPublicKey"], self.fixture["testSignature"]
        self.assertTrue(tool.runtime.verify_bytes(data, signature, key))
        for field, value in [("sha256", "0" * 64), ("version", "0.165.1"), ("agent", "claude"),
                             ("executable", "bin/codex"), ("readsStateFormats", ["codex-sessions-v1"]),
                             ("capabilities", [])]:
            tampered = {**self.fixture["bundle"], field: value}
            self.assertFalse(tool.runtime.verify_bytes(tool.signing_bytes(tampered), signature, key), field)
        # Never valid as a runtime release signature over the same key.
        self.assertFalse(tool.runtime.verify_bytes(data, signature, tool.runtime.RELEASE_SIGNING_PUBLIC_KEY))

    def test_validation_rejects_unsafe_or_inconsistent_bundles(self):
        good = self.fixture["bundle"]
        for bad in [{**good, "version": "latest"}, {**good, "executable": "../codex"},
                    {**good, "executable": "/usr/bin/codex"}, {**good, "agent": "other"},
                    {**good, "readsStateFormats": ["other-v1"]}, {**good, "capabilities": ["Bad"]},
                    {**good, "extra": 1}]:
            with self.assertRaises(tool.ReleaseError):
                tool.signing_bytes(bad)

    # -- build ---------------------------------------------------------------

    def test_build_is_deterministic_and_content_addressed(self):
        out, manifest_path, _ = self.build()
        manifest = json.loads(manifest_path.read_text())
        bundle = manifest["bundle"]
        self.assertEqual(bundle["agent"], "claude")
        self.assertEqual(bundle["executable"], "bin/claude")
        self.assertEqual(bundle["adapter"], "claude-stream-json-v1")
        self.assertEqual(bundle["readsStateFormats"], ["claude-sessions-v1"])
        self.assertIsNone(manifest["signature"])
        self.assertEqual(manifest["artifactKey"], f"runtime/harness/claude/x86_64/{bundle['sha256']}.tar.zst")
        artifact = out / f"{bundle['sha256']}.tar.zst"
        self.assertEqual(manifest["compressedSizeBytes"], artifact.stat().st_size)
        self.assertEqual(tool.runtime.decompressed_sha256(artifact), bundle["sha256"])
        self.assertEqual([f["path"] for f in manifest["files"]], ["LICENSE", "bin/claude"])
        self.assertEqual([f["executable"] for f in manifest["files"]], [False, True])
        # Same tree, different mtimes and location: same digest.
        other = self.tree("again")
        os.utime(other / "LICENSE", (1, 1))
        _, again, _ = self.build(other, "out2")
        self.assertEqual(json.loads(again.read_text())["bundle"]["sha256"], bundle["sha256"])

    def test_build_records_extra_state_formats_and_capabilities(self):
        _, manifest_path, _ = self.build(None, "out", "2.1.312", "--reads-state-format", "codex-claude-tested-v1",
                                         "--capability", "broker-auth-v1")
        bundle = json.loads(manifest_path.read_text())["bundle"]
        self.assertEqual(bundle["readsStateFormats"], ["claude-sessions-v1", "codex-claude-tested-v1"])
        self.assertEqual(bundle["capabilities"], ["broker-auth-v1"])

    def test_build_refuses_wrong_version_links_and_non_executables(self):
        _, _, result = self.build(None, "x", "2.1.313", okay=False)
        self.assertIn("exact version", result.stderr)
        linked = self.tree("linked")
        (linked / "escape").symlink_to("/etc/passwd")
        _, _, result = self.build(linked, "y", okay=False)
        self.assertIn("not a regular file", result.stderr)
        plain = self.tree("plain")
        (plain / "bin/claude").chmod(0o644)
        _, _, result = self.build(plain, "z", okay=False)
        self.assertIn("not an executable", result.stderr)
        for version in ("latest", "2.1"):
            self.build(self.tree("t-" + version), "v-" + version, version, okay=False)

    def test_fetch_lays_out_the_verified_upstream_tree(self):
        def fake_fetch(agent, version, platform, output, keys, node=None):
            self.assertEqual((agent, version, platform, keys), ("codex", "0.165.0", "linux-x64", ["k"]))
            (output / "codex").write_text(FAKE_CLI.replace("VERSION", "codex-cli 0.165.0"))
            assets = output / "codex-assets/codex-package/codex-path"
            assets.mkdir(parents=True)
            (assets / "rg").write_text("rg")
            return {"npmIntegrity": "sha512-fixture",
                    "assets": {"codex-package/codex-path/rg": {"sha256": "0" * 64, "bytes": 2, "executable": True}}}
        fake = types.SimpleNamespace(get=lambda url: json.dumps({"keys": ["k"]}).encode(), fetch=fake_fetch)
        original = tool._load
        tool._load = lambda name, filename: fake
        try:
            tree, provenance = tool.fetch("codex", "0.165.0", "x86_64", self.directory / "fetched")
        finally:
            tool._load = original
        self.assertTrue(os.access(tree / "codex-package/bin/codex", os.X_OK))
        self.assertTrue(os.access(tree / "codex-package/codex-path/rg", os.X_OK))
        self.assertEqual(provenance["npmIntegrity"], "sha512-fixture")
        manifest = tool.build_artifact("codex", "0.165.0", "x86_64", tree, self.directory / "codex-out",
                                       provenance=provenance)
        self.assertEqual(manifest["bundle"]["executable"], "codex-package/bin/codex")
        self.assertEqual(manifest["upstream"]["npmIntegrity"], "sha512-fixture")
        with self.assertRaises(tool.ReleaseError):
            tool.fetch("codex", "latest", "x86_64", self.directory / "never")

    # -- sign / transfer -----------------------------------------------------

    def test_sign_verify_round_trip_and_tamper_detection(self):
        key, public = self.key()
        _, manifest_path, _ = self.build()
        self.run_tool("verify", "--bundle", manifest_path, "--public-key", public, okay=False)
        result = self.run_tool("sign", "--bundle", manifest_path, "--key", key, okay=False)
        self.assertIn("does not match the expected release public key", result.stderr)
        self.run_tool("sign", "--bundle", manifest_path, "--key", key, "--public-key", public)
        self.run_tool("verify", "--bundle", manifest_path, "--public-key", public)
        signed = self.run_tool("signing-bytes", "--bundle", manifest_path).stdout.encode()
        self.assertTrue(signed.startswith(b"zuse-harness-bundle-v1\n{\"sha256\":"))
        manifest = json.loads(manifest_path.read_text())
        manifest["bundle"]["readsStateFormats"].append("other-v1")
        manifest_path.write_text(json.dumps(manifest))
        self.run_tool("verify", "--bundle", manifest_path, "--public-key", public, okay=False)

    @contextlib.contextmanager
    def bucket(self):
        FakeBucket.objects = {}
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeBucket)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            yield {"ZUSE_CLOUD_RUNTIME_R2_BUCKET": "runtimes",
                   "ZUSE_CLOUD_RUNTIME_R2_ENDPOINT": f"http://127.0.0.1:{server.server_port}",
                   "ZUSE_CLOUD_RUNTIME_R2_ACCESS_KEY_ID": "test-access",
                   "ZUSE_CLOUD_RUNTIME_R2_SECRET_ACCESS_KEY": "test-secret"}
        finally:
            server.shutdown()
            server.server_close()

    def test_upload_is_signed_only_idempotent_and_downloads_verified(self):
        key, public = self.key()
        out, manifest_path, _ = self.build()
        with self.bucket() as env:
            self.run_tool("upload", "--bundle", manifest_path, "--artifact", out, "--public-key", public,
                          okay=False, env=env)
            self.assertEqual(FakeBucket.objects, {})
            self.run_tool("sign", "--bundle", manifest_path, "--key", key, "--public-key", public)
            first = json.loads(self.run_tool("upload", "--bundle", manifest_path, "--artifact", out,
                                             "--public-key", public, env=env).stdout)
            digest = json.loads(manifest_path.read_text())["bundle"]["sha256"]
            prefix = f"runtime/harness/claude/x86_64/{digest}"
            self.assertEqual(first, {f"{prefix}.tar.zst": "uploaded", f"{prefix}.bundle.json": "uploaded"})
            second = json.loads(self.run_tool("upload", "--bundle", manifest_path, "--artifact", out,
                                              "--public-key", public, env=env).stdout)
            self.assertEqual(set(second.values()), {"exists"})

            target = self.directory / "installed"
            self.run_tool("download", "--agent", "claude", "--sha256", digest, "--out", target,
                          "--public-key", public, env=env)
            self.assertTrue(os.access(target / "bin/claude", os.X_OK))
            self.assertFalse(os.access(target / "LICENSE", os.W_OK))
            self.run_tool("download", "--agent", "claude", "--sha256", digest, "--out", self.directory / "bad",
                          env=env, okay=False)  # contract key
            FakeBucket.objects[f"/runtimes/{prefix}.tar.zst"] = subprocess.run(
                ["zstd", "-q", "-c"], input=b"evil", capture_output=True, check=True).stdout
            self.run_tool("download", "--agent", "claude", "--sha256", digest, "--out", self.directory / "evil",
                          "--public-key", public, env=env, okay=False)
            self.assertFalse((self.directory / "evil").exists())

    def test_extraction_rejects_links_and_traversal(self):
        bundle = {**self.fixture["bundle"], "executable": "bin/x"}
        for member in (tarfile.TarInfo("../escape"), tarfile.TarInfo("bin/link")):
            if member.name == "bin/link":
                member.type, member.linkname = tarfile.SYMTYPE, "/etc/passwd"
            archive = self.directory / "evil.tar"
            with tarfile.open(archive, "w") as out:
                out.addfile(member, io.BytesIO(b""))
            with self.assertRaises(tool.ReleaseError):
                tool.safe_extract(archive, self.directory / ("x" + str(id(member))), bundle)


if __name__ == "__main__":
    unittest.main()
