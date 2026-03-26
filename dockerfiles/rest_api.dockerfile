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

WORKDIR /src/

COPY --from=planner /src/bento/recipe.json /src/bento/recipe.json
COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
SHELL ["/bin/bash", "-c"]

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4230
ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"

# Cook dependencies — cached until Cargo.toml/Cargo.lock change.
RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_api_sccache \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    cd /src/bento && \
    cargo chef cook --release --recipe-path recipe.json --package api && \
    sccache --show-stats

# Copy full source and build only the changed application code.
COPY . .

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

RUN apt-get -qq update && \
    apt-get install -y -q --no-install-recommends ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/rest_api /app/rest_api
ENTRYPOINT ["/app/rest_api"]
