# TODO: param these correctly and detect OS / arch
ARG S3_CACHE_PREFIX="shared/boundless/rust-cache-docker-Linux-X64/sccache"

FROM rust:1.81.0-bookworm AS builder

RUN apt-get -qq update && \
    apt-get install -y -q clang

FROM builder AS rust-builder

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

COPY ./dockerfiles/sccache-setup.sh .
RUN ./sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
COPY ./dockerfiles/sccache-config.sh .
SHELL ["/bin/bash", "-c"]

RUN curl -L https://foundry.paradigm.xyz | bash && \
    source /root/.bashrc && \
    foundryup

ENV PATH="$PATH:/root/.foundry/bin"
RUN forge build

RUN curl -L https://risczero.com/install | bash && \
    PATH="$PATH:/root/.risc0/bin" rzup install rust r0.1.81.0

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4229

RUN \
    --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bndlss_orderstream_sccache \
    source ./sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release -p order-stream --bin order_stream && \
    cp /src/target/release/order_stream /src/order_stream && \
    sccache --show-stats

FROM rust:1.81.0-bookworm AS runtime

RUN mkdir /app/

RUN apt-get -qq update && \
    apt install -y postgresql-client

COPY --from=rust-builder /src/order_stream /app/order_stream
ENTRYPOINT ["/app/order_stream"]
