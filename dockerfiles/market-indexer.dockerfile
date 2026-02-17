# Build stage
FROM rust:1.89.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

RUN cargo install cargo-chef

ARG CACHE_DATE=2026-02-13  # update this date to force rebuild
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

RUN cargo chef prepare  --recipe-path recipe.json

FROM init AS builder

WORKDIR /src

SHELL ["/bin/bash", "-c"]

COPY --from=planner /src/recipe.json /src/recipe.json

COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"

ARG S3_CACHE_PREFIX="public/rust-cache-docker-Linux-X64/sccache"

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo chef cook --release --recipe-path recipe.json --package boundless-indexer && \
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

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release --bin market-indexer && \
    sccache --show-stats

FROM init AS runtime

COPY --from=builder /src/target/release/market-indexer /app/market-indexer

ENTRYPOINT ["/app/market-indexer"]
