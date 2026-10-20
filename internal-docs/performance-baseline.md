# Native performance baseline

Measured 2026-10-20. These are local engineering measurements, not comparative
product benchmarks. No agents, paid model calls, populated chat histories or
embedded browser previews were active.

## macOS release build

Built the default release profile on an Apple M4 Pro with 48 GiB RAM, running
macOS 26.6.2. Incremental compilation was disabled; the workspace release profile
uses thin LTO and strips symbols.

The arm64 native executable is **149.7 MiB** (157.0 MB decimal).
Gzip level 6 compresses that executable to **51.1 MiB**. This is not a complete
installer: the current distribution packager also includes helper executables,
browser assets and legal notices. Build cache size is unrelated to installed
executable size.

An initial ten-second-warmup measurement caught a headless process-tree RSS
spike of 521.1 MiB and a desktop spike of 383.5 MiB while startup children were
still active. The engine later settled near 41 MiB. These transient children
are included in the sampled process-tree totals; no agent turns were started.

The settled runs used 45 seconds of warmup followed by thirty one-second
process-tree samples:

| Measurement | Headless engine | Desktop with embedded engine |
| --- | ---: | ---: |
| Median resident memory (RSS) | 40.8 MiB | 144.8 MiB |
| Peak sampled RSS | 40.8 MiB | 145.4 MiB |
| Mean sampled idle CPU (one core = 100%) | 0.064% | 0.159% |
| Maximum app process count | 1 | 1 |
| IPC socket readiness | 8.04 s | 0.344 s |

These are single runs on the same Mac, not statistically established startup
figures. Earlier runs observed 10.07 s headless IPC readiness and 0.604 s desktop
readiness. The cause of this headless startup delay has not been investigated.
Neither socket readiness result establishes first-frame latency.

The CPU sampler accumulates nonnegative per-process deltas so that retiring
startup children cannot subtract CPU already consumed. Processes that start
and exit between samples may be missed. RSS is not macOS Activity Monitor's
physical-footprint metric and does not capture all GPU allocations.

## Linux development build

The existing x86-64 executable was built on October 18 with the local
unoptimized development configuration. SHA-256:
`ca9714059ae9bd8d6bdf2cc35eeb55dc6e5285822c4d4da71824fc881571726f`.
It predates the latest source changes. Desktop measurement used Xvfb and software
Vulkan in the cloud sandbox, not a physical GPU.

Each run used a disposable fresh profile, ten seconds of warmup and thirty
one-second process-tree samples. CPU percentages use one logical core as 100%.

| Measurement | Headless engine | Desktop with embedded engine |
| --- | ---: | ---: |
| Median resident memory (RSS) | 69.2 MiB | 250.7 MiB |
| Peak sampled RSS | 69.4 MiB | 250.8 MiB |
| Median proportional memory (PSS) | 39.0 MiB | 229.1 MiB |
| Mean idle CPU | 0.10% | 0.30% |
| IPC socket readiness | 51 ms | 51 ms |
| Maximum app process count | 1 | 1 |

IPC readiness measures when the local control socket accepts a connection. It
does not measure first frame, provider readiness or perceived startup time.
RSS counts resident shared pages fully; PSS apportions them between processes.
The sampled peak can miss brief allocation spikes between samples.

The executable occupies 340.6 MiB. Stripping a separate copy produces 234.9 MiB;
gzip level 6 compresses that copy to 68.1 MiB. These are executable sizes, not a
release installer or a complete package. Optional browser assets in this
checkout occupy another 43.4 MiB; system libraries and external agents are not
included in executable sizes.

## Reproducing

```sh
python3 scripts/benchmark-native.py --binary target/release/zuse \
  --mode headless --warmup 45 --output .context/benchmark-headless.json
python3 scripts/benchmark-native.py --binary target/release/zuse \
  --mode desktop --warmup 45 --output .context/benchmark-desktop.json
```

The desktop mode needs a graphical session. The script launches its own app
with an isolated temporary profile and stops it afterward. It does not modify
the normal user profile. JSON includes all samples, binary path/mtime and OS.
Run on an otherwise idle machine; avoid concurrent compilation when comparing
results. The headless and desktop modes each start their own embedded engine;
the development watcher's separate engine/UI arrangement is a different case.

## Unmeasured

Battery power, physical GPU use, first frame, sustained streaming, loaded
histories, many active agents and their descendant processes still need
measurements. Idle CPU alone does not establish battery efficiency. Agent count
claims must include the selected providers' processes and tools, not only the
desktop shell.
