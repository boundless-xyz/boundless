# Use official Rust image as base
FROM rust:1.83-slim AS builder

# Install system dependencies required for building
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy workspace manifest files
COPY Cargo.toml Cargo.lock ./
COPY rust-toolchain.toml ./

# Copy all crate directories and documentation (required for workspace build)
COPY crates/ ./crates/
COPY documentation/ ./documentation/

# Build the CLI with locked dependencies
RUN cargo build --release --locked -p boundless-cli

# Create runtime image
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    && rm -rf /var/lib/apt/lists/*

# Copy the built binary from builder stage
COPY --from=builder /app/target/release/boundless /usr/local/bin/boundless
COPY --from=builder /app/target/release/boundless-ffi /usr/local/bin/boundless-ffi

# Set the entrypoint
ENTRYPOINT ["/usr/local/bin/boundless"]
