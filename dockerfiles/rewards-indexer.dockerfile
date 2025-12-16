# Build stage
FROM rust:1.89.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

RUN cargo install cargo-chef

ARG CACHE_DATE=2025-07-17  # update this date to force rebuild
# The rewards indexer doesn't need r0vm to run, but cargo chef pulls in dev-dependencies
# which require it. See https://github.com/LukeMathWalker/cargo-chef/issues/114
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

SHELL ["/bin/bash", "-c"]

RUN cargo build --release --bin rewards-indexer

FROM init AS runtime

COPY --from=builder /src/target/release/rewards-indexer /app/rewards-indexer

ENTRYPOINT ["/app/rewards-indexer"]
