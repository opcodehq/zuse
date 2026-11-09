#!/usr/bin/env python3
"""Read-only verification of a privately installed GitHub App signing key.

Print public App identity only. Never prints the PEM, signed JWT, response errors,
installation tokens or subprocess diagnostics. Does not alter the App or its keys.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.request


class CheckError(Exception):
    pass


def encoded(value):
    return base64.urlsafe_b64encode(value).rstrip(b"=")


def app_identity(key_file, app_id):
    path = Path(key_file)
    if not path.is_absolute() or not str(app_id).isdigit():
        raise CheckError("An absolute key path and numeric GitHub App ID are required")
    try:
        details = path.lstat()
    except OSError:
        raise CheckError("Signing key file is not accessible") from None
    if not stat.S_ISREG(details.st_mode) or details.st_mode & 0o077:
        raise CheckError("Signing key must be a regular private file with mode 600 or 400")
    if details.st_uid != os.geteuid() or not 1 <= details.st_size <= 65536:
        raise CheckError("Signing key ownership or size is invalid")
    now = int(time.time())
    header = encoded(json.dumps({"alg": "RS256", "typ": "JWT"}, separators=(",", ":")).encode())
    claims = encoded(json.dumps({"iss": str(app_id), "iat": now - 60, "exp": now + 480}, separators=(",", ":")).encode())
    signing_input = header + b"." + claims
    try:
        signed = subprocess.run(["openssl", "dgst", "-sha256", "-sign", str(path)],
                                input=signing_input, capture_output=True, timeout=10, check=False)
    except (OSError, subprocess.TimeoutExpired):
        raise CheckError("Signing key verification could not run") from None
    if signed.returncode != 0 or not signed.stdout:
        raise CheckError("File is not a usable GitHub RSA signing key")
    jwt = (signing_input + b"." + encoded(signed.stdout)).decode("ascii")
    request = urllib.request.Request("https://api.github.com/app", headers={
        "Authorization": "Bearer " + jwt,
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "zuse-github-app-check",
    })
    # Do not forward the JWT if GitHub or an intermediary returns a redirect.
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            return None
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=15) as response:
            payload = response.read(65537)
        if len(payload) > 65536:
            raise CheckError("GitHub returned an oversized App identity response")
        result = json.loads(payload)
    except urllib.error.HTTPError as error:
        if error.code == 401:
            raise CheckError("GitHub rejected this signing key and App ID combination") from None
        raise CheckError(f"GitHub App identity request failed with HTTP {error.code}") from None
    except (OSError, ValueError, TimeoutError, urllib.error.URLError):
        raise CheckError("GitHub App identity request failed") from None
    if not isinstance(result, dict) or str(result.get("id")) != str(app_id) or not isinstance(result.get("slug"), str):
        raise CheckError("GitHub returned an unexpected App identity")
    return {"verified": True, "appId": str(result["id"]), "slug": result["slug"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--key-file", required=True)
    parser.add_argument("--app-id", required=True)
    args = parser.parse_args()
    try:
        result = app_identity(args.key_file, args.app_id)
    except CheckError as error:
        print(str(error), file=sys.stderr)
        return 1
    print(json.dumps(result, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
