# Build stage
FROM rust:1.81.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

# TODO: Update once rzup 0.3 is released. Current live version does not install due to symlink issue.
# Full install of rzup to get r0vm
RUN curl -L https://risc0-artifacts.s3.us-west-2.amazonaws.com/rzup/test/install | ENV_PATH=test bash && \ 
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

RUN cargo build --release --bin zeth -F zeth

# Use init as we need r0vm to run the executor
FROM init AS runtime

COPY --from=builder /src/target/release/zeth /app/zeth

ENTRYPOINT ["/app/zeth"]
