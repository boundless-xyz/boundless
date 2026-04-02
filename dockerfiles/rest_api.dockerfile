ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

FROM rust:1.89-bookworm AS builder

RUN apt-get -qq update && apt-get install -y -q clang mold

FROM builder AS rust-builder

ARG S3_CACHE_PREFIX
ARG S3_CACHE_BUCKET="boundless-sccache"
ENV SCCACHE_BUCKET=${S3_CACHE_BUCKET}

WORKDIR /src/
COPY . .
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
SHELL ["/bin/bash", "-c"]

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4230
ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_api_sccache \
    --mount=type=cache,target=/usr/local/cargo/registry,id=cargo_registry \
    --mount=type=cache,target=/src/bento/target,id=rest_api_target \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    cargo build --manifest-path bento/Cargo.toml --release -p api --bin rest_api && \
    cp bento/target/release/rest_api /src/rest_api && \
    sccache --show-stats

FROM debian:bookworm-slim AS runtime

RUN mkdir /app/ && \
    apt-get -qq update && \
    apt-get install -y -q --no-install-recommends ca-certificates libssl3 curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /src/rest_api /app/rest_api
ENTRYPOINT ["/app/rest_api"]
