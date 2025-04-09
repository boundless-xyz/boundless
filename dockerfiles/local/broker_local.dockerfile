FROM rust:1.85.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

RUN curl -L https://foundry.paradigm.xyz | bash && \
    source /root/.bashrc && \
    foundryup

RUN curl -L https://risczero.com/install | bash && \
    PATH="$PATH:/root/.risc0/bin" rzup install rust 1.81.0

FROM init AS builder

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

SHELL ["/bin/bash", "-c"]

RUN cargo build --release --bin broker && \
    cp /src/target/release/broker /src/broker

FROM rust:1.85.0-bookworm AS runtime

RUN mkdir /app/

COPY --from=builder /src/broker /app/broker
RUN apt-get update && \
    apt-get install -y awscli && \
    rm -rf /var/lib/apt/lists/*

ENTRYPOINT ["/app/broker"]
