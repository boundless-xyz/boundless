# syntax=docker/dockerfile:1
ARG RUST_IMG=rust:1.88-bookworm
ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

FROM ${RUST_IMG} AS rust-builder

ARG DEBIAN_FRONTEND=noninteractive
ENV TZ="America/Los_Angeles"

RUN apt-get -qq update && apt-get install -y -q \
    openssl libssl-dev pkg-config curl clang git \
    build-essential openssh-client unzip mold

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN cargo install cargo-chef --locked

# # Install RISC0 and groth16 component early for better caching
ENV RISC0_HOME=/usr/local/risc0
ENV PATH="/root/.cargo/bin:${PATH}"

# # Install RISC0 and groth16 component - this layer will be cached unless RISC0_HOME changes
RUN curl -L https://risczero.com/install | bash && \
    /root/.risc0/bin/rzup install && \
    # Clean up any temporary files to reduce image size
    rm -rf /tmp/* /var/tmp/*

FROM rust-builder AS planner

WORKDIR /src/bento
COPY bento/ .
RUN cargo chef prepare --recipe-path /src/bento/recipe.json

FROM rust-builder AS builder

ARG S3_CACHE_PREFIX
ARG S3_CACHE_BUCKET="boundless-sccache"
ENV SCCACHE_BUCKET=${S3_CACHE_BUCKET}
ENV SCCACHE_SERVER_PORT=4227

WORKDIR /src/

COPY --from=planner /src/bento/recipe.json /src/bento/recipe.json
COPY blake3_groth16/Cargo.toml ./blake3_groth16/Cargo.toml
RUN printf '[workspace]\nmembers = ["blake3_groth16"]\n\n[workspace.package]\nversion = "0.0.0"\nedition = "2021"\nhomepage = "."\nrepository = "."\n' > /src/Cargo.toml && \
    mkdir -p /src/blake3_groth16/src && touch /src/blake3_groth16/src/lib.rs
COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
SHELL ["/bin/bash", "-c"]

# Consider using if building and running on the same CPU
ARG RUSTFLAGS="-C target-cpu=native -C link-arg=-fuse-ld=mold"
ENV RUSTFLAGS=${RUSTFLAGS}

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_cli_sc \
    --mount=type=cache,target=/usr/local/cargo/registry,id=cargo_registry \
    --mount=type=cache,target=/src/bento/target,id=bento_cli_target \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cd /src/bento && cargo chef cook --release --recipe-path recipe.json --package bento-client && \
    sccache --show-stats

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
COPY --from=builder /usr/local/risc0 /usr/local/risc0

ENTRYPOINT ["/app/bento_cli"]
