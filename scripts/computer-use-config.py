#!/usr/bin/env python3
"""Print an opt-in Opcode CU configuration for the native Zuse harness."""

import argparse
import json
import os
from pathlib import Path
import sys


DESKTOP_ENVIRONMENT = ("DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS")


def configuration(executable, trusted=False, forward_env=()):
    path = Path(executable).expanduser()
    if not path.is_absolute() or not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError("--cu must name an installed, executable absolute path")
    if any(name not in DESKTOP_ENVIRONMENT for name in forward_env):
        raise ValueError("Only explicit desktop environment references are supported")
    return {
        "servers": {
            "opcode-cu": {
                "trusted": trusted,
                "protocol": "2025-06-18",
                "readOnlyTools": [],
                "connection": {
                    "type": "stdio",
                    "executable": str(path),
                    "args": ["mcp"],
                    "env_refs": {name: name for name in sorted(set(forward_env))},
                },
            }
        }
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cu", required=True, help="Absolute path to an already installed CU CLI")
    parser.add_argument(
        "--trust-desktop", action="store_true",
        help="Trust this process with the engine user's desktop and OS access",
    )
    parser.add_argument(
        "--forward-env", action="append", default=[], choices=DESKTOP_ENVIRONMENT,
        help="Forward a named desktop variable from the engine environment, never its value",
    )
    args = parser.parse_args()
    try:
        value = configuration(args.cu, args.trust_desktop, args.forward_env)
    except ValueError as error:
        parser.error(str(error))
    print("Preview only: merge the server into your trusted profile's harness/extensions.json. "
          "No configuration was written and no process was started.", file=sys.stderr)
    print(json.dumps(value, indent=2))


if __name__ == "__main__":
    main()
