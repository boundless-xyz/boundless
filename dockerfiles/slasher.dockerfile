# Build stage
FROM rust:1.81.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

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

RUN cargo build --release --bin boundless-slasher

FROM init AS runtime

COPY --from=builder /src/target/release/boundless-slasher /app/boundless-slasher

ENTRYPOINT ["/app/boundless-slasher"]
