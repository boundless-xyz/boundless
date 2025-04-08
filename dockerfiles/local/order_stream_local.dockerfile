FROM rust:1.85.0-bookworm AS builder

RUN apt-get -qq update && \
    apt-get install -y -q clang

FROM builder AS rust-builder

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

SHELL ["/bin/bash", "-c"]

RUN curl -L https://foundry.paradigm.xyz | bash && \
    source /root/.bashrc && \
    foundryup

ENV PATH="$PATH:/root/.foundry/bin"
RUN forge build

RUN curl -L https://risczero.com/install | bash && \
    PATH="$PATH:/root/.risc0/bin" rzup install rust 1.81.0


RUN cargo build --release -p order-stream --bin order_stream && \
    cp /src/target/release/order_stream /src/order_stream

FROM rust:1.85.0-bookworm AS runtime

RUN mkdir /app/

RUN apt-get -qq update && \
    apt install -y postgresql-client

COPY --from=rust-builder /src/order_stream /app/order_stream
ENTRYPOINT ["/app/order_stream"]
