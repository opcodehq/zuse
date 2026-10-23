FROM rust:1.99-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends cmake libclang-dev libasound2-dev pkg-config && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --locked --release -p zuse-cloud-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates zlib1g libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home zuse \
    && install -d -o 10001 -g 10001 -m 700 /var/lib/zuse/recovery
COPY --from=build /src/target/release/zuse-cloud-api /usr/local/bin/zuse-cloud-api
USER 10001:10001
ENV ZUSE_CLOUD_LISTEN=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/zuse-cloud-api"]
CMD ["serve"]
