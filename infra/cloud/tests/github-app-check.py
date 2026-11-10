#!/usr/bin/env python3
"""Verify the App preflight signs correctly and never returns secret diagnostics."""
import base64
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

source = Path(__file__).resolve().parents[1] / "github-app-check.py"
spec = importlib.util.spec_from_file_location("app_check", source)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class AppCheckTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.key = Path(self.directory.name) / "app.pem"
        subprocess.run(["openssl", "genrsa", "-out", str(self.key), "2048"], check=True, capture_output=True)
        self.key.chmod(0o600)

    def test_real_rsa_signature_and_public_identity(self):
        public = Path(self.directory.name) / "public.pem"
        subprocess.run(["openssl", "rsa", "-in", str(self.key), "-pubout", "-out", str(public)], check=True, capture_output=True)
        test = self
        class Fixture:
            def open(self, request, timeout):
                test.assertEqual(request.full_url, "https://api.github.com/app")
                jwt = request.get_header("Authorization").removeprefix("Bearer ")
                header, claims, signature = jwt.split(".")
                decoded = json.loads(base64.urlsafe_b64decode(claims + "=" * (-len(claims) % 4)))
                test.assertEqual(decoded["iss"], "123")
                test.assertLessEqual(decoded["exp"] - decoded["iat"], 600)
                sig = Path(test.directory.name) / "signature"
                sig.write_bytes(base64.urlsafe_b64decode(signature + "=" * (-len(signature) % 4)))
                verified = subprocess.run(["openssl", "dgst", "-sha256", "-verify", str(public), "-signature", str(sig)], input=(header+"."+claims).encode(), capture_output=True)
                test.assertEqual(verified.returncode, 0)
                return io.BytesIO(json.dumps({"id":123,"slug":"zuse-test","client_secret":"must-not-return"}).encode())
        with patch.object(check.urllib.request, "build_opener", return_value=Fixture()):
            self.assertEqual(check.app_identity(self.key, "123"), {"verified":True,"appId":"123","slug":"zuse-test"})

    def test_wrong_app_and_vendor_diagnostics_are_private(self):
        class Fixture:
            def open(self, *_args, **_kwargs):
                raise urllib.error.HTTPError("https://api.github.com/app",401,"secret-vendor-details",{},io.BytesIO(b"secret-response"))
        with patch.object(check.urllib.request, "build_opener", return_value=Fixture()):
            with self.assertRaises(check.CheckError) as error:
                check.app_identity(self.key, "123")
        self.assertEqual(str(error.exception), "GitHub rejected this signing key and App ID combination")

    def test_private_file_and_no_symlink_required_before_network(self):
        with patch.object(check.urllib.request, "build_opener") as network:
            self.key.chmod(0o644)
            with self.assertRaises(check.CheckError): check.app_identity(self.key,"123")
            self.key.chmod(0o600)
            link = self.key.with_suffix(".link")
            link.symlink_to(self.key)
            with self.assertRaises(check.CheckError): check.app_identity(link,"123")
            network.assert_not_called()

    def test_redirects_cannot_forward_signed_jwt(self):
        class Fixture:
            def open(self, *_args, **_kwargs):
                raise urllib.error.HTTPError("https://api.github.com/app",302,"redirect",{},None)
        with patch.object(check.urllib.request, "build_opener", return_value=Fixture()) as build:
            with self.assertRaisesRegex(check.CheckError,"HTTP 302"):
                check.app_identity(self.key,"123")
            handler = build.call_args.args[0]
            self.assertIsNone(handler.redirect_request(None,None,302,"redirect",{},"https://elsewhere.example"))


if __name__ == "__main__":
    unittest.main()
