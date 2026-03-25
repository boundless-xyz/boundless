FROM rust:1.89.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]
ARG CACHE_DATE=2026-02-13  # update this date to force rebuild
RUN curl -L https://foundry.paradigm.xyz | bash && \
    source /root/.bashrc && \
    foundryup

# Github token can be provided as a secret with the name githubTokenSecret. Useful
# for shared build environments where Github rate limiting is an issue.
RUN --mount=type=secret,id=githubTokenSecret,target=/run/secrets/githubTokenSecret \
    if [ -f /run/secrets/githubTokenSecret ]; then \
    GITHUB_TOKEN=$(cat /run/secrets/githubTokenSecret) curl -L https://risczero.com/install | bash && \
    GITHUB_TOKEN=$(cat /run/secrets/githubTokenSecret) PATH="$PATH:/root/.risc0/bin" rzup install; \
    else \
    curl -L https://risczero.com/install | bash && \
    PATH="$PATH:/root/.risc0/bin" rzup install; \
    fi

RUN cargo install cargo-chef

FROM init AS planner

WORKDIR /src/

COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY contracts/ ./contracts/
COPY lib/ ./lib/
COPY remappings.txt .
COPY foundry.toml .
COPY blake3_groth16/ ./blake3_groth16/

RUN cargo chef prepare  --recipe-path recipe.json

FROM init AS builder

WORKDIR /src

SHELL ["/bin/bash", "-c"]

ENV RISC0_SKIP_BUILD=1
ENV RISC0_SKIP_BUILD_KERNELS=1
ENV CARGO_PROFILE_RELEASE_LTO=thin
ENV CARGO_PROFILE_RELEASE_DEBUG=0

COPY --from=planner /src/recipe.json /src/recipe.json

COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"

ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"
ARG S3_CACHE_BUCKET="boundless-sccache"
ENV SCCACHE_BUCKET=${S3_CACHE_BUCKET}

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=order_stream_sc \
    --mount=type=cache,target=/usr/local/cargo/registry,id=cargo_registry \
    --mount=type=cache,target=/src/target,id=order_stream_target \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo chef cook --release --recipe-path recipe.json --package order-stream && \
    sccache --show-stats

COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY contracts/ ./contracts/
COPY lib/ ./lib/
COPY remappings.txt .
COPY foundry.toml .
COPY blake3_groth16/ ./blake3_groth16/

ENV PATH="$PATH:/root/.foundry/bin"
RUN forge build

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=order_stream_sc \
    --mount=type=cache,target=/usr/local/cargo/registry,id=cargo_registry \
    --mount=type=cache,target=/src/target,id=order_stream_target \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release -p order-stream --bin order_stream && \
    cp /src/target/release/order_stream /src/order_stream && \
    sccache --show-stats

FROM debian:bookworm-slim AS runtime

RUN apt-get -qq update && \
    apt-get install -y -q --no-install-recommends ca-certificates libssl3 libpq5 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/order_stream /app/order_stream

ENTRYPOINT ["/app/order_stream"]
