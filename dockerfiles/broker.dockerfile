ARG S3_CACHE_PREFIX="shared/boundless/rust-cache-docker-Linux-X64/sccache"

FROM rust:1.85.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

RUN curl -L https://foundry.paradigm.xyz | bash && \
    source /root/.bashrc && \
    foundryup

# Github token can be provided as a secret with the name githubTokenSecret. Useful
# for shared build environments where Github rate limiting is an issue.
RUN --mount=type=secret,id=githubTokenSecret,target=/run/secrets/githubTokenSecret \
    if [ -f /run/secrets/githubTokenSecret ]; then \
        GITHUB_TOKEN=$(cat /run/secrets/githubTokenSecret) curl -L https://risczero.com/install | bash && \
        GITHUB_TOKEN=$(cat /run/secrets/githubTokenSecret) PATH="$PATH:/root/.risc0/bin" rzup install rust 1.85.0; \
    else \
        curl -L https://risczero.com/install | bash && \
        PATH="$PATH:/root/.risc0/bin" rzup install rust 1.85.0; \
    fi

FROM init AS builder

ARG S3_CACHE_PREFIX

WORKDIR /src/
COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY contracts/ ./contracts/
COPY documentation/ ./documentation/
COPY lib/ ./lib/
COPY remappings.txt .
COPY foundry.toml .

ENV PATH="$PATH:/root/.foundry/bin"
RUN forge build

COPY ./dockerfiles/sccache-setup.sh .
RUN ./sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
COPY ./dockerfiles/sccache-config.sh .
SHELL ["/bin/bash", "-c"]

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4228

RUN \
    --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bndlss_broker_sc \
    source ./sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release --bin broker && \
    cp /src/target/release/broker /src/broker && \
    sccache --show-stats

FROM rust:1.85.0-bookworm AS runtime

RUN mkdir /app/

COPY --from=builder /src/broker /app/broker
RUN apt-get update && \
    apt-get install -y awscli && \
    rm -rf /var/lib/apt/lists/*

ENTRYPOINT ["/app/broker"]
