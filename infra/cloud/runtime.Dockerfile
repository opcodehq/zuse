FROM rust:1.99-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends cmake libclang-dev libasound2-dev pkg-config && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --locked --release -p zuse-cloud-guest

# Prepared workspace images derive from this guest base and install their selected CLIs.
FROM node:22-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates zlib1g git openssh-client ripgrep python3 build-essential sudo util-linux \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 zuse \
    && useradd --uid 10001 --gid 10001 --shell /bin/bash --create-home zuse \
    && mkdir -p /home/zuse/.zuse-data /home/zuse/workspace /var/lib/zuse/workspace /var/lib/zuse/recovery \
    && chown -R zuse:zuse /home/zuse /var/lib/zuse/workspace /var/lib/zuse/recovery \
    && chmod 0700 /home/zuse/.zuse-data
COPY --from=build /src/target/release/zuse-cloud-runtime /usr/local/bin/zuse-cloud-runtime
RUN ln -s /usr/local/bin/zuse-cloud-runtime /usr/local/bin/zuse-grok-auth \
    && printf 'zuse ALL=(ALL) NOPASSWD:ALL\n' > /etc/sudoers.d/zuse \
    && chmod 0440 /etc/sudoers.d/zuse
COPY --chmod=0755 infra/cloud/harness-release.py /usr/local/lib/zuse/harness-release.py
COPY --chmod=0755 infra/cloud/launch-runtime.sh /usr/local/lib/zuse/launch-rust-runtime
COPY --chmod=0755 infra/cloud/launch-recovery.sh /usr/local/lib/zuse/launch-rust-recovery
COPY infra/cloud/runtime-repair.py /usr/local/lib/zuse/runtime-repair.py
COPY infra/cloud/runtime-recreation.py /usr/local/lib/zuse/runtime-recreation.py
COPY infra/cloud/runtime-supervisor.py /usr/local/lib/zuse/runtime-supervisor.py
RUN chmod 0555 /usr/local/lib/zuse/runtime-supervisor.py
USER 10001:10001
WORKDIR /home/zuse
ENV HOME=/home/zuse ZUSE_USER_DATA=/home/zuse/.zuse-data
ENTRYPOINT ["/usr/bin/python3", "/usr/local/lib/zuse/runtime-supervisor.py", "serve"]
