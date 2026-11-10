#!/usr/bin/env python3
"""Private owner canary using the same WorkOS device flow as engine/account.rs.

Only the original public staging client is supported. Device grants and tokens
remain in a mode0700 private directory outside the checkout. Standard output
contains safe status and, after server verification, the public account ID.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

CLIENT_ID = "client_01KW6ZEZKVMZ0G429A89XZD83Q"
IDENTITY = "https://api.workos.com"
CLOUD_API = "https://api-rust-stag.zuse.sh"


class Failure(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


class Transport:
    def __init__(self):
        self.opener = urllib.request.build_opener(NoRedirect)

    def request(self, method, url, *, form=None, body=None, token=None):
        headers = {"Accept": "application/json", "User-Agent": "Zuse-staging-owner-canary"}
        data = None
        if form is not None:
            data = urllib.parse.urlencode(form).encode()
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        if token is not None:
            headers["Authorization"] = "Bearer " + token
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            response = self.opener.open(request, timeout=15)
        except urllib.error.HTTPError as response:
            payload = response.read(1024 * 1024 + 1)
            return response.code, self.decode(payload)
        except (OSError, urllib.error.URLError):
            raise Failure("transport_unavailable") from None
        with response:
            return response.status, self.decode(response.read(1024 * 1024 + 1))

    @staticmethod
    def decode(payload):
        if len(payload) > 1024 * 1024:
            raise Failure("response_too_large")
        try:
            value = json.loads(payload)
        except (ValueError, UnicodeDecodeError):
            raise Failure("invalid_response") from None
        if not isinstance(value, dict):
            raise Failure("invalid_response")
        return value


def private_directory(path):
    path = Path(path).expanduser().absolute()
    # Reject a normal repository-local profile, including .context, even when
    # the caller explicitly chooses it. Conductor may sync checkout files.
    script = Path(__file__).resolve()
    checkout = next((parent for parent in script.parents if (parent / "Cargo.toml").exists()), None)
    if checkout and path.resolve().is_relative_to(checkout):
        raise Failure("private_profile_must_be_outside_checkout")
    if path.is_symlink():
        raise Failure("private_directory_required")
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    metadata = path.stat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) & 0o077:
        raise Failure("private_directory_required")
    return path


def read_private(path):
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return None
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) & 0o077:
        raise Failure("private_file_required")
    if metadata.st_size > 1024 * 1024:
        raise Failure("private_state_too_large")
    try:
        return json.loads(path.read_text())
    except (ValueError, UnicodeDecodeError):
        raise Failure("private_state_invalid") from None


def write_private(path, value, *, text=False):
    if path.is_symlink():
        raise Failure("private_file_required")
    temporary = path.with_name("." + path.name + "." + uuid.uuid4().hex)
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, "w") as output:
            output.write(value if text else json.dumps(value, separators=(",", ":")))
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if temporary.exists():
            temporary.unlink()


class Flow:
    def __init__(self, directory, transport=None, clock=None):
        self.root = private_directory(directory)
        self.http = transport or Transport()
        self.clock = clock or time.time

    def now(self):
        return int(self.clock())

    def status(self):
        session = read_private(self.root / "session.json")
        if session:
            if session.get("clientId") != CLIENT_ID:
                raise Failure("identity_environment_changed")
            state = "verified" if session.get("verified") else "awaiting_api_verification"
            if session["expiresAt"] <= self.now():
                state = "expired"
            return {"state": state, "refreshUncertain": bool(session.get("refreshUncertain")),
                    "ownerId": session["subject"] if session.get("verified") else None,
                    "accessExpiresAt": session["expiresAt"], "clientId": CLIENT_ID}
        grant = read_private(self.root / "grant.json")
        if grant:
            return {"state": "expired" if grant["expiresAt"] <= self.now() else "awaiting_browser",
                    "expiresAt": grant["expiresAt"], "intervalSeconds": grant["interval"], "nextPollAt": grant["nextPollAt"],
                    "approvalUrlPath": str(self.root / "approval-url"), "clientId": CLIENT_ID}
        return {"state": "not_started", "clientId": CLIENT_ID}

    def start(self):
        if read_private(self.root / "session.json"):
            return self.status()
        old = read_private(self.root / "grant.json")
        if old and old["expiresAt"] > self.now():
            return self.status()
        status, grant = self.http.request("POST", IDENTITY + "/user_management/authorize/device", form={"client_id": CLIENT_ID})
        if status < 200 or status >= 300:
            raise Failure("device_authorization_unavailable")
        expires = grant.get("expires_in")
        interval = grant.get("interval")
        uri = grant.get("verification_uri_complete")
        if (not isinstance(expires, int) or isinstance(expires, bool) or not 0 < expires <= 3600
                or not isinstance(interval, int) or isinstance(interval, bool) or interval <= 0
                or not isinstance(uri, str) or len(uri) > 16384 or any(ord(c) < 32 for c in uri)
                or not isinstance(grant.get("device_code"), str) or not grant["device_code"]
                or not isinstance(grant.get("user_code"), str) or not grant["user_code"]):
            raise Failure("device_authorization_invalid")
        url = urllib.parse.urlsplit(uri)
        if url.scheme != "https" or not url.hostname or url.username or url.password:
            raise Failure("device_authorization_invalid")
        saved = {"deviceCode": grant["device_code"], "userCode": grant["user_code"], "verificationUri": uri,
                 "clientId": CLIENT_ID, "expiresAt": self.now() + expires, "interval": min(60, interval),
                 "nextPollAt": self.now() + min(60, interval)}
        write_private(self.root / "grant.json", saved)
        write_private(self.root / "approval-url", uri + "\n", text=True)
        return self.status()

    def save_tokens(self, tokens, expected=None):
        token = tokens.get("access_token")
        refresh = tokens.get("refresh_token")
        user = tokens.get("user")
        subject = user.get("id") if isinstance(user, dict) else None
        if not isinstance(token, str) or len(token) > 16384 or not isinstance(refresh, str) or not refresh or not isinstance(subject, str):
            raise Failure("authentication_response_invalid")
        try:
            part = token.split(".")[1]
            claims = json.loads(base64.urlsafe_b64decode(part + "=" * (-len(part) % 4)))
        except (ValueError, IndexError, UnicodeDecodeError):
            raise Failure("authentication_response_invalid") from None
        if not isinstance(claims, dict):
            raise Failure("authentication_response_invalid")
        expiry = claims.get("exp")
        if (not re.fullmatch(r"user_[A-Za-z0-9]+", subject) or claims.get("sub") != subject
                or claims.get("client_id") != CLIENT_ID or not isinstance(expiry, int) or expiry <= self.now()
                or (expected is not None and expected != subject)):
            raise Failure("authentication_identity_changed")
        # Persist rotated credentials before a dependent API request. A temporary
        # API outage must not discard a one-use refresh/device response.
        write_private(self.root / "session.json", {"version": 1, "clientId": CLIENT_ID, "subject": subject,
                      "accessToken": token, "refreshToken": refresh, "expiresAt": expiry, "verified": False})
        for name in ("grant.json", "approval-url"):
            path = self.root / name
            if path.exists():
                path.unlink()
        return self.verify()

    def verify(self):
        session = read_private(self.root / "session.json")
        if not session or session.get("clientId") != CLIENT_ID:
            raise Failure("staging_session_required")
        if session["expiresAt"] <= self.now():
            raise Failure("access_token_expired_refresh_required")
        status, context = self.http.request("GET", CLOUD_API + "/v1/cloud/context", token=session["accessToken"])
        # The deployed Rust API verifies WorkOS signature, issuer, client_id,
        # expiry and subject using the existing WorkosIdentity implementation.
        if status != 200 or context.get("actor", {}).get("accountId") != session["subject"] or context.get("ownerId") != session["subject"]:
            raise Failure("staging_api_identity_verification_failed")
        session["verified"] = True
        write_private(self.root / "session.json", session)
        return self.status()

    def poll(self, once=False):
        while True:
            if read_private(self.root / "session.json"):
                return self.verify()
            grant = read_private(self.root / "grant.json")
            if not grant or grant.get("clientId") != CLIENT_ID:
                raise Failure("pending_staging_authorization_required")
            if grant["expiresAt"] <= self.now():
                raise Failure("device_authorization_expired")
            delay = max(0, grant["nextPollAt"] - self.now())
            if delay:
                if once:
                    return self.status()
                time.sleep(min(delay, 60))
                continue
            status, body = self.http.request("POST", IDENTITY + "/user_management/authenticate", form={
                "client_id": CLIENT_ID, "grant_type": "urn:ietf:params:oauth:grant-type:device_code", "device_code": grant["deviceCode"]})
            if 200 <= status < 300:
                return self.save_tokens(body)
            code = body.get("error")
            if code == "slow_down":
                grant["interval"] = min(60, grant["interval"] + 5)
            elif code not in ("authorization_pending",):
                if code in ("access_denied", "expired_token"):
                    for name in ("grant.json", "approval-url"):
                        path = self.root / name
                        if path.exists():
                            path.unlink()
                raise Failure("device_authorization_" + (code if code in ("access_denied", "expired_token") else "unavailable"))
            grant["nextPollAt"] = self.now() + grant["interval"]
            write_private(self.root / "grant.json", grant)
            if once:
                return self.status()

    def refresh(self):
        session = read_private(self.root / "session.json")
        if not session or session.get("clientId") != CLIENT_ID:
            raise Failure("staging_session_required")
        if session.get("refreshUncertain"):
            raise Failure("refresh_response_uncertain_new_login_required")
        session["refreshUncertain"] = True
        write_private(self.root / "session.json", session)
        status, tokens = self.http.request("POST", IDENTITY + "/user_management/authenticate", body={
            "client_id": CLIENT_ID, "grant_type": "refresh_token", "refresh_token": session["refreshToken"]})
        if not 200 <= status < 300:
            raise Failure("staging_refresh_rejected")
        return self.save_tokens(tokens, expected=session["subject"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("action", choices=("start", "poll", "status", "verify", "refresh"))
    parser.add_argument("--once", action="store_true")
    parser.add_argument("--open-browser", action="store_true")
    args = parser.parse_args()
    if args.open_browser and (args.action != "start" or sys.platform != "darwin"):
        raise Failure("browser_open_requires_mac_start")
    flow = Flow(args.state_dir)
    result = flow.poll(args.once) if args.action == "poll" else getattr(flow, args.action)()
    if args.open_browser and result["state"] == "awaiting_browser":
        uri = (flow.root / "approval-url").read_text().strip().replace("\\", "\\\\").replace('"', '\\"')
        subprocess.run(["osascript"], input='open location "' + uri + '"\n', text=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (Failure, OSError, ValueError, subprocess.CalledProcessError) as error:
        code = str(error) if isinstance(error, Failure) else "private_canary_operation_failed"
        raise SystemExit(json.dumps({"state": "failed", "code": code}))
