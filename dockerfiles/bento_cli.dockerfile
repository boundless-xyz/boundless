# syntax=docker/dockerfile:1
ARG BUILDER_BASE=ghcr.io/boundless-xyz/boundless/builder-base:latest
ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

FROM ${BUILDER_BASE} AS init

FROM init AS planner

WORKDIR /src/bento
COPY bento/ .
RUN cargo chef prepare --recipe-path /src/bento/recipe.json

FROM init AS builder

ARG S3_CACHE_PREFIX
ARG S3_CACHE_BUCKET="boundless-sccache"
ENV SCCACHE_BUCKET=${S3_CACHE_BUCKET}
ENV SCCACHE_SERVER_PORT=4227

WORKDIR /src/

COPY --from=planner /src/bento/recipe.json /src/bento/recipe.json
COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
SHELL ["/bin/bash", "-c"]

ARG RUSTFLAGS="-C target-cpu=native -C link-arg=-fuse-ld=mold"
ENV RUSTFLAGS=${RUSTFLAGS}

# Cook dependencies — cached until Cargo.toml/Cargo.lock change.
RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_cli_sc \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    cd /src/bento && \
    cargo chef cook --release --recipe-path recipe.json --package bento-client && \
    sccache --show-stats

# Copy full source and build only the changed application code.
COPY . .

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_cli_sc \
    --mount=type=cache,target=/usr/local/cargo/registry,id=cargo_registry \
    --mount=type=cache,target=/src/bento/target,id=bento_cli_target \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    cargo build --manifest-path bento/Cargo.toml --release -p bento-client --bin bento_cli && \
    cp bento/target/release/bento_cli /src/bento_cli && \
    sccache --show-stats

FROM debian:bookworm-slim AS runtime

RUN apt-get update -q -y \
    && apt-get install -q -y ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

# bento_cli binary
COPY --from=builder /src/bento_cli /app/bento_cli
COPY --from=builder /root/.risc0 /root/.risc0

ENTRYPOINT ["/app/bento_cli"]
