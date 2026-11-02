#!/usr/bin/env python3
"""Read-only checks against the actual localhost Rust API, without credentials."""
import argparse
import json
from urllib.error import HTTPError
from urllib.request import Request, urlopen

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--managed", action="store_true", help="Also require the managed-computer authority routes")
args = parser.parse_args()

origin = "http://127.0.0.1:28700"
checks = [("/healthz", 200, "GET"), ("/readyz", 200, "GET"), ("/v1/cloud/workspaces", 401, "GET")]
if args.managed:
    checks += [("/v1/environments/local-check/connect", 401, "POST")]
    checks += [("/v1/environments", 401, "GET"), ("/v1/client/dpop-token", 401, "POST"), ("/v1/mobile/devices", 401, "POST")]
for path, expected, method in checks:
    try:
        request = Request(origin + path, data=b"{}" if method == "POST" else None, method=method, headers={"Content-Type": "application/json"})
        with urlopen(request, timeout=10) as response:
            status, body = response.status, response.read()
    except HTTPError as response:
        status, body = response.code, response.read()
    assert status == expected, f"{path}: expected {expected}, got {status}"
    json.loads(body)
    print(f"PASS {path}: HTTP {status}")
print("Readiness and unauthenticated rejection verified; signed-in flows were not exercised.")
