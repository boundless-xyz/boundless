FROM rust:1.79.0-bookworm AS builder

RUN apt-get -qq update && \
    apt-get install -y -q clang

FROM builder AS rust-builder

WORKDIR /src/
COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY contracts/ ./contracts/
COPY lib/ ./lib/
COPY remappings.txt .
COPY foundry.toml .

COPY ./dockerfiles/sccache-setup.sh .
RUN ./sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.1"
COPY ./dockerfiles/sccache-config.sh .
SHELL ["/bin/bash", "-c"]

RUN curl -L https://foundry.paradigm.xyz | bash && \
    source /root/.bashrc && \
    foundryup

ENV PATH="$PATH:/root/.foundry/bin"
RUN forge build

RUN \
    --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bndlss_orderstream_sccache \
    source ./sccache-config.sh && \
    ls /root/.cache/sccache/ && \
    cargo install --version 1.6.9 cargo-binstall && \
    cargo binstall -y --force cargo-risczero --version 1.1 && \
    cargo risczero install && \
    sccache --show-stats

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4229

RUN \
    --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bndlss_orderstream_sccache \
    source ./sccache-config.sh && \
    cargo build --release -p order-stream --bin order_stream && \
    cp /src/target/release/order_stream /src/order_stream && \
    sccache --show-stats

FROM rust:1.79.0-bookworm AS runtime

RUN mkdir /app/

COPY --from=rust-builder /src/order_stream /app/order_stream
ENTRYPOINT ["/app/order_stream"]
