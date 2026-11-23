# Live cloud monitor experiment

The agent controls one 1920×1080 X11 desktop. An Applications bar launches Chromium, Thunar, Mousepad and XTerm. Viewers share one stream of H.264 frames and lossless scroll patches and can reconnect while applications keep running. The guest user owns the applications and their files. The guest CLI works from the existing agent shell.

## Run on one Linux machine

Build the standalone image. Host networking makes the guest's loopback listener reachable on the Linux host. Use a machine where port 6901 is free.

```sh
docker build -t zuse-monitor-mvp infra/cloud/monitor
docker run -d --name monitor --network host --shm-size=256m zuse-monitor-mvp
docker exec monitor zuse-monitor password
```

Open `http://127.0.0.1:6901/` and enter the viewer password shown in your terminal. A remote viewer can use an authenticated SSH local forward. The server refuses viewer input, clipboard and file transfer. Keep the password private.

```sh
docker exec -d monitor zuse-monitor launch chromium --app=about:blank
docker exec monitor zuse-monitor screenshot /tmp/before.png
docker cp monitor:/tmp/before.png ./before.png
docker exec monitor zuse-monitor click 400 300
docker exec monitor zuse-monitor type 'Hello from the agent'
docker exec monitor zuse-monitor key Return
docker exec monitor zuse-monitor scroll down
```

Chromium requires a working browser sandbox. If the container runtime blocks its sandbox, use a guest VM or an X11 application installed in the image for the smoke test. The automated Playwright fixture uses an isolated local page.

## Open from Zuse desktop

Build with `--build-arg GUEST_IMAGE=<prepared Debian bookworm Zuse guest>` to keep the real guest's entrypoint. The derived image adds the graphical stack. Launch the desktop app with `ZUSE_EXPERIMENTAL_MONITOR=1`, open a running cloud conversation, then select **Computer → Open Monitor**. **Close Monitor** disconnects the viewer. An agent can independently run `zuse-monitor launch <application> [arguments...]` to start the same display, including from shells that filter environment variables.

The app prepares the selected computer's SSH key and pinned host key through its existing authenticated cloud connection. SSH binds an ephemeral local port, forwards it to guest loopback port 6901, and invokes `zuse-monitor attach`. The guest returns its random viewer credential over SSH. The app opens the local viewer in an isolated browser context. Closing it drops SSH. The guest holds the display open until its supervisor or machine stops. A cold restart starts a fresh graphical session.

## Streaming implementation

The Rust service starts one capture worker when viewers connect. XDamage wakes the worker after a paint, and a short poll also catches cursor motion. The C adapter alternates between two MIT-SHM images. The Rust delta encoder compares a new image against its predecessor, estimates vertical translation and emits the changed scanline spans. The worker compresses these spans with LZ4. For changes outside the patch budget it uses libyuv and x264. A recovery keyframe at least once per second lets late and lagging viewers resume. The browser applies pixel patches to a canvas and decodes H.264 via WebCodecs. During continuous motion it allows up to 7 ms for arrival jitter. The first frame after idle is drawn immediately. FFmpeg handles on-demand PNG screenshots. HTTPS viewers use authenticated binary WebSockets so a proxy can forward each frame immediately. The embedded loopback viewer uses an authenticated HTTP stream through the SSH forward. Screenshots require the same guest credential.

The Zuse image builds from this directory. The only use of voxl in this experiment is as a separately installed benchmark competitor and a source of common measurement fixtures. The native H.264 fallback uses x264. Its licensing still applies to distribution.

## Reproduce the checks

Run these in an isolated 1080p guest with the monitor running. Use Node 22+, Playwright, Chromium, a C compiler and X11/Xext headers. The benchmark needs a spare loopback port 7902. Input automation operates the guest desktop.

```sh
npm install --prefix /tmp/monitor-tools playwright@1.65.1
export PLAYWRIGHT_NODE_MODULES=/tmp/monitor-tools/node_modules/
export CHROMIUM=/usr/bin/chromium
export DISPLAY=:91 XAUTHORITY="$HOME/.local/state/zuse-monitor/Xauthority"
zuse-monitor start
node infra/cloud/monitor/smoke.mjs /tmp/agent.png /tmp/viewer.png
bash infra/cloud/monitor/bench.sh
```

The smoke test compares guest input with the read-only viewer, exercises viewer reconnect, and captures both views. Set `MONITOR_VIEWER_PORT` to exercise a local SSH forward. On Linux, pipe `zuse-monitor password` to `python3 infra/cloud/monitor/webkit-probe.py /path/to/zuse-webkit <forwarded-port>` from a separate desktop display to test the real Zuse WebKit helper.

The benchmark builds its own X11 scrolling document. The viewer samples its binary frame counter at animation-frame boundaries and counts distinct IDs in ten seconds. A local TCP proxy applies a 30 Mbit/s limit and 30 or 60 ms RTT. A separate browser workload measures input-to-visible latency by checking a known color transition. CPU is capture worker process time divided by wall time. Run in an otherwise isolated guest with one capture worker.

## Matched comparison

See [the head-to-head benchmark](BENCHMARK.md) for the common workload, repeated runs, build contention and reproduction instructions. The following sections document earlier prototypes.

## Native capture results

With capture, the scrolling application and Chromium viewer sharing the same two logical CPUs, the previous Zuse FFmpeg-based path delivered 24.9 distinct FPS at 1080p and 30 ms RTT. The new scrolling path delivered 52.9 on the same test. At 60 ms RTT it delivered 58.7. Each run sampled ten seconds. A separate CSS animation produced 18.3 and 19.3 viewer updates/s at 30 and 60 ms RTT respectively. In the input test at 30 ms RTT, first frame was 283 ms and p95 typing-to-visible latency was 122 ms. These figures measure the desktop the viewer actually drew. The historical comparison below used a different scrolling pattern.

## Historical voxl baseline on the current VM

One run per condition, Debian guest image, stock x264, CPU encoding, 1920×1080. Guest display, encoder, test generator and Chromium viewer shared two logical CPUs by affinity. The contended run added a cold two-job Rust build pinned to the same CPUs. The link shaper capped throughput at 30 Mbit/s. The input timing starts before the agent CLI runs. Stalls, CPU and bandwidth come from the separate five-second CSS animation workload. CPU covers the streamer only.

| Build load | RTT | Distinct scrolling FPS | First frame (ms) | Type / wheel p95 (ms) | Animation gaps >50 ms / worst (ms) | Encoder cores | Animation kbit/s |
|---|---:|---:|---:|---:|---:|---:|---:|
| Idle | 30ms | 34.3 | 546 | 189 / 366 | 49 / 150 | 0.2 | 424 |
| Idle | 60ms | 29.3 | 769 | 256 / 411 | 45 / 133 | 0.2 | 377 |
| Cargo build | 30ms | 30.2 | 716 | 312 / 430 | 41 / 167 | 0.14 | 349 |
| Cargo build | 60ms | 25.6 | 659 | 231 / 407 | 44 / 150 | 0.17 | 375 |

On the unrestricted eight-CPU VM, scrolling averaged 51.3 and 50.4 distinct FPS at 30 and 60 ms RTT. Under a four-job build those averages fell to 44.3 and 41.4. The 60 FPS target remains open. Profile capture, guest rendering and client presentation on separate machines before choosing a replacement encoder. The experiment did not test 120 Hz.

The standalone image measured 936 MB uncompressed with Chromium installed. A separate smoke test exercised an authenticated SSH forward, verified a 401 for anonymous screenshot access, typed a unique value, reopened the Chrome viewer and observed it again. The compiled Linux Zuse WebKitGTK helper also negotiated H.264 through the forward. Zuse staging and a physical Mac display were outside the local performance measurements.

## Independent streamer results

Before the native capture worker, the Zuse server used FFmpeg/libx264 for video. These numbers describe that earlier implementation.

| Implementation | 1080p distinct scrolling FPS at 30 ms RTT |
|---|---:|
| Original voxl baseline | 34.3 |
| Zuse server with experimental VP8 encoding | 3.1 |
| Zuse server with low-latency H.264 encoding | 19.7 |

These are single exploratory measurements with two CPUs shared by the guest and viewer. The H.264 experiment also reached 19.5 distinct scrolling FPS at 60 ms RTT. Encoder settings and delivered image quality differ. The final build also discards duplicate capture timestamps. Those exploratory runs preceded the matched experiment above.

The standalone image passed a separate read-only viewer, guest input, screenshot and reconnect smoke test. On the live VM, the agent opened the file manager from the launcher, used the editor to save a file, and verified that file on the machine. A fresh external Chromium viewer authenticated over a temporary HTTPS tunnel and decoded the shared desktop. Requests to the external video endpoint without a viewer credential returned 401.
