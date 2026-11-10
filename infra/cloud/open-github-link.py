#!/usr/bin/env python3
"""Explicit human-only helper: pipe stdout over SSH directly to the Mac's open.

The private opaque browser transaction never enters agent logs. This script does
not open a browser, select an installation, or start until a human invokes it.
"""
import argparse
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("github_canary", Path(__file__).with_name("staging-github-canary.py"))
canary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(canary)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir",type=Path,default=Path(__file__).resolve().parent)
    parser.add_argument("--owner",default="user_01KW7R9WGJFFSKDNESE7RN00N1")
    parser.add_argument("--add-installation",action="store_true",help="After completion, start a new explicit installation selection using the verified identity")
    args=parser.parse_args()
    print(canary.Canary(args.state_dir,args.owner).run("open_installation_link" if args.add_installation else "open_link"))

if __name__ == "__main__":
    try:
        main()
    except (canary.device.Failure,OSError,ValueError,KeyError):
        raise SystemExit(json.dumps({"state":"failed","code":"private_native_github_link_failed"}))
