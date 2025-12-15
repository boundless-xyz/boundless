# syntax=docker/dockerfile:1
ARG CUDA_IMG=nvidia/cuda:12.9.1-devel-ubuntu24.04
ARG CUDA_RUNTIME_IMG=nvidia/cuda:12.9.1-runtime-ubuntu24.04
ARG S3_CACHE_PREFIX="public/rust-cache-docker-Linux-X64/sccache"

FROM ${CUDA_IMG} AS rust-builder

ARG DEBIAN_FRONTEND=noninteractive
ENV TZ="America/Los_Angeles"

RUN apt-get -qq update && apt-get install -y -q \
    openssl libssl-dev pkg-config curl clang git \
    build-essential openssh-client unzip

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# Install rust and target version (should match rust-toolchain.toml for best speed)
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y \
    && chmod -R a+w $RUSTUP_HOME $CARGO_HOME \
    && rustup install 1.88

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
    # Clean up any temporary files to reduce image size
    rm -rf /tmp/* /var/tmp/*

FROM rust-builder AS builder

ARG NVCC_APPEND_FLAGS="\
  --generate-code arch=compute_75,code=sm_75 \
  --generate-code arch=compute_86,code=sm_86 \
  --generate-code arch=compute_89,code=sm_89 \
  --generate-code arch=compute_120,code=sm_120"
ARG CUDA_OPT_LEVEL=1
ARG S3_CACHE_PREFIX
ENV NVCC_APPEND_FLAGS=${NVCC_APPEND_FLAGS}
ENV RISC0_CUDA_OPT=${CUDA_OPT_LEVEL}
ENV SCCACHE_SERVER_PORT=4227

WORKDIR /src/
COPY . .

RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
SHELL ["/bin/bash", "-c"]

# Consider using if building and running on the same CPU
ARG RUSTFLAGS="-C target-cpu=native"
ENV RUSTFLAGS=${RUSTFLAGS}

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_agent_sc \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --manifest-path bento/Cargo.toml --release -p workflow -F cuda --bin agent && \
    cp bento/target/release/agent /src/agent && \
    sccache --show-stats

FROM ${CUDA_RUNTIME_IMG} AS runtime

RUN apt-get update -q -y \
    && apt-get install -q -y ca-certificates libssl3 curl tar xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Download and extract BLAKE3 Groth16 artifacts.
ARG BLAKE3_GROTH16_ARTIFACTS_URL
# If USE_LOCAL_BLAKE3_GROTH16_SETUP is set to 1 or yes or true, skip downloading and expect user to mount setup files
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

# Main prover
COPY --from=builder /src/agent /app/agent
COPY --from=builder /usr/local/risc0 /usr/local/risc0


ENTRYPOINT ["/app/agent"]
