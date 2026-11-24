"""Decode poof frames and sample the upstream benchmark's 40-bit pixel marker."""

import argparse
import ctypes
import json
import os
import statistics
import struct
import time
import zlib
from pathlib import Path

import av
import lz4.block
import numpy as np
from websockets.sync.client import connect


def native_painter():
    path = os.environ.get(
        "POOF_FRAMEBUFFER_LIB", Path(__file__).with_name("libpoof_framebuffer.so")
    )
    paint = ctypes.CDLL(str(path)).poof_apply_patch
    paint.argtypes = [
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_size_t,
        ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    paint.restype = ctypes.c_bool
    return paint


def unpack_patch(data):
    (size,) = struct.unpack_from("<I", data)
    deflated = size & (1 << 31)
    size &= 0x7FFFFFFF
    if not 2 <= size <= 1024 * 1024:
        raise ValueError("invalid patch size")
    if deflated:
        raw = zlib.decompressobj().decompress(data[4:], size + 1)
        if len(raw) != size:
            raise ValueError("invalid decompressed length")
        return raw
    return lz4.block.decompress(data[4:], uncompressed_size=size)


def indexed_frame(data):
    w, h, fmt = struct.unpack_from("<HHH", data)
    n = fmt & 0x7FFF
    if (w, h) not in ((1920, 1080), (2560, 1600)) or not 1 <= n <= 256:
        raise ValueError("invalid indexed desktop")
    palette = np.frombuffer(data, dtype=np.uint32, count=n, offset=6)
    dictionary = bool(fmt & 0x8000)
    limit = 1024 * 1024 if dictionary else w * h
    raw = zlib.decompressobj().decompress(data[6 + n * 4 :], limit + 1)
    if len(raw) > limit:
        raise ValueError("oversized desktop")
    indices = np.frombuffer(raw, dtype=np.uint8, offset=h * 2 if dictionary else 0)
    rows = palette[indices].reshape(-1, w)
    if dictionary:
        refs = np.frombuffer(raw, dtype="<u2", count=h)
        rows = rows[refs]
    return rows.view(np.uint8).reshape(h, w, 4)


def sample(args):
    paint = native_painter()
    frame = None
    codec = av.CodecContext.create("h264", "r")
    codec.thread_type = "SLICE"
    xs = args.x + (np.arange(40) % 20) * 24 + 12
    ys = args.y + (np.arange(40) // 20) * 24 + 12
    ages, rates = [], []
    bucket = set()
    previous = None
    byte_count = 0
    protocols = ["zuse.monitor.v1", "bearer." + os.environ["ZUSE_DISPLAY_PASSWORD"]]
    with connect(
        f"ws://{args.host}/v2/live",
        subprotocols=protocols,
        compression=None,
        max_size=8 << 20,
    ) as ws:
        start = time.monotonic() + 4
        window = start
        while time.monotonic() < start + args.seconds:
            message = ws.recv(timeout=10)
            (flags,) = struct.unpack_from("<I", message)
            data = message[12:]
            if flags >> 30 == 3:
                frame = indexed_frame(data)
            elif flags & (1 << 30):
                raw = unpack_patch(data)
                if frame is None or not paint(
                    frame.ctypes.data, frame.shape[1], frame.shape[0], raw, len(raw)
                ):
                    raise ValueError("invalid patch")
            else:
                decoded = codec.decode(av.Packet(data))
                if not decoded:
                    continue
                frame = np.ascontiguousarray(decoded[-1].to_ndarray(format="rgba"))
            now = time.monotonic()
            if now < start:
                continue
            byte_count += len(message)
            bits = np.packbits(frame[ys, xs, 0] > 127, bitorder="little")
            marker = int.from_bytes(bits.tobytes(), "little")
            seq, stamp = marker >> 24, marker & 0xFFFFFF
            if seq != previous:
                previous = seq
                age = (int(time.time() * 1000) - stamp) & 0xFFFFFF
                if age < 10000:
                    ages.append(age)
                    bucket.add(seq)
            if now - window >= 1:
                rates.append(len(bucket) / (now - window))
                bucket.clear()
                window = now
    if not rates or not ages:
        raise RuntimeError("no valid marker samples")
    ages.sort()
    return {
        "mode": "poof",
        "uniqueFps": {
            "avg": round(statistics.mean(rates), 1),
            "min": round(min(rates), 1),
        },
        "latencyMs": {
            f"p{q}": ages[min(len(ages) - 1, len(ages) * q // 100)] for q in (50, 99)
        },
        "mbps": round(byte_count * 8 / (now - start) / 1e6, 2),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("host")
    parser.add_argument("seconds", type=int)
    parser.add_argument("x", type=int)
    parser.add_argument("y", type=int)
    print(json.dumps(sample(parser.parse_args())))
