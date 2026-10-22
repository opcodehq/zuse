FROM rust:1.99-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p zuse-cloud-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home zuse
COPY --from=build /src/target/release/zuse-cloud-api /usr/local/bin/zuse-cloud-api
USER 10001:10001
ENV ZUSE_CLOUD_LISTEN=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/zuse-cloud-api"]
CMD ["serve"]
