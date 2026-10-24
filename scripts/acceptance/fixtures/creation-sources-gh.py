#!/usr/bin/env python3
"""Isolated GitHub responses; checkout still operates on a real Git worktree."""
import json
import os
from pathlib import Path
import subprocess
import sys

root = Path(os.environ["ZUSE_CREATION_SOURCE_FIXTURE"])
args = sys.argv[1:]
with (root / "github-calls.jsonl").open("a") as calls:
    calls.write(json.dumps({"args": args, "cwd": os.getcwd()}) + "\n")
if args[:2] == ["pr", "list"]:
    failure = root / "fail-pr-list"
    if failure.exists():
        failure.unlink()
        sys.exit("Could not reach GitHub. Try again.")
    print(json.dumps([
        {"number": 72, "title": "Improve editor shortcuts", "headRefName": "feature/from-pr", "author": {"login": "alice"}},
        {"number": 84, "title": "Fix fork default branch", "headRefName": "main", "author": {"login": "bob"}},
    ]))
elif args[:2] == ["pr", "view"]:
    fork = args[2] == "84"
    print(json.dumps({"headRefName": "main" if fork else "feature/from-pr", "baseRefName": "main", "isCrossRepository": fork, "headRepositoryOwner": {"login": "bob"}, "headRepository": {"defaultBranchRef": {"name": "main"}}}))
elif args[:2] == ["pr", "checkout"]:
    branch = args[args.index("--branch") + 1]
    result = subprocess.run(["git", "switch", "-c", branch, "refs/heads/pr-fixture"], capture_output=True)
    if result.returncode:
        sys.stderr.buffer.write(result.stderr)
        sys.exit(result.returncode)
elif args[:2] == ["issue", "list"]:
    print(json.dumps([
        {"number": 93, "title": "Keep composer draft when selecting a source", "author": {"login": "alice"}},
        {"number": 94, "title": "Improve project search", "author": {"login": "bob"}},
    ]))
elif args[:2] == ["issue", "view"]:
    number = int(args[2])
    print(json.dumps({"number": number, "title": "Keep composer draft when selecting a source", "body": "Preserve the text and attach this issue as Markdown.\n\n- Keep the primary checkout unchanged.", "url": f"https://github.com/example/project/issues/{number}"}))
else:
    sys.exit("This isolated account supports source-picker fixture commands only.")
