#!/usr/bin/env python3
"""Offline tests for infra/cloud/runtime-release.py (no network, throwaway keys)."""
import contextlib
import datetime
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "infra/cloud/runtime-release.py"
FIXTURE = ROOT / "crates/protocol/fixtures/runtime-release-v1.json"
CONTRACT = ROOT / "crates/protocol/src/cloud_runtime_updates.rs"
spec = importlib.util.spec_from_file_location("runtime_release", SCRIPT)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)

FAKE_RUNTIME = """#!/bin/sh
[ "$1" = runtime-capabilities ] || exit 2
sha=SHA_EXPR
printf '{"version":1,"runtimeSha256":"%s","commandAdmissionVersion":2,"runtimeRpcProtocol":"zuse-rpc-v1","storageFormat":"zuse-rust-engine-v1","sessionCommandVersion":1,"executionProgressVersion":1,"managedAgentVersion":1}\\n' "$sha"
# PADDING
"""


class FakeBucket(http.server.BaseHTTPRequestHandler):
    """Minimal path-style S3: HEAD/GET/PUT, requires SigV4 authorization."""
    objects = {}

    def log_message(self, *args):
        pass

    def authorized(self):
        auth = self.headers.get("Authorization", "")
        if not auth.startswith("AWS4-HMAC-SHA256 Credential=test-access/") or "x-amz-content-sha256" not in auth:
            self.send_response(403)
            self.end_headers()
            return False
        return True

    def do_HEAD(self):
        if not self.authorized():
            return
        body = self.objects.get(self.path)
        self.send_response(200 if body is not None else 404)
        self.send_header("Content-Length", str(len(body) if body is not None else 0))
        self.end_headers()

    def do_GET(self):
        if not self.authorized():
            return
        body = self.objects.get(self.path)
        self.send_response(200 if body is not None else 404)
        self.send_header("Content-Length", str(len(body or b"")))
        self.end_headers()
        self.wfile.write(body or b"")

    def do_PUT(self):
        if not self.authorized():
            return
        self.objects[self.path] = self.rfile.read(int(self.headers["Content-Length"]))
        self.send_response(200)
        self.send_header("Content-Length", "0")
        self.end_headers()


class RuntimeReleaseTest(unittest.TestCase):
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

    def key(self, name="key.pem"):
        path = self.directory / name
        subprocess.run(["openssl", "genpkey", "-algorithm", "ed25519", "-out", str(path)], check=True,
                       capture_output=True)
        return path, release.public_key_of(path)

    def fake_runtime(self, padding="a", sha_expr="$(sha256sum \"$0\" | cut -d' ' -f1)"):
        path = self.directory / f"zuse-cloud-runtime-{padding}"
        path.write_text(FAKE_RUNTIME.replace("SHA_EXPR", sha_expr).replace("PADDING", padding))
        path.chmod(0o755)
        return path

    def build(self, version="0.26.0+gabc", padding="a"):
        out = self.directory / f"out-{padding}"
        self.run_tool("build-artifact", "--binary", self.fake_runtime(padding), "--version", version,
                      "--arch", "x86_64", "--out", out)
        return out, out / "release.json"

    # -- contract ------------------------------------------------------------

    def test_signing_bytes_are_byte_identical_to_the_rust_fixture(self):
        self.assertEqual(release.signing_bytes(self.fixture["release"]), self.fixture["signingBytes"].encode())
        # Key order comes from the contract, not the input.
        shuffled = dict(reversed(list(self.fixture["release"].items())))
        self.assertEqual(release.signing_bytes(shuffled), self.fixture["signingBytes"].encode())

    def test_public_key_and_domain_match_the_rust_contract(self):
        source = CONTRACT.read_text()
        key = re.search(r'RELEASE_SIGNING_PUBLIC_KEY: &str = "([^"]+)"', source).group(1)
        self.assertEqual(key, release.RELEASE_SIGNING_PUBLIC_KEY)
        self.assertIn('b"zuse-runtime-release-v1\\n"', source)
        storage = (ROOT / "crates/protocol/src/cloud.rs").read_text()
        current = re.search(r'RUST_RUNTIME_STORAGE_FORMAT: &str = "([^"]+)"', storage).group(1)
        self.assertIn(current, release.DEFAULT_STORAGE_FORMATS)

    def test_fixture_signature_verifies_and_every_tamper_is_detected(self):
        data = release.signing_bytes(self.fixture["release"])
        key, signature = self.fixture["testPublicKey"], self.fixture["testSignature"]
        self.assertTrue(release.verify_bytes(data, signature, key))
        for field, value in [("sha256", "0" * 64), ("version", "0.26.1"), ("architecture", "aarch64"),
                             ("storageFormats", ["zuse-rust-engine-v1"])]:
            tampered = {**self.fixture["release"], field: value}
            self.assertFalse(release.verify_bytes(release.signing_bytes(tampered), signature, key), field)
        self.assertFalse(release.verify_bytes(data, signature, release.RELEASE_SIGNING_PUBLIC_KEY))
        self.assertFalse(release.verify_bytes(data, "", key))
        self.assertFalse(release.verify_bytes(data, "not base64!", key))

    def test_release_validation_rejects_values_that_could_encode_differently(self):
        good = self.fixture["release"]
        for bad in [{**good, "sha256": "AB" * 32}, {**good, "version": "1.2.3 "},
                    {**good, "architecture": "arm64"}, {**good, "storageFormats": []},
                    {**good, "storageFormats": ["Zuse"]}, {**good, "extra": 1}]:
            with self.assertRaises(release.ReleaseError):
                release.signing_bytes(bad)

    # -- build / sign / verify -----------------------------------------------

    def test_build_writes_content_addressed_artifact_and_manifest(self):
        out, manifest_path = self.build()
        manifest = json.loads(manifest_path.read_text())
        digest = release.sha256_file(self.fake_runtime("a"))
        self.assertEqual(manifest["release"], {"sha256": digest, "version": "0.26.0+gabc", "architecture": "x86_64",
                                               "storageFormats": ["zuse-rust-engine-v1"]})
        self.assertIsNone(manifest["signature"])
        self.assertEqual(manifest["artifactKey"], f"runtime/x86_64/{digest}.zst")
        self.assertEqual(manifest["compressedSizeBytes"], (out / f"{digest}.zst").stat().st_size)
        self.assertEqual(manifest["capabilities"]["runtimeSha256"], digest)
        self.assertEqual(release.decompressed_sha256(out / f"{digest}.zst"), digest)

    def test_build_rejects_a_receipt_for_another_binary_or_format(self):
        liar = self.fake_runtime("liar", sha_expr="0" * 64)
        result = self.run_tool("build-artifact", "--binary", liar, "--version", "0.26.0", "--arch", "x86_64",
                               "--out", self.directory / "x", okay=False)
        self.assertIn("runtimeSha256 does not match", result.stderr)
        binary = self.fake_runtime()
        result = self.run_tool("build-artifact", "--binary", binary, "--version", "0.26.0", "--arch", "x86_64",
                               "--storage-format", "zuse-rust-engine-v2", "--out", self.directory / "y", okay=False)
        self.assertIn("storage format", result.stderr)

    def test_sign_verify_round_trip_and_tamper_detection(self):
        key, public = self.key()
        _, manifest_path = self.build()
        self.run_tool("verify", "--release", manifest_path, "--public-key", public, okay=False)
        # Refuses a key that is not the expected release key.
        result = self.run_tool("sign", "--release", manifest_path, "--key", key, okay=False)
        self.assertIn("does not match the expected release public key", result.stderr)
        self.run_tool("sign", "--release", manifest_path, "--key", key, "--public-key", public)
        self.run_tool("verify", "--release", manifest_path, "--public-key", public)
        self.run_tool("verify", "--release", manifest_path, okay=False)  # contract key
        signed_bytes = self.run_tool("signing-bytes", "--release", manifest_path).stdout.encode()
        self.assertTrue(signed_bytes.startswith(b"zuse-runtime-release-v1\n{\"sha256\":"))
        manifest = json.loads(manifest_path.read_text())
        manifest["release"]["version"] = "0.26.1"
        manifest_path.write_text(json.dumps(manifest))
        self.run_tool("verify", "--release", manifest_path, "--public-key", public, okay=False)

    # -- R2 ------------------------------------------------------------------

    def test_sigv4_matches_the_published_s3_example(self):
        # AWS S3 SigV4 documentation, "GET Object" example.
        headers = release.sigv4_headers(
            "GET", "https://examplebucket.s3.amazonaws.com/test.txt", {"range": "bytes=0-9"},
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "us-east-1",
            datetime.datetime(2013, 5, 24, tzinfo=datetime.timezone.utc))
        self.assertTrue(headers["authorization"].endswith(
            "SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,"
            " Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"))

    @contextlib.contextmanager
    def bucket(self):
        FakeBucket.objects = {}
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeBucket)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield {"ZUSE_CLOUD_RUNTIME_R2_BUCKET": "runtimes",
                   "ZUSE_CLOUD_RUNTIME_R2_ENDPOINT": f"http://127.0.0.1:{server.server_port}",
                   "ZUSE_CLOUD_RUNTIME_R2_ACCESS_KEY_ID": "test-access",
                   "ZUSE_CLOUD_RUNTIME_R2_SECRET_ACCESS_KEY": "test-secret"}
        finally:
            server.shutdown()
            server.server_close()

    def test_upload_is_signed_only_content_addressed_idempotent_and_downloadable(self):
        key, public = self.key()
        out, manifest_path = self.build()
        with self.bucket() as env:
            self.run_tool("upload", "--release", manifest_path, "--artifact", out, "--public-key", public,
                          okay=False, env=env)  # unsigned
            self.assertEqual(FakeBucket.objects, {})
            self.run_tool("sign", "--release", manifest_path, "--key", key, "--public-key", public)
            first = json.loads(self.run_tool("upload", "--release", manifest_path, "--artifact", out,
                                             "--public-key", public, env=env).stdout)
            digest = json.loads(manifest_path.read_text())["release"]["sha256"]
            self.assertEqual(first, {f"runtime/x86_64/{digest}.zst": "uploaded",
                                     f"runtime/x86_64/{digest}.release.json": "uploaded"})
            self.assertEqual(sorted(FakeBucket.objects), [f"/runtimes/runtime/x86_64/{digest}.release.json",
                                                          f"/runtimes/runtime/x86_64/{digest}.zst"])
            second = json.loads(self.run_tool("upload", "--release", manifest_path, "--artifact", out,
                                              "--public-key", public, env=env).stdout)
            self.assertEqual(set(second.values()), {"exists"})

            binary = self.directory / "old" / "zuse-cloud-runtime"
            self.run_tool("download", "--sha256", digest, "--out", binary, "--public-key", public, env=env)
            self.assertEqual(release.sha256_file(binary), digest)
            self.assertTrue(os.access(binary, os.X_OK))
            # Wrong release key: refused before the artifact is fetched.
            self.run_tool("download", "--sha256", digest, "--out", self.directory / "bad", env=env, okay=False)
            # A swapped artifact under the right key is rejected by digest.
            FakeBucket.objects[f"/runtimes/runtime/x86_64/{digest}.zst"] = subprocess.run(
                ["zstd", "-q", "-c"], input=b"evil", capture_output=True, check=True).stdout
            self.run_tool("download", "--sha256", digest, "--out", self.directory / "evil", "--public-key", public,
                          env=env, okay=False)
            self.assertFalse((self.directory / "evil").exists())
            # Same key, different size: never overwritten.
            self.run_tool("upload", "--release", manifest_path, "--artifact", out, "--public-key", public,
                          env=env, okay=False)

    def test_missing_r2_environment_fails_cleanly(self):
        key, public = self.key()
        out, manifest_path = self.build()
        self.run_tool("sign", "--release", manifest_path, "--key", key, "--public-key", public)
        env = {name: "" for name in release.R2_ENV}
        result = self.run_tool("upload", "--release", manifest_path, "--artifact", out, "--public-key", public,
                               env=env, okay=False)
        self.assertIn("missing environment", result.stderr)

    def test_compat_list_is_well_formed(self):
        compat = json.loads((ROOT / "infra/cloud/runtime-compat.json").read_text())
        self.assertIsInstance(compat["releases"], list)
        self.assertLessEqual(len(compat["releases"]), 3)
        for entry in compat["releases"]:
            self.assertRegex(entry["sha256"], r"\A[0-9a-f]{64}\Z")
            self.assertIn(entry["architecture"], release.ARCHITECTURES)
            self.assertRegex(entry["version"], release.VERSION.pattern)


if __name__ == "__main__":
    unittest.main()
