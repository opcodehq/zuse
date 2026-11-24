# Reproduce the native comparison

Use one disposable Linux desktop with X11, Openbox, Chromium, Node, Playwright, Rust, a C compiler, X11/Xext headers and Python. The native clients use the same host clock as the producer. Each test drives the selected display. Run one cell at a time.

## Prepare the common fixtures

```sh
git clone https://github.com/tembo/voxl /tmp/voxl
git -C /tmp/voxl checkout 21f194f9e446e7073abd7bbd52e0b397c3800b42
export BENCH_ROOT=/tmp/voxl
python3 -m venv /tmp/poof-python
export BENCH_PYTHON=/tmp/poof-python/bin/python
"$BENCH_PYTHON" -m pip install av==19.0.2 numpy==2.5.5 lz4==4.4.5 websockets==15.0.1 Pillow==13.0.0 aiortc==1.16.0
npm install --prefix /tmp/poof-browser playwright@1.65.1
export PLAYWRIGHT_NODE_MODULES=/tmp/poof-browser/node_modules/
export CHROMIUM=/usr/bin/google-chrome
rustc --edition=2021 -O --crate-type cdylib stream/framebuffer.rs -o /tmp/libpoof_framebuffer.so
export POOF_FRAMEBUFFER_LIB=/tmp/libpoof_framebuffer.so
```

## Start the two streamers

Build poof's normal container and point the client at its loopback port. For a direct head-to-head on the same X server, run the two binaries from the poof image and the independently built voxl binary on a benchmark display. Keep the display, IPC namespace, X authority and guest user the same. The following commands use the current user's local Linux session.

```sh
docker build -t poof .
mkdir -p /tmp/poof-bin
image=$(docker create poof)
docker cp "$image":/usr/local/bin/zuse-display /tmp/poof-bin/
docker cp "$image":/usr/local/bin/zuse-capture /tmp/poof-bin/
docker rm "$image"
export PATH=/tmp/poof-bin:$PATH
export DISPLAY=:4 BENCH_DISPLAY=:4
"$BENCH_ROOT/bench/env/xvfb.sh" & sleep 1
openbox &
export ZUSE_DISPLAY_PASSWORD=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
ZUSE_DISPLAY_PORT=6959 zuse-display &
```

Boxd used Google Chrome 158.0.8127.88. The capture binary needs the shared libraries in the final image, including x264, libyuv, libXdamage, LZ4 and libdeflate. The upstream Xvfb launcher uses `-shmem` at 2560×1600 and 144 DPI. A production guest uses X authority and its normal monitor launcher. These benchmark launchers are intended for an isolated VM.

For voxl, build at the pinned revision with the six patches in its x264 derivation. The Boxd test applied them in filename order to x264 commit `c24e06c2e184345ceb33eb20a15d1024d9fd3497`, built the shared library with SIMD assembly enabled, and selected it with `LD_LIBRARY_PATH` for voxl only. Confirm the process maps the patched library and logs direct framebuffer capture. [Upstream x264 build](https://github.com/tembo/voxl/blob/21f194f9e446e7073abd7bbd52e0b397c3800b42/x264.nix)

```sh
git clone https://github.com/mirror/x264 /tmp/poof-bench-x264
git -C /tmp/poof-bench-x264 checkout c24e06c2e184345ceb33eb20a15d1024d9fd3497
(
  cd /tmp/poof-bench-x264
  for file in "$BENCH_ROOT"/patches/x264/*.patch; do git apply "$file"; done
  ./configure --enable-shared --disable-cli --prefix=/tmp/poof-bench-libs
  make -j2 && make install
)
LD_LIBRARY_PATH=/tmp/poof-bench-libs/lib voxl --addr=127.0.0.1 --port=6958 --framerate=60 --preset=ultrafast \
  --max-kbps=30000 --audio-enabled=false --enable-basic-auth=false &
```

## Measure

```sh
bench/native/run.sh poof 127.0.0.1:6959 anim1080 10/40
bench/native/run.sh voxl 127.0.0.1:6958 anim1080 10/40
```

Use `scroll` and `anim1080` on `lan`, `10/40` and `5/60`. Use `animfull` on LAN. The default sample is 20 seconds after four seconds of warm-up. `SAMPLE_SECONDS` overrides its duration. The output includes distinct IDs decoded per second, the slowest one-second bucket, median and p99 marker age, and received Mbit/s. The poof client applies every replacement pixel to a full RGBA desktop using the Rust framebuffer decoder.

For the remaining engines, start their servers with the pinned upstream environment scripts and run the same cell with the matching display, TCP endpoint or Moonlight host. Selkies and KasmVNC use their own X servers. Use the upstream Moonlight patch and pair the client once. Select the advertised 60 FPS x264 profile for tuned Neko and the ultrafast four-thread preset for tuned Sunshine. Use 80% of the link rate for Moonlight on shaped links. [Competitor build and pairing instructions](https://github.com/tembo/voxl/tree/21f194f9e446e7073abd7bbd52e0b397c3800b42/bench)

The Neko confirmation used host ICE on the isolated route and started warm-up on the first decoded frame. To run that client while preserving the upstream checkout:

```sh
export NEKO_CLIENT=$(mktemp --suffix=.py)
patch -s -o "$NEKO_CLIENT" "$BENCH_ROOT/bench/nkclient.py" < bench/native/neko-direct.patch
```

Set `NEKO_RECEIVE_BYTES=4194304` for the tuned profile and the default full-screen receive-buffer diagnostic. Other default-profile cells used aiortc’s normal socket size.

The TCP tests start upstream's deep-queue bottleneck proxy automatically. For WebRTC and Moonlight, use upstream's server-egress netem setup and set `UDP_LINK_CONFIGURED=10/40` or `5/60` to match the cell. The Boxd kernel lacked netem. We routed the client through two Linux TUN devices, serialized server-egress packets by IP size and added the whole RTT as a fixed egress delay, as in upstream's UDP configuration. Pass `CLIENT_NETNS_PID` for an isolated client network namespace and `UDP_SHAPE_FILE` for the active TUN shaper.

For CPU, invoke `python3 bench/native/cpu.py SERVER_PID X_SERVER_PID` when the client starts. It samples seven seconds of process ticks after a three-second delay, including capture children. For idle, leave the viewer connected to a stationary desktop. Use the bundled browser smoke test for the complete human-facing viewer and agent-input path.

### Repeat the userspace UDP link on Boxd

Use a fresh VM with Docker forwarding enabled. The commands below create one client namespace and a nonpersistent TUN pair. Keep the router running during UDP cells. The IPs reserve a /30 on this isolated guest.

```sh
sudo ip netns add poofbench
export UDP_SHAPE_FILE=$(mktemp)
echo lan >"$UDP_SHAPE_FILE"
sudo python3 bench/native/udp-link.py poofbench "$UDP_SHAPE_FILE" &
sudo ip netns exec poofbench sleep 7200 &
export CLIENT_NETNS_PID=$(sudo ip netns pids poofbench | head -1)
sudo ip netns exec poofbench ping -c 3 10.244.9.1
```

For Sunshine, connect Moonlight to `10.244.9.1`. For Neko, use the upstream bridge configuration with `172.30.9.10` as the advertised ICE address. Omit the netem sidecar when using this router. Permit forwarding only between that container and the test client. On the test VM, Docker's raw PREROUTING rule dropped direct packets to the container until the scoped exception was installed.

```sh
sudo iptables -t raw -I PREROUTING -i poofhost -s 10.244.9.2 -d 172.30.9.10 -j ACCEPT
sudo iptables -I FORWARD -i poofhost -s 10.244.9.2 -d 172.30.9.10 -j ACCEPT
sudo iptables -I FORWARD -o poofhost -s 172.30.9.10 -d 10.244.9.2 -j ACCEPT
sudo iptables -t nat -I POSTROUTING -o poofhost -s 172.30.9.10 -d 10.244.9.2 -j ACCEPT
```

Run the same four rules with `-D` in place of `-I` when finished. Stop the router and namespace sleeper, then run `sudo ip netns del poofbench`. The TUN interfaces disappear when the router closes its descriptors.
