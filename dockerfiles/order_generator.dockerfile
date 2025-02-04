ARG S3_CACHE_PREFIX="shared/boundless/rust-cache-docker-Linux-X64/sccache"

# Build stage
FROM rust:1.81.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

RUN curl -L https://risczero.com/install | bash && \
    PATH="$PATH:/root/.risc0/bin" rzup install rust r0.1.81.0

FROM init AS builder

ARG S3_CACHE_PREFIX

WORKDIR /src

COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY contracts/ ./contracts/
COPY documentation/ ./documentation/
COPY lib/ ./lib/
COPY remappings.txt .
COPY foundry.toml .

COPY ./dockerfiles/sccache-setup.sh .
RUN ./sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
COPY ./dockerfiles/sccache-config.sh .
SHELL ["/bin/bash", "-c"]

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4231

RUN \
    --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bndlss_api_sccache \
    source ./sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release --bin boundless-order-generator && \
    sccache --show-stats

FROM rust:1.81.0-bookworm AS runtime

RUN apt-get -qq update

COPY --from=builder /src/target/release/boundless-order-generator /app/boundless-order-generator

ENTRYPOINT ["/app/boundless-order-generator"]
