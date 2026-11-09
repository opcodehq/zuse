#!/usr/bin/env python3
"""Fixture-only WorkOS protocol/private-profile checks; never requests live codes."""
import base64
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("owner_device", Path(__file__).resolve().parents[1] / "staging-owner-device.py")
owner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(owner)


def tokens(now=1000, refresh="fixture-refresh"):
    encode = lambda value: base64.urlsafe_b64encode(json.dumps(value).encode()).rstrip(b"=").decode()
    jwt = encode({"alg": "RS256"}) + "." + encode({"sub": "user_fixture", "client_id": owner.CLIENT_ID, "exp": now + 300}) + ".fixture-signature"
    return {"access_token": jwt, "refresh_token": refresh, "user": {"id": "user_fixture"}}


class FakeTransport:
    def __init__(self):
        self.calls = []
        self.queue = []

    def request(self, method, url, **kwargs):
        self.calls.append((method, url, kwargs))
        response = self.queue.pop(0)
        if isinstance(response, Exception):
            raise response
        return response


class OwnerDeviceTest(unittest.TestCase):
    def test_existing_protocol_resumes_pending_and_never_returns_codes_or_tokens(self):
        with tempfile.TemporaryDirectory() as directory:
            http = FakeTransport()
            now = [1000]
            flow = owner.Flow(directory, http, lambda: now[0])
            http.queue.append((200, {"device_code": "fixture-device", "user_code": "fixture-user-code", "verification_uri_complete": "https://auth.example/device?code=fixture-user-code", "expires_in": 100, "interval": 5}))
            first = flow.start()
            self.assertEqual(flow.start(), first)
            self.assertEqual(len(http.calls), 1)
            self.assertEqual(flow.poll(once=True), first)
            self.assertNotIn("fixture-user-code", json.dumps(first))
            self.assertNotIn("fixture-device", json.dumps(first))
            self.assertEqual((Path(directory) / "grant.json").stat().st_mode & 0o777, 0o600)
            now[0] = 1005
            http.queue.append((400, {"error": "slow_down"}))
            flow.poll(once=True)
            self.assertEqual(owner.read_private(Path(directory) / "grant.json")["interval"], 10)
            now[0] = 1015
            http.queue.extend([(200, tokens()), (200, {"actor": {"accountId": "user_fixture"}, "ownerId": "user_fixture"})])
            result = flow.poll(once=True)
            self.assertEqual(result["state"], "verified")
            self.assertEqual(result["ownerId"], "user_fixture")
            self.assertEqual(http.calls[-1][1], owner.CLOUD_API + "/v1/cloud/context")
            self.assertIn("token", http.calls[-1][2])
            self.assertFalse((Path(directory) / "grant.json").exists())
            self.assertFalse((Path(directory) / "approval-url").exists())
            self.assertNotIn("fixture-signature", json.dumps(result))
            self.assertEqual((Path(directory) / "session.json").stat().st_mode & 0o777, 0o600)

    def test_api_outage_keeps_consumed_credentials_and_never_claims_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            http = FakeTransport()
            flow = owner.Flow(directory, http, lambda: 1000)
            http.queue.append((503, {}))
            with self.assertRaisesRegex(owner.Failure, "staging_api_identity_verification_failed"):
                flow.save_tokens(tokens())
            self.assertEqual(flow.status()["state"], "awaiting_api_verification")
            self.assertIsNone(flow.status()["ownerId"])
            http.queue.append((200, {"actor": {"accountId": "user_fixture"}, "ownerId": "user_fixture"}))
            flow.verify()
            self.assertTrue(all(call[0] == "GET" for call in http.calls))
            http.queue.extend([(200, tokens(refresh="fixture-rotated-refresh")), (503, {})])
            with self.assertRaises(owner.Failure):
                flow.refresh()
            self.assertEqual(owner.read_private(Path(directory) / "session.json")["refreshToken"], "fixture-rotated-refresh")
            self.assertEqual(flow.status()["state"], "awaiting_api_verification")

    def test_profile_fences_environment_symlinks_and_uncertain_refresh(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            public = root / "public"
            public.mkdir(mode=0o755)
            with self.assertRaisesRegex(owner.Failure, "private_directory_required"):
                owner.Flow(public)
            link = root / "link"
            link.symlink_to(root, target_is_directory=True)
            with self.assertRaises(owner.Failure):
                owner.Flow(link)
            http = FakeTransport()
            flow = owner.Flow(root, http, lambda: 1000)
            wrong = tokens()
            wrong["user"]["id"] = "user_different"
            with self.assertRaisesRegex(owner.Failure, "authentication_identity_changed"):
                flow.save_tokens(wrong)
            http.queue.append((200, {"actor": {"accountId": "user_fixture"}, "ownerId": "user_fixture"}))
            flow.save_tokens(tokens())
            http.queue.append(owner.Failure("transport_unavailable"))
            with self.assertRaises(owner.Failure):
                flow.refresh()
            self.assertTrue(flow.status()["refreshUncertain"])
            count = len(http.calls)
            with self.assertRaisesRegex(owner.Failure, "refresh_response_uncertain"):
                flow.refresh()
            self.assertEqual(len(http.calls), count)
        checkout = next(parent for parent in Path(owner.__file__).parents if (parent / "Cargo.toml").exists())
        with self.assertRaisesRegex(owner.Failure, "outside_checkout"):
            owner.Flow(checkout / ".context/forbidden-owner-profile")


if __name__ == "__main__":
    unittest.main()
