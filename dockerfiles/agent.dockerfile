# syntax=docker/dockerfile:1
ARG CUDA_IMG=nvidia/cuda:13.0.2-devel-ubuntu24.04
ARG CUDA_RUNTIME_IMG=nvidia/cuda:13.0.2-runtime-ubuntu24.04
ARG PROVING_ARTIFACTS=ghcr.io/boundless-xyz/boundless/proving-artifacts:v1
ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

FROM ${CUDA_IMG} AS rust-builder

ARG DEBIAN_FRONTEND=noninteractive
ENV TZ="America/Los_Angeles"

RUN apt-get -qq update && apt-get install -y -q \
    openssl libssl-dev pkg-config curl clang git \
    build-essential openssh-client unzip mold

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# Install rust and target version (should match rust-toolchain.toml for best speed)
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y \
    && chmod -R a+w $RUSTUP_HOME $CARGO_HOME \
    && rustup install 1.88

RUN cargo install cargo-chef --locked

# Install protoc
RUN curl -o protoc.zip -L https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-linux-x86_64.zip \
    && unzip protoc.zip -d /usr/local \
    && rm protoc.zip

# Install RISC0 and groth16 component early for better caching
ENV RISC0_HOME=/usr/local/risc0
ENV PATH="/root/.cargo/bin:${PATH}"

# Install RISC0 and groth16 component - this layer will be cached unless RISC0_HOME changes
RUN curl -L https://risczero.com/install | bash && \
    /root/.risc0/bin/rzup install risc0-groth16 && \
    rm -rf /tmp/* /var/tmp/*

FROM rust-builder AS planner

WORKDIR /src/bento
COPY bento/ .
RUN cargo chef prepare --recipe-path /src/bento/recipe.json

FROM rust-builder AS builder

ARG NVCC_APPEND_FLAGS="\
  --generate-code arch=compute_75,code=sm_75 \
  --generate-code arch=compute_86,code=sm_86 \
  --generate-code arch=compute_89,code=sm_89 \
  --generate-code arch=compute_120,code=sm_120"
ARG CUDA_OPT_LEVEL=1
ARG S3_CACHE_PREFIX
ARG S3_CACHE_BUCKET="boundless-sccache"
ENV NVCC_APPEND_FLAGS=${NVCC_APPEND_FLAGS}
ENV RISC0_CUDA_OPT=${CUDA_OPT_LEVEL}
ENV SCCACHE_BUCKET=${S3_CACHE_BUCKET}
ENV SCCACHE_SERVER_PORT=4227

WORKDIR /src/

COPY --from=planner /src/bento/recipe.json /src/bento/recipe.json
COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
SHELL ["/bin/bash", "-c"]

# Consider using if building and running on the same CPU
ARG RUSTFLAGS="-C target-cpu=native -C link-arg=-fuse-ld=mold"
ENV RUSTFLAGS=${RUSTFLAGS}

# Cook dependencies — cached until Cargo.toml/Cargo.lock change.
# CUDA kernel compilation happens here and gets cached.
RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_agent_sc \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    export CARGO_TARGET_DIR=/src/bento/target-agent-gpu && \
    cd /src/bento && \
    cargo chef cook --release --recipe-path recipe.json \
        --package workflow --features cuda && \
    sccache --show-stats

# Copy full source and build only the changed application code.
COPY . .

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_agent_sc \
    --mount=type=cache,target=/usr/local/cargo/registry,id=cargo_registry \
    --mount=type=cache,target=/src/bento/target-agent-gpu,id=agent_target \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    export CARGO_TARGET_DIR=/src/bento/target-agent-gpu && \
    cargo build --manifest-path bento/Cargo.toml --release -p workflow -F cuda --bin agent && \
    cp ${CARGO_TARGET_DIR}/release/agent /src/agent && \
    sccache --show-stats

# Proving artifacts — pulled from a pre-built GHCR image. BuildKit caches the
# pull so artifacts are only downloaded once and reused across builds.
FROM ${PROVING_ARTIFACTS} AS artifacts

FROM ${CUDA_RUNTIME_IMG} AS runtime

RUN apt-get update -q -y \
    && apt-get install -q -y ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

ENV BLAKE3_GROTH16_SETUP_DIR=/.blake3_groth16_artifacts/
ENV RISC0_HOME=/usr/local/risc0

COPY --from=artifacts /artifacts/blake3-groth16/ /.blake3_groth16_artifacts/
COPY --from=artifacts /artifacts/risc0-home/ /usr/local/risc0/
COPY --from=builder /src/agent /app/agent

ENTRYPOINT ["/app/agent"]
