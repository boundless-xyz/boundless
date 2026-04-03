ARG BUILDER_BASE=ghcr.io/boundless-xyz/boundless/builder-base:latest
FROM ${BUILDER_BASE} AS builder

WORKDIR /src

SHELL ["/bin/bash", "-c"]

ENV RISC0_SKIP_BUILD=1
ENV RISC0_SKIP_BUILD_KERNELS=1
ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ ./crates/
COPY contracts/ ./contracts/
COPY lib/ ./lib/
COPY remappings.txt foundry.toml ./
COPY blake3_groth16/ ./blake3_groth16/

ARG BUILD_PROFILE=dev

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=localnet_os_cargo_registry \
    --mount=type=cache,target=/src/target,id=localnet_os_target \
    if [ "$BUILD_PROFILE" = "dev" ]; then \
        cargo build -p order-stream --bin order_stream && \
        cp /src/target/debug/order_stream /src/order_stream; \
    else \
        cargo build --release -p order-stream --bin order_stream && \
        cp /src/target/release/order_stream /src/order_stream; \
    fi

FROM debian:bookworm-slim AS runtime

RUN apt-get -qq update && \
    apt-get install -y -q --no-install-recommends ca-certificates libssl3 libpq5 curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/order_stream /app/order_stream
COPY scripts/localnet-order-stream.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

ENTRYPOINT ["/app/entrypoint.sh"]
