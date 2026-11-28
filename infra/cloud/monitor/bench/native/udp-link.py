"""Route packets between TUN endpoints with an upstream-style server-egress queue.

Run as root on Linux with Python 3.12+. A settings file contains "lan",
"10/40" or "5/60". The existing named namespace holds the benchmark client.
"""

import argparse
import collections
import fcntl
import os
import selectors
import struct
import subprocess
import time
from pathlib import Path


def tun(name):
    fd = os.open("/dev/net/tun", os.O_RDWR | os.O_NONBLOCK)
    try:
        fcntl.ioctl(fd, 0x400454CA, struct.pack("16sH", name.encode(), 0x1001))
    except BaseException:
        os.close(fd)
        raise
    return fd


def forward(server, client, settings):
    pending = collections.deque()
    available = next_read = 0.0
    rate = delay = 0
    with selectors.DefaultSelector() as poll:
        for fd in (server, client):
            poll.register(fd, selectors.EVENT_READ)
        while True:
            now = time.monotonic()
            if now >= next_read:
                profile = settings.read_text().strip()
                if profile == "lan":
                    rate = delay = 0
                else:
                    mbps, rtt = map(int, profile.split("/"))
                    if mbps <= 0 or rtt < 0:
                        raise ValueError("invalid link profile")
                    rate, delay = mbps * 1_000_000, rtt / 1000
                next_read = now + 0.2
            while pending and pending[0][0] <= now:
                os.write(client, pending.popleft()[1])
            timeout = min(0.2, max(0, pending[0][0] - now)) if pending else 0.2
            for key, _ in poll.select(timeout):
                data = os.read(key.fd, 65536)
                if key.fd == client:
                    os.write(server, data)
                elif rate == 0:
                    os.write(client, data)
                else:
                    available = max(time.monotonic(), available) + len(data) * 8 / rate
                    pending.append((available + delay, data))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("namespace")
    parser.add_argument("settings", type=Path)
    args = parser.parse_args()
    server = tun("poofhost")
    original = os.open("/proc/self/ns/net", os.O_RDONLY)
    namespace = os.open(f"/run/netns/{args.namespace}", os.O_RDONLY)
    try:
        os.setns(namespace, 0)
        client = tun("poofclient")
    finally:
        os.setns(original, 0)
        os.close(original)
        os.close(namespace)
    try:

        def ip(*args):
            subprocess.run(["ip", *args], check=True)

        ip("addr", "add", "10.244.9.1/30", "dev", "poofhost")
        ip("link", "set", "poofhost", "up")
        inside = ("-n", args.namespace)
        ip(*inside, "link", "set", "lo", "up")
        ip(*inside, "addr", "add", "10.244.9.2/30", "dev", "poofclient")
        ip(*inside, "link", "set", "poofclient", "up")
        ip(*inside, "route", "add", "default", "via", "10.244.9.1")
        forward(server, client, args.settings)
    finally:
        os.close(server)
        os.close(client)


if __name__ == "__main__":
    main()
