# Build with the monitor directory as context and a prepared Zuse guest as GUEST_IMAGE.
ARG GUEST_IMAGE=standalone
FROM debian:bookworm-slim AS standalone
RUN groupadd --gid 10001 zuse && useradd --uid 10001 --gid 10001 -m zuse
ENV HOME=/home/zuse
WORKDIR /home/zuse
CMD ["zuse-monitor", "serve"]

FROM rust:1.99-bookworm AS streamer
RUN apt-get update && apt-get install -y --no-install-recommends libxext-dev libxdamage-dev liblz4-dev libdeflate-dev libx264-dev libyuv-dev
WORKDIR /src
COPY stream/capture.c stream/raster.c stream/raster.h stream/scroll_delta.rs stream/palette.rs ./
RUN rustc --edition=2021 -O --crate-type staticlib scroll_delta.rs -o libdelta.a
RUN cc -O3 -Wall -Werror capture.c raster.c libdelta.a -o /usr/local/bin/zuse-capture -lX11 -lXext -lXdamage -lyuv -lx264 -llz4 -ldeflate
COPY stream/Cargo.toml stream/Cargo.lock ./
COPY stream/src ./src
RUN cargo build --release --locked

FROM ${GUEST_IMAGE} AS guest
USER root
RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium chromium-sandbox openbox xvfb xauth xdotool ffmpeg python3-gi gir1.2-gtk-3.0 thunar mousepad xterm libxdamage1 liblz4-1 libdeflate0 libyuv0 libx264-164 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=streamer /usr/local/bin/zuse-capture /usr/local/bin/
COPY --from=streamer /src/target/release/zuse-display /usr/local/bin/zuse-display
COPY guest/assets/wallpaper.jpg /usr/local/share/zuse-monitor/wallpaper.jpg
COPY --chmod=0755 guest/zuse-monitor guest/zuse-apps /usr/local/bin/
ENV DISPLAY=:91 XAUTHORITY=/home/zuse/.local/state/zuse-monitor/Xauthority
USER 10001:10001

FROM guest AS screen-sharing
USER root
RUN apt-get update && apt-get install -y --no-install-recommends x11vnc && rm -rf /var/lib/apt/lists/*
USER 10001:10001
COPY --chmod=755 demo/screen-sharing /usr/local/bin/
CMD ["screen-sharing"]

FROM guest AS runtime
