#!/usr/bin/env python3
"""Report upstream candidates; never install, promote or change the stable pin."""
import argparse
import datetime
import json
from pathlib import Path
import re
import urllib.parse
import urllib.request

PACKAGES = {"codex": "@openai/codex", "claude": "@anthropic-ai/claude-code", "grok": "@xai-official/grok"}


def candidates():
    result = {}
    for harness, package in PACKAGES.items():
        encoded = urllib.parse.quote(package, safe="")
        request = urllib.request.Request(f"https://registry.npmjs.org/{encoded}/latest",
                                         headers={"User-Agent": "zuse-harness-release-check/1"})
        with urllib.request.urlopen(request, timeout=20) as response:
            data = response.read(1024 * 1024 + 1)
        if len(data) > 1024 * 1024:
            raise ValueError("upstream metadata too large")
        metadata = json.loads(data)
        version = metadata["version"]
        if metadata["name"] != package or not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][a-zA-Z0-9.-]+)?", version):
            raise ValueError("upstream package identity/version mismatch")
        result[harness] = {"package": package, "version": version,
                           "npmIntegrity": metadata["dist"]["integrity"]}
    return {"schema": 1, "observedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "status": "candidate-only", "harnesses": result}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = candidates()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
