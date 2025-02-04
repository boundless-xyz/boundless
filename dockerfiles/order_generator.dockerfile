# Build stage
FROM rust:1.81.0-bookworm AS builder

RUN apt-get -qq update && \
    apt-get install -y -q clang

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

RUN cargo build --release --bin boundless-order-generator

# Runtime stage
FROM rust:1.81.0-bookworm AS runtime

RUN apt-get -qq update

COPY --from=builder /src/target/release/boundless-order-generator /app/boundless-order-generator

ENTRYPOINT ["/app/boundless-order-generator"]
