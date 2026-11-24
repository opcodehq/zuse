"""Sample the X server, streamer and their descendants while a viewer is connected."""

import argparse
import os
import time
from pathlib import Path


def counters(roots):
    processes = {}
    for path in Path("/proc").glob("[0-9]*/stat"):
        try:
            fields = path.read_text().rsplit(")", 1)[1].split()
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
        processes[int(path.parent.name)] = (
            int(fields[1]),
            int(fields[11]) + int(fields[12]),
        )
    selected = set(roots)
    while True:
        children = {pid for pid, (parent, _) in processes.items() if parent in selected}
        if children <= selected:
            return {pid: processes[pid][1] for pid in selected if pid in processes}
        selected.update(children)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", nargs="+", type=int, help="server and X server PIDs")
    parser.add_argument("--delay", type=float, default=3)
    parser.add_argument("--seconds", type=float, default=7)
    args = parser.parse_args()
    time.sleep(args.delay)
    start = time.monotonic()
    before = counters(args.pid)
    time.sleep(args.seconds)
    after = counters(args.pid)
    elapsed = time.monotonic() - start
    if before.keys() != after.keys() or not set(args.pid) <= after.keys():
        parser.exit(1, "Process membership changed during the sample\n")
    ticks = sum(after[p] - before[p] for p in before)
    print(f"{ticks / os.sysconf('SC_CLK_TCK') / elapsed:.3f}")


if __name__ == "__main__":
    main()
