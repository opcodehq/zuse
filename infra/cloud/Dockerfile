FROM oven/bun:1.3.10 AS executor-runtime

FROM rust:1.99-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends cmake libclang-dev libasound2-dev pkg-config && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --locked --release -p zuse-cloud-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl git zlib1g libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home zuse \
    && install -d -o 10001 -g 10001 -m 700 /var/lib/zuse/recovery
COPY --from=build /src/target/release/zuse-cloud-api /usr/local/bin/zuse-cloud-api
COPY --from=executor-runtime /usr/local/bin/bun /usr/local/bin/bun
COPY packages/executor-v2/vendor/runtime.mjs packages/executor-v2/vendor/host.mjs /opt/zuse/executor/vendor/
ENV ZUSE_CLOUD_EXECUTOR_SCRIPT=/opt/zuse/executor/vendor/host.mjs
USER 10001:10001
ENV ZUSE_CLOUD_LISTEN=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/zuse-cloud-api"]
CMD ["serve"]
