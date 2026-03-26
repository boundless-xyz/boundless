# syntax=docker/dockerfile:1
ARG CUDA_IMG=nvidia/cuda:13.0.2-devel-ubuntu24.04
ARG CUDA_RUNTIME_IMG=nvidia/cuda:13.0.2-runtime-ubuntu24.04
ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

# ── init: toolchain, risc0, cargo-chef ──────────────────────────────
FROM ${CUDA_IMG} AS init

ARG DEBIAN_FRONTEND=noninteractive
ENV TZ="America/Los_Angeles"

RUN apt-get -qq update && apt-get install -y -q \
    openssl libssl-dev pkg-config curl clang git \
    build-essential openssh-client unzip mold

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y \
    && chmod -R a+w $RUSTUP_HOME $CARGO_HOME \
    && rustup install 1.88

# Install protoc
RUN curl -o protoc.zip -L https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-linux-x86_64.zip \
    && unzip protoc.zip -d /usr/local \
    && rm protoc.zip

ENV RISC0_HOME=/usr/local/risc0
ENV PATH="/root/.cargo/bin:${PATH}"

RUN curl -L https://risczero.com/install | bash && \
    /root/.risc0/bin/rzup install risc0-groth16 && \
    rm -rf /tmp/* /var/tmp/*

RUN cargo install cargo-chef

# ── planner: generate dependency recipe ─────────────────────────────
FROM init AS planner

WORKDIR /src/

# Both workspaces are needed: bento/ has a path dep on blake3_groth16/
# which resolves workspace deps from the root Cargo.toml.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ ./crates/
COPY blake3_groth16/ ./blake3_groth16/
COPY bento/ ./bento/

RUN cargo chef prepare --manifest-path bento/Cargo.toml --recipe-path recipe.json

# ── builder: cook deps (cached), then compile source ────────────────
FROM init AS builder

ARG NVCC_APPEND_FLAGS="\
  --generate-code arch=compute_75,code=sm_75 \
  --generate-code arch=compute_86,code=sm_86 \
  --generate-code arch=compute_89,code=sm_89 \
  --generate-code arch=compute_120,code=sm_120"
ARG CUDA_OPT_LEVEL=1
ARG S3_CACHE_PREFIX
ENV NVCC_APPEND_FLAGS=${NVCC_APPEND_FLAGS}
ENV RISC0_CUDA_OPT=${CUDA_OPT_LEVEL}
ENV SCCACHE_SERVER_PORT=4229

WORKDIR /src/
SHELL ["/bin/bash", "-c"]

COPY --from=planner /src/recipe.json /src/recipe.json

COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"

ARG RUSTFLAGS="-C target-cpu=native -C link-arg=-fuse-ld=mold"
ENV RUSTFLAGS=${RUSTFLAGS}

# Cook dependencies — this layer is cached until Cargo.toml/Cargo.lock change.
# CUDA kernel compilation (the slowest part) happens here and gets cached.
RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_agent_sc \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    export CARGO_TARGET_DIR=/src/bento/target-agent-gpu && \
    cargo chef cook --manifest-path bento/Cargo.toml --release \
        --recipe-path recipe.json --package workflow --features cuda && \
    sccache --show-stats

# Copy full source and build only the changed application code.
COPY . .

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_agent_sc \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    export CARGO_TARGET_DIR=/src/bento/target-agent-gpu && \
    cargo build --manifest-path bento/Cargo.toml --release -p workflow -F cuda --bin agent && \
    cp ${CARGO_TARGET_DIR}/release/agent /src/agent && \
    sccache --show-stats

# ── runtime ─────────────────────────────────────────────────────────
FROM ${CUDA_RUNTIME_IMG} AS runtime

RUN apt-get update -q -y \
    && apt-get install -q -y ca-certificates libssl3 curl tar xz-utils \
    && rm -rf /var/lib/apt/lists/*

ARG BLAKE3_GROTH16_ARTIFACTS_URL
ARG USE_LOCAL_BLAKE3_GROTH16_SETUP
ENV BLAKE3_GROTH16_SETUP_DIR=/.blake3_groth16_artifacts/

RUN if [ "$USE_LOCAL_BLAKE3_GROTH16_SETUP" = "1" ] || [ "$USE_LOCAL_BLAKE3_GROTH16_SETUP" = "yes" ] || [ "$USE_LOCAL_BLAKE3_GROTH16_SETUP" = "true" ]; then \
      echo "Using local BLAKE3 Groth16 setup files, skipping download"; \
    else \
      if [ -z "$BLAKE3_GROTH16_ARTIFACTS_URL" ]; then \
        echo "ERROR: BLAKE3_GROTH16_ARTIFACTS_URL not specified. Either set USE_LOCAL_BLAKE3_GROTH16_SETUP=1 and mount the setup files, or provide a URL via BLAKE3_GROTH16_ARTIFACTS_URL"; exit 1; \
      fi && \
      mkdir -p $BLAKE3_GROTH16_SETUP_DIR && \
      curl -L -o /tmp/blake3_groth16_artifacts.tar.xz "$BLAKE3_GROTH16_ARTIFACTS_URL" && \
      tar -xvf /tmp/blake3_groth16_artifacts.tar.xz -C $BLAKE3_GROTH16_SETUP_DIR  --strip-components=1 && \
      rm -rf /tmp/* ; \
    fi

COPY --from=builder /src/agent /app/agent
COPY --from=builder /usr/local/risc0 /usr/local/risc0

ENTRYPOINT ["/app/agent"]
