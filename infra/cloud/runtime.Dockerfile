FROM rust:1.99-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends cmake libclang-dev libasound2-dev pkg-config && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --locked --release -p zuse-cloud-guest

# Prepared workspace images derive from this guest base and install their selected CLIs.
FROM node:22-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates zlib1g git openssh-client ripgrep python3 build-essential \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 zuse \
    && useradd --uid 10001 --gid 10001 --shell /bin/bash --create-home zuse \
    && mkdir -p /home/zuse/.zuse-data /var/lib/zuse/workspace \
    && chown -R zuse:zuse /home/zuse /var/lib/zuse/workspace
COPY --from=build /src/target/release/zuse-cloud-runtime /usr/local/bin/zuse-cloud-runtime
COPY --chmod=0755 infra/cloud/launch-runtime.sh /usr/local/lib/zuse/launch-rust-runtime
USER 10001:10001
WORKDIR /home/zuse
ENV HOME=/home/zuse ZUSE_USER_DATA=/home/zuse/.zuse-data
ENTRYPOINT ["/usr/local/bin/zuse-cloud-runtime"]
