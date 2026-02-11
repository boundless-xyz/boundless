# Build stage for all indexer binaries
# This single image contains all indexer binaries: market-indexer, rewards-indexer, 
# market-indexer-backfill, and market-efficiency-indexer.
#
# Usage:
#   docker run <image> market-indexer [args...]
#   docker run <image> rewards-indexer [args...]
#   docker run <image> market-indexer-backfill [args...]
#   docker run <image> market-efficiency-indexer [args...]

FROM rust:1.89.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

RUN cargo install cargo-chef

# Install protoc (needed for market-indexer-backfill)
RUN curl -o protoc.zip -L https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-linux-x86_64.zip \
    && unzip protoc.zip -d /usr/local \
    && rm protoc.zip

ARG CACHE_DATE=2025-10-13  # update this date to force rebuild
# The indexer doesn't need r0vm to run, but its tests do need it. 
# Cargo chef always pulls in and builds dev-dependencies, meaning that we need to install r0vm
# to leverage chef. See https://github.com/LukeMathWalker/cargo-chef/issues/114
# For this reason we also need to build for amd64.
#
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

FROM init AS planner

WORKDIR /src

COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY contracts/ ./contracts/
COPY lib/ ./lib/
COPY remappings.txt .
COPY foundry.toml .
COPY blake3_groth16/ ./blake3_groth16/
COPY xtask/ ./xtask/

RUN cargo chef prepare  --recipe-path recipe.json

FROM init AS builder

WORKDIR /src

COPY --from=planner /src/recipe.json /src/recipe.json

RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY contracts/ ./contracts/
COPY lib/ ./lib/
COPY remappings.txt .
COPY foundry.toml .
COPY blake3_groth16/ ./blake3_groth16/
COPY xtask/ ./xtask/

SHELL ["/bin/bash", "-c"]

# Build all indexer binaries in a single cargo invocation for efficiency
RUN cargo build --release \
    --bin market-indexer \
    --bin rewards-indexer \
    --bin market-indexer-backfill \
    --bin market-efficiency-indexer

FROM init AS runtime

# Copy all indexer binaries
COPY --from=builder /src/target/release/market-indexer /app/market-indexer
COPY --from=builder /src/target/release/rewards-indexer /app/rewards-indexer
COPY --from=builder /src/target/release/market-indexer-backfill /app/market-indexer-backfill
COPY --from=builder /src/target/release/market-efficiency-indexer /app/market-efficiency-indexer

WORKDIR /app

# No default entrypoint - user specifies which binary to run
# Example: docker run <image> ./market-indexer --help
