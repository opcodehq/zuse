#!/usr/bin/env python3
"""Owner-only native GitHub canary; private WorkOS tokens never leave Hetzner.

Uses the sibling staging-owner-device.py public authentication helper. All owner
session refresh and API requests are serialized with the shared operation.lock.
"""
import argparse
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import time
import urllib.parse

spec = importlib.util.spec_from_file_location("owner_device", Path(__file__).with_name("staging-owner-device.py"))
device = importlib.util.module_from_spec(spec)
spec.loader.exec_module(device)


class Canary:
    def __init__(self, directory, expected_owner, transport=None, clock=None):
        if not re.fullmatch(r"user_[A-Za-z0-9]+", expected_owner):
            raise device.Failure("verified_owner_id_required")
        self.flow = device.Flow(directory, transport, clock)
        self.expected = expected_owner
        self.root = self.flow.root

    def credential(self):
        session = device.read_private(self.root / "session.json")
        if not session or session.get("subject") != self.expected or session.get("clientId") != device.CLIENT_ID:
            raise device.Failure("verified_staging_owner_required")
        if session.get("expiresAt", 0) <= self.flow.now() + 45:
            self.flow.refresh()
        elif not session.get("verified"):
            self.flow.verify()
        session = device.read_private(self.root / "session.json")
        if not session.get("verified") or session.get("subject") != self.expected:
            raise device.Failure("verified_staging_owner_required")
        return session["accessToken"]

    def request(self, method, path, body=None):
        token = self.credential()
        status, data = self.flow.http.request(method, device.CLOUD_API + path, token=token, body=body)
        if status < 200 or status >= 300:
            raise device.Failure("native_github_api_unavailable")
        return data

    def check(self):
        context = self.request("GET", "/v1/cloud/context")
        if context.get("actor", {}).get("accountId") != self.expected or context.get("ownerId") != self.expected:
            raise device.Failure("staging_owner_scope_changed")
        catalog = self.request("GET", "/v1/cloud/github")
        if catalog.get("nativeAuthority") is not True:
            raise device.Failure("native_github_rollout_not_active")
        return {"state": "native_ready", "ownerId": self.expected,
                "connected": catalog.get("user") is not None,
                "installationCount": len(catalog.get("installations", [])),
                "repositoryCount": len(catalog.get("repositories", []))}

    def begin(self, *, add_installation=False):
        self.check()
        previous = device.read_private(self.root / "github-operation.json")
        if previous and previous.get("ownerId") == self.expected and previous.get("expiresAt", 0) > self.flow.now() * 1000:
            current = self.status()
            if current["state"] not in ("complete", "cancelled", "failed", "expired") or (current["state"] == "complete" and not add_installation):
                return current
        operation = self.request("POST", "/v1/cloud/github/install", {})
        url = operation.get("url")
        identifier = operation.get("operationId")
        expiry = operation.get("expiresAt")
        if not isinstance(url, str) or not isinstance(identifier, str) or not re.fullmatch(r"[0-9a-f-]{36}", identifier) or not isinstance(expiry, int):
            raise device.Failure("native_github_operation_invalid")
        self.validate_browser_url(url)
        device.write_private(self.root / "github-browser-url", url + "\n", text=True)
        device.write_private(self.root / "github-operation.json", {"operationId": identifier, "ownerId": self.expected, "expiresAt": expiry})
        return {"state": "awaiting_browser", "ownerId": self.expected, "operationId": identifier,
                "expiresAt": expiry, "browserUrlPath": str(self.root / "github-browser-url")}

    @staticmethod
    def validate_browser_url(url):
        parsed = urllib.parse.urlsplit(url)
        if (parsed.scheme != "https" or parsed.netloc != "api-rust-stag.zuse.sh"
                or parsed.path != "/v1/cloud/github/connect" or parsed.username or parsed.password or parsed.fragment):
            raise device.Failure("native_github_operation_origin_invalid")
        state = urllib.parse.parse_qs(parsed.query).get("state")
        if not state or len(state) != 1 or not re.fullmatch(r"[0-9a-f]{64}", state[0]):
            raise device.Failure("native_github_operation_invalid")

    def open_link(self, *, add_installation=False):
        # Explicit human invocation starts a new selection after completion;
        # a still-pending transaction keeps its original state/cookie/expiry.
        result = self.begin(add_installation=add_installation)
        if result["state"] not in ("awaiting_browser", "created", "authorizing", "choosing"):
            raise device.Failure("native_github_browser_not_ready")
        path = self.root / "github-browser-url"
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        try:
            metadata = os.fstat(fd)
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077 or metadata.st_size > 4096:
                raise device.Failure("private_native_github_url_required")
            url = os.read(fd, 4097).decode("utf-8").strip()
        finally:
            os.close(fd)
        self.validate_browser_url(url)
        operation = device.read_private(self.root / "github-operation.json")
        if not operation or operation.get("operationId") != result["operationId"] or operation.get("expiresAt", 0) <= self.flow.now()*1000:
            raise device.Failure("native_github_operation_changed")
        return url

    def open_installation_link(self):
        return self.open_link(add_installation=True)

    def status(self):
        operation = device.read_private(self.root / "github-operation.json")
        if not operation or operation.get("ownerId") != self.expected:
            raise device.Failure("native_github_operation_required")
        query = urllib.parse.urlencode({"operationId": operation["operationId"]})
        observed = self.request("GET", "/v1/cloud/github/status?" + query)
        state = observed.get("state")
        if state not in ("created", "authorizing", "exchanging", "choosing", "complete", "cancelled", "failed", "expired"):
            raise device.Failure("native_github_operation_invalid")
        if state == "complete":
            result = self.check()
            result["state"] = "complete"
            result["operationId"] = operation["operationId"]
            return result
        return {"state": state, "ownerId": self.expected, "operationId": operation["operationId"], "expiresAt": operation["expiresAt"]}

    def run(self, action):
        lock = self.root / "operation.lock"
        descriptor = os.open(lock, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        try:
            metadata = os.fstat(descriptor)
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid():
                raise device.Failure("private_operation_lock_required")
            os.fchmod(descriptor, 0o600)
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            return getattr(self, action)()
        finally:
            os.close(descriptor)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--owner", required=True)
    parser.add_argument("action", choices=("check", "begin", "status"))
    args = parser.parse_args()
    print(json.dumps(Canary(args.state_dir, args.owner).run(args.action), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (device.Failure, OSError, ValueError, KeyError):
        raise SystemExit(json.dumps({"state": "failed", "code": "private_native_github_canary_failed"}))
