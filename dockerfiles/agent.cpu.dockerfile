# syntax=docker/dockerfile:1
# CPU-only agent build (no CUDA/GPU support)
ARG RUST_IMG=rust:1.88-bookworm
ARG RUNTIME_IMG=debian:bookworm-slim
ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

# ── init: toolchain, risc0, cargo-chef ──────────────────────────────
FROM ${RUST_IMG} AS init

ARG DEBIAN_FRONTEND=noninteractive
ENV TZ="America/Los_Angeles"

RUN apt-get -qq update && apt-get install -y -q \
    openssl libssl-dev pkg-config curl clang git \
    build-essential openssh-client unzip mold

# Install protoc
RUN curl -o protoc.zip -L https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-linux-x86_64.zip \
    && unzip protoc.zip -d /usr/local \
    && rm protoc.zip

ENV RISC0_HOME=/usr/local/risc0
ENV PATH="/root/.cargo/bin:${PATH}"

RUN curl -L https://risczero.com/install | bash && \
    rm -rf /tmp/* /var/tmp/*

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
ENV SCCACHE_SERVER_PORT=4228

WORKDIR /src/
SHELL ["/bin/bash", "-c"]

COPY --from=planner /manifests/ /src/
COPY --from=planner /src/recipe.json /src/recipe.json

COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"

ARG RUSTFLAGS="-C target-cpu=native -C link-arg=-fuse-ld=mold"
ENV RUSTFLAGS=${RUSTFLAGS}

# Cook dependencies — this layer is cached until Cargo.toml/Cargo.lock change.
RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_agent_cpu_sc \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    export CARGO_TARGET_DIR=/src/bento/target-agent-cpu && \
    cd /src/bento && \
    cargo chef cook --release --recipe-path /src/recipe.json \
        --package workflow && \
    sccache --show-stats

# Copy full source and build only the changed application code.
COPY . .

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_agent_cpu_sc \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    export CARGO_TARGET_DIR=/src/bento/target-agent-cpu && \
    cargo build --manifest-path bento/Cargo.toml --release -p workflow --bin agent && \
    cp ${CARGO_TARGET_DIR}/release/agent /src/agent && \
    sccache --show-stats

# ── runtime ─────────────────────────────────────────────────────────
FROM ${RUNTIME_IMG} AS runtime

RUN apt-get update -q -y \
    && apt-get install -q -y ca-certificates libssl3 curl tar xz-utils \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/agent /app/agent

ENTRYPOINT ["/app/agent"]
