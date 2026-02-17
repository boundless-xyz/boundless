# syntax=docker/dockerfile:1
# CPU-only agent build (no CUDA/GPU support)
ARG RUST_IMG=rust:1.88-bookworm
ARG RUNTIME_IMG=debian:bookworm-slim
ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

FROM ${RUST_IMG} AS rust-builder

ARG DEBIAN_FRONTEND=noninteractive
ENV TZ="America/Los_Angeles"

RUN apt-get -qq update && apt-get install -y -q \
    openssl libssl-dev pkg-config curl clang git \
    build-essential openssh-client unzip

# Install protoc
RUN curl -o protoc.zip -L https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-linux-x86_64.zip \
    && unzip protoc.zip -d /usr/local \
    && rm protoc.zip

# Install RISC0 (CPU-only, no groth16 needed for exec/aux)
ENV RISC0_HOME=/usr/local/risc0
ENV PATH="/root/.cargo/bin:${PATH}"

RUN curl -L https://risczero.com/install | bash && \
    rm -rf /tmp/* /var/tmp/*

FROM rust-builder AS builder

ARG S3_CACHE_PREFIX
ENV SCCACHE_SERVER_PORT=4227

WORKDIR /src/
COPY . .

RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
SHELL ["/bin/bash", "-c"]

ARG RUSTFLAGS="-C target-cpu=native"
ENV RUSTFLAGS=${RUSTFLAGS}

# Build WITHOUT cuda feature
RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_agent_cpu_sc \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --manifest-path bento/Cargo.toml --release -p workflow --bin agent && \
    cp bento/target/release/agent /src/agent && \
    sccache --show-stats

FROM ${RUNTIME_IMG} AS runtime

RUN apt-get update -q -y \
    && apt-get install -q -y ca-certificates libssl3 curl tar xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Main agent (CPU-only)
COPY --from=builder /src/agent /app/agent

ENTRYPOINT ["/app/agent"]
