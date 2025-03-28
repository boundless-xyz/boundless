# Build stage
FROM rust:1.85.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

RUN curl -L https://risczero.com/install  | ENV_PATH=test bash && \ 
    PATH="$PATH:/root/.risc0/bin" rzup install
ENV RISC0_SERVER_PATH=/usr/local/cargo/bin/r0vm

FROM init AS builder

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

SHELL ["/bin/bash", "-c"]

RUN cargo build --release --bin boundless-order-generator

# Use init as we need r0vm to run the executor
FROM init AS runtime

COPY --from=builder /src/target/release/boundless-order-generator /app/boundless-order-generator

ENTRYPOINT ["/app/boundless-order-generator"]
