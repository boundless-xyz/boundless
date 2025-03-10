# syntax=docker/dockerfile:1
ARG CUDA_IMG=nvidia/cuda:12.2.0-devel-ubuntu22.04
ARG CUDA_RUNTIME_IMG=nvidia/cuda:12.2.0-runtime-ubuntu22.04
ARG S3_CACHE_PREFIX="shared/boundless/rust-cache-docker-Linux-X64/sccache"

FROM ${CUDA_IMG} AS rust-builder

ARG DEBIAN_FRONTEND=noninteractive
ENV TZ="America/Los_Angeles"

RUN apt-get -qq update && apt-get install -y -q \
    openssl libssl-dev pkg-config curl clang git \
    build-essential openssh-client

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# Install rust and a target rust version (should match rust-toolchain.toml for best speed)
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
RUN chmod -R a+w $RUSTUP_HOME $CARGO_HOME
RUN rustup install 1.81

FROM rust-builder AS builder

ARG NVCC_APPEND_FLAGS=""
ARG CUDA_OPT_LEVEL=1
ARG S3_CACHE_PREFIX

WORKDIR /src/
COPY bento/ ./bento/
COPY rust-toolchain.toml .

WORKDIR /src/bento/

ENV NVCC_APPEND_FLAGS=${NVCC_APPEND_FLAGS}
ENV RISC0_CUDA_OPT=${CUDA_OPT_LEVEL}

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4227
# ENV SCCACHE_LOG=trace

COPY ./dockerfiles/sccache-setup.sh .
RUN ./sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
COPY ./dockerfiles/sccache-config.sh .
SHELL ["/bin/bash", "-c"]

# Consider using if building and running on the same CPU
# ENV RUSTFLAGS="-C target-cpu=native"

RUN \
    --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bndlss_agent_sc \
    source ./sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release -p workflow -F cuda --bin agent && \
    cp /src/bento/target/release/agent /src/agent && \
    sccache --show-stats

FROM risczero/risc0-groth16-prover:v2024-05-17.1 AS binaries

FROM ${CUDA_RUNTIME_IMG} AS runtime

RUN mkdir /app/ && \
    apt-get -qq update && \
    apt-get install -y -q openssl libssl-dev \
    libsodium23 nodejs npm && \
    npm install -g snarkjs@0.7.3

# Stark2snark
COPY --from=binaries /usr/local/sbin/rapidsnark /usr/local/sbin/rapidsnark
COPY --from=binaries /app/stark_verify /app/stark_verify
COPY --from=binaries /app/stark_verify.dat /app/stark_verify.dat
COPY --from=binaries /app/stark_verify_final.zkey /app/stark_verify_final.zkey

# Main prover
COPY --from=builder /src/agent /app/agent

ENTRYPOINT ["/app/agent"]
