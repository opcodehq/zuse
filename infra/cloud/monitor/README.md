# poof

The agent controls one X11 desktop. Its default size is 1920×1080. Set `-e POOF_DISPLAY_SIZE=2560x1600` on the container for a larger monitor. An Applications bar launches Chromium, Thunar, Mousepad and XTerm. Viewers can disconnect and return to the same running applications. The Rust streamer sends changed regions as lossless patches and uses indexed images or H.264 when a viewer needs a full frame. The guest user owns the applications and their files. The guest CLI works from the existing agent shell.

On a two-vCPU Boxd guest at 1080p, Chromium presented 60.1 distinct FPS over a 5 Mbps link with 60 ms RTT. Patched voxl presented 47.5 on the same test. During a two-job build the readings were 58.7 and 46.1 FPS. These are ten-second browser samples. [Full comparison and raw results](bench/native/RESULTS.md)

## Click around from a Mac

With Docker Desktop running, execute these commands from the checkout.

```sh
docker compose up -d --build --wait
docker compose exec -T desktop cat /home/zuse/.local/state/zuse-monitor/vnc-password
open vnc://127.0.0.1:5905
```

Enter the printed password in Screen Sharing. Click a dock icon or type in Mousepad. Screen Sharing uses VNC for this interactive demo and shares poof’s X11 display with the agent. The port is bound to the Mac’s loopback address. Run `docker compose stop` when finished.

## Run on one Linux machine

Build the standalone image. Host networking makes the guest's loopback listener reachable on the Linux host. Use a machine where port 6901 is free.

```sh
docker build -t zuse-monitor-mvp .
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

Enable **Settings → General → Cloud computer**, open a chat, and select **Desktop** from the right sidebar’s **+** menu. A cloud chat uses its own computer. A local chat lets you select an existing cloud computer. The sidebar’s expand button gives Desktop the conversation area.

Zuse prepares SSH with the computer’s pinned host key and runs `zuse-monitor attach-control`. The interactive viewer uses guest loopback port 6912 and a separate credential sent through the SSH session. Port 6901 remains available for read-only clients. Closing the tab disconnects the viewer. Applications continue running on the machine until the machine stops. Older guest images need the interactive monitor installed.

## Streaming implementation

The Rust service shares a capture worker among viewers of each protocol version and stops it when the last viewer disconnects. XDamage wakes the worker after a paint. The capture adapter alternates between two MIT-SHM images. The Rust encoder locates the changed rectangle, estimates its vertical movement, and sends replacement pixels for the parts that differ. Patches use LZ4, with DEFLATE available for larger updates. Desktops with up to 256 colors can use a lossless indexed recovery image. Repeated scanlines share one row in its dictionary. Richer images fall back to libyuv and x264.

A viewer requests a recovery frame when it connects or falls behind. An idle desktop sends a heartbeat every five seconds. The legacy routes keep periodic keyframes for existing readers. The WebCodecs viewer applies each patch in order on the same canvas. During continuous motion it allows up to 7 ms for arrival jitter. The first frame after idle is drawn immediately. FFmpeg handles on-demand PNG screenshots. HTTPS viewers use authenticated binary WebSockets so a proxy can forward each frame immediately. The embedded loopback viewer uses an authenticated HTTP stream through the SSH forward. Screenshots require the same guest credential.

The poof image builds from this directory. The only use of voxl in this experiment is as a separately installed benchmark competitor and a source of common measurement fixtures. The native H.264 fallback uses x264. Its licensing still applies to distribution.

## Reproduce the checks

Run these in an isolated 1080p guest with the monitor running. Use Node 22+, Playwright, Chromium, a C compiler and X11/Xext headers. The benchmark needs a spare loopback port 7902. Input automation operates the guest desktop.

```sh
npm install --prefix /tmp/monitor-tools playwright@1.65.1
export PLAYWRIGHT_NODE_MODULES=/tmp/monitor-tools/node_modules/
export CHROMIUM=/usr/bin/chromium
export DISPLAY=:91 XAUTHORITY="$HOME/.local/state/zuse-monitor/Xauthority"
zuse-monitor start
node ./checks/smoke.mjs /tmp/agent.png /tmp/viewer.png
bash ./bench/bench.sh
```

The smoke test compares guest input with the read-only viewer, exercises viewer reconnect, and captures both views. Set `MONITOR_VIEWER_PORT` to exercise a local SSH forward. On Linux, pipe `zuse-monitor password` to `python3 ./checks/webkit-probe.py /path/to/zuse-webkit <forwarded-port>` from a separate desktop display to test the real Zuse WebKit helper.

The benchmark builds its own X11 scrolling document. The viewer samples its binary frame counter at animation-frame boundaries and counts distinct IDs in ten seconds. A local TCP proxy applies a 30 Mbit/s limit and 30 or 60 ms RTT. A separate browser workload measures input-to-visible latency by checking a known color transition. CPU is capture worker process time divided by wall time. Run in an otherwise isolated guest with one capture worker.

## Matched comparison

See the [Boxd native and browser comparison](bench/native/RESULTS.md) for 2560×1600 desktop measurements. See [the head-to-head benchmark](bench/BENCHMARK.md) for the common workload, repeated runs, build contention and reproduction instructions. The following sections document earlier prototypes.

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

Before the native capture worker, the poof server used FFmpeg/libx264 for video. These numbers describe that earlier implementation.

| Implementation | 1080p distinct scrolling FPS at 30 ms RTT |
|---|---:|
| Original voxl baseline | 34.3 |
| poof server with experimental VP8 encoding | 3.1 |
| poof server with low-latency H.264 encoding | 19.7 |

These are single exploratory measurements with two CPUs shared by the guest and viewer. The H.264 experiment also reached 19.5 distinct scrolling FPS at 60 ms RTT. Encoder settings and delivered image quality differ. The final build also discards duplicate capture timestamps. Those exploratory runs preceded the matched experiment above.

The standalone image passed a separate read-only viewer, guest input, screenshot and reconnect smoke test. On the live VM, the agent opened the file manager from the launcher, used the editor to save a file, and verified that file on the machine. A fresh external Chromium viewer authenticated over a temporary HTTPS tunnel and decoded the shared desktop. Requests to the external video endpoint without a viewer credential returned 401.
