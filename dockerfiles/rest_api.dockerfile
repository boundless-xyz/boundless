# syntax=docker/dockerfile:1
ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

# ── init: toolchain + cargo-chef ────────────────────────────────────
FROM rust:1.85.0-bookworm AS init

RUN apt-get -qq update && apt-get install -y -q clang mold

RUN cargo install cargo-chef

# ── planner: generate dependency recipe ─────────────────────────────
FROM init AS planner

WORKDIR /src/

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ ./crates/
COPY blake3_groth16/ ./blake3_groth16/
COPY bento/ ./bento/

WORKDIR /src/bento
RUN cargo chef prepare --recipe-path /src/recipe.json

# Extract manifests + stub sources for external path deps.
WORKDIR /src
RUN mkdir /manifests && \
    find . \( -name "Cargo.toml" -o -name "Cargo.lock" -o -name "rust-toolchain.toml" \) \
        -not -path "*/target/*" | \
    while read f; do mkdir -p "/manifests/$(dirname "$f")" && cp "$f" "/manifests/$f"; done && \
    find /manifests -name "Cargo.toml" -path "*/crates/*" -o -name "Cargo.toml" -path "*/blake3*" | \
    while read f; do dir=$(dirname "$f") && mkdir -p "$dir/src" && touch "$dir/src/lib.rs"; done

# ── builder: cook deps (cached), then compile source ────────────────
FROM init AS builder

ARG S3_CACHE_PREFIX
ENV SCCACHE_SERVER_PORT=4230

WORKDIR /src/
SHELL ["/bin/bash", "-c"]

COPY --from=planner /manifests/ /src/
COPY --from=planner /src/recipe.json /src/recipe.json

COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"

# Cook dependencies — cached until Cargo.toml/Cargo.lock change.
RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_api_sccache \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    cd /src/bento && \
    cargo chef cook --release --recipe-path /src/recipe.json \
        --package api && \
    sccache --show-stats

# Copy full source and build only the changed application code.
COPY . .

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_api_sccache \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    cargo build --manifest-path bento/Cargo.toml --release -p api --bin rest_api && \
    cp bento/target/release/rest_api /src/rest_api && \
    sccache --show-stats

# ── runtime ─────────────────────────────────────────────────────────
FROM rust:1.85.0-bookworm AS runtime

RUN mkdir /app/ && \
    apt -qq update && \
    apt install -y -q openssl

COPY --from=builder /src/rest_api /app/rest_api
ENTRYPOINT ["/app/rest_api"]
