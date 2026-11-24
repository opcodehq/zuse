# Desktop streaming on one Boxd machine

Two AMD EPYC vCPUs, 8 GiB RAM, Ubuntu 24.04, software encoding. The X server, source application, server, network emulator and client shared the same guest. The producer drew a 40-bit frame ID and timestamp. Each native client sampled the decoded pixels.

Across three runs per native workload, poof had lower p99 draw-to-decode latency and used less bandwidth than patched voxl in all seven cells. The clearest gain came in 1080p animation at 5 Mbps / 60 ms RTT. Median p99 fell from 138 to 50 ms. Median worst-second throughput rose from 51.8 to 59.4 distinct FPS.

Frame-rate results at the 60 Hz source ceiling overlapped. Patched voxl had the higher worst-second median for LAN 1080p animation and for Chromium scrolling at 5 Mbps. Median scroll latency on that slow link was 74 ms for both. Connected idle CPU also favored voxl. The separate CSS workload produced approximately 20 frames per second at its source. The measurements support using poof for CPU-only remote desktops on this tier. Testing human interaction on an actual 120 Hz client remains open.

The original poof worker sent 7.93 Mbit/s during LAN scrolling. The release worker used 0.79 Mbit/s on the same workload after limiting moves to the damaged region and encoding distant edits separately. Recovery images can reuse duplicate rows. We also tested longer sleeps with XInput motion notifications. In an isolated pointer-only test, five idle cursor warps took about 110 ms with that prototype and 9–16 ms on the shipping path. The release keeps the shorter cursor poll.

## Native results

Each cell gives **average FPS (slowest one-second bucket) · p50/p99 milliseconds**. A dash indicates no valid measurement. For poof and patched voxl, each number is the median across independent runs. The CSV retains each trial and prototype. Near 60 FPS, a fraction of a frame per second is within the resolution of the one-second buckets.

### Chromium scrolling

| Engine | LAN | 10 Mbps / 40 ms | 5 Mbps / 60 ms |
|---|---:|---:|---:|
| poof | 60.0 (59.8) · 40/48 | 60.0 (59.6) · 32/40 | 60.0 (57.7) · 74/82 |
| voxl | 60.0 (59.0) · 46/53 | 60.0 (59.4) · 72/80 | 60.0 (59.4) · 74/93 |
| Selkies with picom | 22.0 (10.9) · 97/387 | 22.2 (10.5) · 124/322 | 20.8 (12.1) · 141/390 |
| Selkies without picom | 60.1 (59.8) · 47/51 | 60.1 (59.5) · 56/68 | 60.0 (58.5) · 86/111 |
| KasmVNC | 50.9 (47.9) · 64/104 | 3.0 (2.6) · 473/793 | 1.3 (1.1) · 940/1470 |
| Neko | 22.2 (18.6) · 161/254 | 22.3 (16.7) · 194/564 | 19.5 (7.6) · 299/1258 |
| Neko tuned | 48.4 (42.8) · 102/143 | 46.5 (42.7) · 132/323 | Client timeout or exit |
| Sunshine | 32.5 (28.3) · 72/97 | 32.6 (29.2) · 131/163 | 35.9 (33.5) · 160/177 |
| Sunshine tuned | 36.7 (31.5) · 91/116 | 37.6 (30.0) · 122/147 | 37.4 (30.6) · 157/183 |
| noVNC | 12.8 (12.3) · 114/177 | 3.9 (3.2) · 242/4188 | 2.3 (1.7) · 441/4208 |

### Animation

| Engine | LAN | 10 Mbps / 40 ms | 5 Mbps / 60 ms | 2560×1600 LAN |
|---|---:|---:|---:|---:|
| poof | 60.0 (59.6) · 9/11 | 60.0 (59.7) · 32/37 | 60.0 (59.4) · 44/50 | 60.0 (59.8) · 7/10 |
| voxl | 60.1 (59.9) · 14/24 | 60.0 (57.9) · 45/72 | 59.4 (51.8) · 60/138 | 60.0 (58.6) · 18/44 |
| Selkies with picom | 22.5 (11.6) · 67/219 | 24.3 (10.7) · 109/364 | 16.0 (10.9) · 145/314 | 9.7 (5.9) · 66/291 |
| Selkies without picom | 60.1 (59.8) · 15/21 | 60.1 (52.2) · 52/178 | 60.1 (54.5) · 69/246 | 59.8 (56.7) · 24/47 |
| KasmVNC | 48.5 (43.1) · 59/92 | 40.9 (36.2) · 84/114 | 28.0 (21.7) · 111/176 | 23.2 (19.7) · 80/138 |
| Neko | 14.4 (0.0) · 251/3830 | 22.1 (13.8) · 238/648 | 13.9 (2.5) · 5811/9064 | 7.4 (6.3) · 423/599* |
| Neko tuned | 49.6 (46.5) · 65/135 | Client timeout or exit | Client timeout or exit | 45.9 (41.4) · 75/128 |
| Sunshine | 30.9 (26.1) · 64/85 | 38.2 (34.9) · 106/123 | 36.6 (34.4) · 125/138 | 29.0 (24.9) · 61/92 |
| Sunshine tuned | 36.1 (33.5) · 49/74 | 40.6 (38.0) · 103/121 | 35.2 (31.7) · 125/147 | 36.7 (32.7) · 55/84 |
| noVNC | 11.1 (10.2) · 96.5/134 | 7.3 (6.4) · 99/158 | 6.3 (5.7) · 112/181 | 8.1 (6.7) · 124/4153 |

* The default Neko full-screen client received 31.31 Mbit/s on LAN but produced no fresh timestamp. This cell shows a separate 15-second run with a 4 MiB socket receive buffer. The tuned Neko confirmation also used 4 MiB. The standard aiortc receive setting was 256 KiB, and the isolated client kernel recorded UDP receive-buffer drops. Both Neko attempts are retained in the CSV. The tuned Neko client also exited before returning a sample in three bandwidth-limited tests. Those attempts appear in the trial data.

## Bandwidth and CPU

Payload Mbit/s for poof and patched voxl. The upstream clients use protocol-specific byte counters. Cross-protocol figures can cover different amounts of framing.

| Workload | Link | poof | voxl | poof / voxl trials |
|---|---|---:|---:|---:|
| scroll | lan | 0.79 | 2.29 | 3 / 3 |
| scroll | 10/40 | 0.79 | 2.29 | 3 / 3 |
| scroll | 5/60 | 0.79 | 2.08 | 3 / 3 |
| anim1080 | lan | 1.57 | 5.78 | 3 / 3 |
| anim1080 | 10/40 | 1.57 | 4.79 | 3 / 3 |
| anim1080 | 5/60 | 1.57 | 3.34 | 3 / 3 |
| animfull | lan | 1.87 | 3.23 | 3 / 3 |

Median CPU cores for the X server, streamer and capture children over seven seconds on LAN. The source, native decoder, separate picom process and network emulator are excluded. The process count is retained with each sample.

| Engine | Connected idle | 1080p animation | Chromium scroll | Full-screen animation |
|---|---:|---:|---:|---:|
| poof | 0.011 | 0.442 | 0.264 | 0.331 |
| voxl | 0.001 | 0.714 | 0.523 | 0.941 |
| Selkies with picom | 1.026 | 1.375 | 1.378 | 1.672 |
| Selkies without picom | — | — | 0.504 | 1.122 |
| KasmVNC | 0.000 | 0.784 | 0.749 | 0.817 |
| Neko | 0.736 | 1.434 | 1.250 | 1.238 |
| Neko tuned | 1.344 | 1.550 | 1.260 | 1.476 |
| Sunshine | 1.547 | 1.474 | 1.333 | 1.462 |
| Sunshine tuned | 1.598 | 1.550 | 1.351 | 1.544 |
| noVNC | 0.007 | 0.411 | 0.235 | 0.405 |

## Method

poof and voxl shared the same 2560×1600 Xvfb with shared framebuffer access enabled. voxl logged direct framebuffer capture. It used the upstream 60 FPS, ultrafast, 30 Mbit/s settings, plus the six upstream x264 patches. Each steady-state measurement used the same raster encoding and decoding algorithms as the shipped build. A subsequent listener change removed the startup polling delay; the Boxd browser measurements used that change. The source was either Chromium scrolling the upstream document, the upstream 1920×1080 animation in a window at (300,150), or a 2560×1600 animation at (0,0). For Chromium we normalized the timestamp cells to 24 physical pixels at scale 1 and found their location on X11 before each run.

The head-to-head alternated engine order. poof went through regional, palette and compression experiments during the initial runs. Only the release algorithm appears in its summary. We filled remaining release repetitions on the same guest. The poof reader applied every patch to a complete RGBA framebuffer. H.264 went through FFmpeg. The upstream protocol-specific clients decoded the alternatives. Native latency ends at decode. Browser presentation is a separate measurement.

Four seconds of warm-up preceded each native sampling window. poof and patched voxl used 20-second windows. Several competitor runs used 10-second windows. See the duration column before comparing worst-second performance. TCP used the upstream bottleneck proxy, with half the RTT each way. UDP used a routed TUN pair with server-egress serialization and full-RTT egress delay. Both emulate a deep queue with no configured packet loss.

KasmVNC 1.4.0, x11vnc 0.9.16 / noVNC 1.3.0, Sunshine 2025.628.4510 and Moonlight Embedded 2.7 with the upstream dump patch. Selkies image digest `sha256:fb75f28bad031dcd21980509965b2f6f573ce7734d1c890ef1c1c37a99211187`. Neko image digest `sha256:587a3389b6b99c8999eb00a768c79205a74647c57cf5b9695a90234a0edaa446`. Neko used its VP8 default and the upstream tuned H.264 pipeline. In direct-ICE runs the client disabled external STUN and waited for its first decoded frame before warm-up. The preliminary ICE negotiation failures are identified in the data.

The published Tembo table used 16 vCPUs, a best-of-three selection for voxl, and a separate hosted Daytona sandbox. This experiment used one two-vCPU guest and includes Daytona’s self-hostable x11vnc/noVNC stack. The hosted Daytona preview route, GPU capture, visual quality scoring and physical panel scanout were outside this experiment. [Upstream measurements](https://github.com/tembo/voxl/blob/21f194f9e446e7073abd7bbd52e0b397c3800b42/README.md)

[Reproduction instructions](README.md) · [Native trials](trials.csv) · [CPU samples](cpu.csv)

## Tembo’s published reference

Tembo reported the following results on its 16-vCPU guest. The matched two-vCPU measurements above use the same workload definitions and take precedence for the direct comparison. [Pinned upstream table](https://github.com/tembo/voxl/blob/21f194f9e446e7073abd7bbd52e0b397c3800b42/README.md)

| Source | Link | voxl FPS (worst second) | p50/p99 ms |
|---|---|---:|---:|
| Chromium scroll | LAN | 60.0 (58.1) | 37/50 |
| Chromium scroll | 10/40 | 60.1 (59.6) | 80/90 |
| Chromium scroll | 5/60 | 60.0 (59.2) | 98/115 |
| 1080p animation | LAN | 60.0 (59.8) | 13/18 |
| 1080p animation | 10/40 | 60.0 (58.1) | 45/72 |
| 1080p animation | 5/60 | 60.0 (59.4) | 60/84 |
| 2560×1600 animation | LAN | 60.0 (59.4) | 16/23 |

## Browser presentation at 1920×1080

The same headless Chromium client sampled the visible canvas on each animation callback. The producer filled the 1920×1080 X server. Each cell is one ten-second run after two seconds of warm-up. The compile condition ran a clean two-job Moonlight build on the same vCPUs. Single-run variation is visible in the raw results. [Browser samples](browser.csv)

| Link and CPU load | poof FPS (worst) | voxl FPS (worst) | poof p50/p99 ms | voxl p50/p99 ms |
|---|---:|---:|---:|---:|
| LAN | 59.9 (58.8) | 59.4 (56.4) | 18/22 | 34/43 |
| 10 Mbps / 40 ms | 59.5 (59) | 54.5 (50.2) | 37/42 | 55/90 |
| 5 Mbps / 60 ms | 60.1 (60) | 47.5 (45) | 56/57 | 76/142 |
| 5 Mbps / 60 ms, compiling | 58.7 (57.1) | 46.1 (39.9) | 58/63 | 84/150 |

The agent issued 20 keystrokes and 20 wheel events through the real CLI. Each latency ends when the browser sees the corresponding color change. A separate five-second CSS animation measures the guest’s own frame rate and viewer stalls.

| Link | First frame ms | Type p95 ms | Wheel p95 ms | Guest Hz | Viewer FPS | Gaps >50 ms | Worst gap ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| 10 Mbps / 40 ms | 214 | 121 | 249 | 20.4 | 21 | 53 | 69 |
| 5 Mbps / 60 ms | 312 | 150 | 264 | 19.3 | 19.8 | 53 | 108 |

The interactive acceptance test opened a local application on the Boxd desktop, clicked a control and typed a unique value. After the viewer reconnected, a fresh 1920×1080 screenshot showed the same value, `Zuse-945542f5`.
