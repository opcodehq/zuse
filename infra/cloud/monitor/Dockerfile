# Build with the monitor directory as context and a prepared Zuse guest as GUEST_IMAGE.
ARG GUEST_IMAGE=standalone
FROM debian:bookworm-slim AS standalone
RUN groupadd --gid 10001 zuse && useradd --uid 10001 --gid 10001 -m zuse
ENV HOME=/home/zuse
WORKDIR /home/zuse
CMD ["zuse-monitor", "serve"]

FROM rust:1.99-bookworm AS streamer
RUN apt-get update && apt-get install -y --no-install-recommends libxext-dev libxdamage-dev liblz4-dev libx264-dev libyuv-dev
WORKDIR /src
COPY stream/capture.c stream/scroll_delta.rs ./
RUN rustc --edition=2021 -O --crate-type staticlib scroll_delta.rs -o libdelta.a
RUN cc -O3 -Wall -Werror capture.c libdelta.a -o /usr/local/bin/zuse-capture -lX11 -lXext -lXdamage -lyuv -lx264 -llz4
COPY stream/Cargo.toml stream/Cargo.lock ./
COPY stream/src ./src
RUN cargo build --release --locked

FROM ${GUEST_IMAGE}
USER root
RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium chromium-sandbox openbox xvfb xauth xdotool ffmpeg python3-gi gir1.2-gtk-3.0 thunar mousepad xterm libxdamage1 liblz4-1 libyuv0 libx264-164 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=streamer /usr/local/bin/zuse-capture /usr/local/bin/
COPY --from=streamer /src/target/release/zuse-display /usr/local/bin/zuse-display
COPY wallpaper.jpg /usr/local/share/zuse-monitor/wallpaper.jpg
COPY --chmod=0755 zuse-monitor zuse-apps /usr/local/bin/
ENV DISPLAY=:91 XAUTHORITY=/home/zuse/.local/state/zuse-monitor/Xauthority
USER 10001:10001
