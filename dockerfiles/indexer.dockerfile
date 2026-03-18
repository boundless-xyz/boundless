FROM rust:1.89.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

RUN cargo install cargo-chef

# protoc needed for market-indexer-backfill dependencies
RUN curl -o protoc.zip -L https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-linux-x86_64.zip \
    && unzip protoc.zip -d /usr/local \
    && rm protoc.zip

ARG CACHE_DATE=2026-02-13
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

RUN cargo chef prepare --recipe-path recipe.json

FROM init AS builder

WORKDIR /src

SHELL ["/bin/bash", "-c"]

COPY --from=planner /src/recipe.json /src/recipe.json

COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"

ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

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
COPY xtask/ ./xtask/

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release --bin market-indexer && \
    cargo build --release --bin rewards-indexer && \
    cargo build --release --bin market-efficiency-indexer && \
    cargo build --release --bin market-indexer-backfill && \
    sccache --show-stats

FROM init AS runtime

COPY --from=builder /src/target/release/market-indexer /app/market-indexer
COPY --from=builder /src/target/release/rewards-indexer /app/rewards-indexer
COPY --from=builder /src/target/release/market-efficiency-indexer /app/market-efficiency-indexer
COPY --from=builder /src/target/release/market-indexer-backfill /app/market-indexer-backfill
