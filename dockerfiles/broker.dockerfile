FROM rust:1.79.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

COPY ./dockerfiles/sccache-setup.sh .
RUN ./sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.1"
COPY ./dockerfiles/sccache-config.sh .
SHELL ["/bin/bash", "-c"]

RUN curl -L https://foundry.paradigm.xyz | bash && \
    source /root/.bashrc && \
    foundryup

# RUN curl -L https://risczero.com/install | bash
RUN \
    # --mount=type=cache,target=/root/.cache/sccache/ \
    # --mount=type=cache,target=/usr/local/cargo/git/db \
    # --mount=type=cache,target=/usr/local/cargo/registry/ \
    source ./sccache-config.sh && \
    cargo install --version 1.6.9 cargo-binstall && \
    cargo binstall -y --force cargo-risczero --version 1.1 && \
    cargo risczero install

FROM init AS builder

WORKDIR /src/
COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY .sqlx/ ./.sqlx/
COPY contracts/ ./contracts/
COPY lib/ ./lib/
COPY remappings.txt .
COPY foundry.toml .

ENV PATH="$PATH:/root/.foundry/bin"
RUN forge build

RUN \
    # --mount=type=cache,target=target,rw,id=spear_broker \
    # --mount=type=cache,target=/root/.cargo/,rw,id=spear_broker_cargo \
    # --mount=type=cache,target=/root/.cache/sccache/,rw,id=spear_broker_sccache \
    # source ./sccache-config.sh && \
    cargo build --release --bin broker && \
    cp /src/target/release/broker /src/broker

FROM rust:1.79.0-bookworm AS runtime

RUN mkdir /app/

COPY --from=builder /src/broker /app/broker

ENTRYPOINT ["/app/broker"]
