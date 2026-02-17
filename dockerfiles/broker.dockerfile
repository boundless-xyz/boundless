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
    GITHUB_TOKEN=$(cat /run/secrets/githubTokenSecret) PATH="$PATH:/root/.risc0/bin" rzup install rust 1.88.0; \
    else \
    curl -L https://risczero.com/install | bash && \
    PATH="$PATH:/root/.risc0/bin" rzup install rust 1.88.0; \
    fi

RUN cargo install cargo-chef

# Install protoc
RUN curl -o protoc.zip -L https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-linux-x86_64.zip \
    && unzip protoc.zip -d /usr/local \
    && rm protoc.zip

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
RUN cargo chef prepare --recipe-path recipe.json

FROM init AS builder

WORKDIR /src/

SHELL ["/bin/bash", "-c"]

COPY --from=planner /src/recipe.json /src/recipe.json

COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"

ARG S3_CACHE_PREFIX="public/rust-cache-docker-Linux-X64/sccache"

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo chef cook --release --recipe-path recipe.json --package broker && \
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
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release --bin broker && \
    cp /src/target/release/broker /src/broker && \
    sccache --show-stats

FROM rust:1.89.0-bookworm AS runtime

RUN mkdir /app/

COPY --from=builder /src/broker /app/broker
RUN apt-get update && \
    apt-get install -y awscli && \
    rm -rf /var/lib/apt/lists/*

ENTRYPOINT ["/app/broker"]
