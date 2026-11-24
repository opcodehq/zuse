# poof vs voxl 0.4.0

This page records the earlier two-CPU affinity experiment. The newer Boxd report covers a real two-vCPU guest, patched voxl, and the other self-hosted servers in Tembo’s comparison. [Boxd measurements](native/RESULTS.md)

On this VM, the independent poof encoder delivered 57.8 and 58.1 distinct scrolling frames/s at 30 and 60 ms RTT. The corresponding voxl medians were 56.4 and 57.1. Individual runs overlap. The clearest advantage was lower tail latency. Median frame age at 30 ms RTT favored voxl.

| Guest load | RTT | poof / voxl distinct FPS | poof / voxl median frame age | poof / voxl p99 frame age |
|---|---:|---:|---:|---:|
| Quiet | 30 ms | 57.8 / 56.4 | 40 / 37 ms | 50 / 57 ms |
| Quiet | 60 ms | 58.1 / 57.1 | 55 / 57 ms | 61 / 82 ms |
| Two-job build | 30 ms | 52.3 / 53.8 | 43 / 51 ms | 57 / 76 ms |
| Two-job build | 60 ms | 50.9 / 47.7 | 56 / 81 ms | 84 / 120 ms |

The quiet rows are medians of three independent ten-second measurements per engine per RTT. Each build row has one measurement. Raw observations are in [matched-runs.csv](matched-runs.csv). The marker measures elapsed time from drawing in the X11 application to reading the decoded pixels on the browser's animation callback. This covers capture, encoding, transport and rendering into the canvas. Physical panel scanout and Internet routing are outside this test.

## Conditions

Both isolated desktops used Xvfb at 1920×1080, Debian Chromium 156.0.8078.79, the same load-generator binary and the same browser measurement script. The synthetic 60 Hz source scrolls a document behind a moving box and writes its timestamp and frame ID into the pixels. The viewer, X server, producer, encoder and link shaper share CPUs 0 and 1. The link shaper allows 30 Mbit/s with equal delay in both directions. Each run starts a fresh viewer, warms up for two seconds and samples for ten. The confirmation used the Rust service’s loopback HTTP transport. The confirmatory order was voxl/poof, poof/voxl, voxl/poof with both RTTs per turn. The build runs used a new Cargo target directory for each measurement, two compiler jobs on the same two CPUs and verified the build was still active after sampling. The build load used the monitor crate before its public WebSocket transport added dependencies. Rebuilding the current crate reproduces two-job compiler contention with a different compilation mix. No physical GPU or 120 Hz display was measured. The two encoding pipelines and image quality differ; visual quality has not been scored.

poof captures on XDamage events, compares alternating shared-memory images in Rust, emits LZ4 scroll patches and falls back to H.264. The direct viewer applies up to 7 ms of pacing during continuous motion. In the exploratory 30 ms series the old timer-based pipeline delivered a median 46.2 distinct FPS. A capture-on-damage video-only experiment delivered 30.2 in one run. The final configuration was frozen before the head-to-head runs above.

## Public demo route

The same 1080p pixel-marker test also ran through the public HTTPS demo URL. The original HTTP response stream delivered 10.6 and 7.7 distinct FPS in two exploratory runs. The Rust server now sends one binary WebSocket message per encoded frame over HTTPS. Three fresh viewers measured **58.5, 59.3 and 58.6 distinct FPS**. Their median frame ages were 22, 24 and 22 ms. The Browser and source ran on the same VM and traversed the actual external tunnel. The live guest and tunnel had access to additional CPUs, and the network was unshaped. These are end-to-end route checks, separate from the two-CPU competitor comparison. A remote Mac can have different network latency and display timing.

## Application input and animation

A separate poof test types 20 times, scrolls 20 times, then animates a CSS background for five seconds. Each input is issued through the agent CLI and timed until the remote pixels change. Each cell below comes from one run. The guest browser's own animation callback documents the rate of the source. The viewer counter includes recovery frames. CPU covers the capture worker and the bitrate measures application payload. The guest, viewer and build shared CPUs 4 and 5.

| Load | RTT | First frame | Type / wheel p95 | Guest rAF / viewer FPS | Gaps >50 ms / max | Encoder cores | kbit/s |
|---|---:|---:|---:|---:|---:|---:|---:|
| Quiet | 30 ms | 245 ms | 102 / 260 ms | 19.2 / 19.4 | 67 / 76 ms | 0.11 | 1346 |
| Quiet | 60 ms | 360 ms | 103 / 285 ms | 17.5 / 17.9 | 54 / 114 ms | 0.10 | 1330 |
| Two-job build | 30 ms | 317 ms | 106 / 337 ms | 15.5 / 15.9 | 62 / 133 ms | 0.08 | 1043 |
| Two-job build | 60 ms | 305 ms | 112 / 275 ms | 17.9 / 18.4 | 63 / 89 ms | 0.09 | 1280 |

## Reproduce

Use two disposable Linux desktops with the same Chromium binary, X11 display geometry, Node, Playwright, CPU affinity and memory. Start the poof image as described in [README.md](../README.md). In the other desktop install voxl 0.4.0 separately and start it read-only on loopback port 6901 with 60 FPS, clipboard and audio disabled, and file transfers set to none. Supply its configured credential in a local file. Pin the X server and already-running streamer threads with `taskset -apc 0,1 PID`, or start each whole guest container with `--cpuset-cpus=0,1`. Run only one measured engine at a time.

The comparison uses the [upstream benchmark fixtures](https://github.com/tembo/voxl/tree/21f194f9e446e7073abd7bbd52e0b397c3800b42/bench) in the benchmark environment. The poof runtime image contains its own server, encoder and viewer. With the guest DISPLAY and XAUTHORITY exported, run:

```sh
git clone https://github.com/tembo/voxl /tmp/voxl-reference
git -C /tmp/voxl-reference checkout --detach 21f194f9e446e7073abd7bbd52e0b397c3800b42
npm install --prefix /tmp/monitor-tools playwright@1.65.1
export BENCH_ROOT=/tmp/voxl-reference
export PLAYWRIGHT_NODE_MODULES=/tmp/monitor-tools/node_modules/
export CHROMIUM=/usr/bin/chromium
taskset -c 0,1 bash ./bench/compare.sh zuse 30 60
# On the reference guest, set MONITOR_PASSWORD_FILE to its credential file:
taskset -c 0,1 bash ./bench/compare.sh voxl 30 60
```

The script builds the common workload with a C compiler and X11/Xext headers. Set `MONITOR_LOADGEN_BIN` to the same prebuilt executable on both guests to use an image without a compiler. Repeat the pair three times, reversing engine order on the second round. For build contention, start the following on the same VM immediately before each individual RTT, verify the compiler stayed active throughout the measurement, and wait for it before the next run:

```sh
CARGO_TARGET_DIR="$(mktemp -d)" taskset -c 0,1 cargo build --release --locked --offline -j2 \
  --manifest-path ./stream/Cargo.toml
```

For the application benchmark, run `bench/bench.sh` from the repository root. To verify the Rust encoder's pixel reconstruction independently, run:

```sh
rustc --edition=2021 --test -O ./stream/scroll_delta.rs -o /tmp/zuse-patch-tests
/tmp/zuse-patch-tests
```
