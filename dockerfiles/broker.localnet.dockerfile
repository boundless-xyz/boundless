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

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=localnet_broker_cargo_registry \
    --mount=type=cache,target=/src/target,id=localnet_broker_target \
    if [ "$BUILD_PROFILE" = "dev" ]; then \
        cargo build -p broker --bin broker && \
        cp /src/target/debug/broker /src/broker; \
    else \
        cargo build --release -p broker --bin broker && \
        cp /src/target/release/broker /src/broker; \
    fi

FROM debian:bookworm-slim AS runtime

RUN apt-get -qq update && \
    apt-get install -y -q --no-install-recommends ca-certificates libssl3 curl && \
    rm -rf /var/lib/apt/lists/*

# Copy r0vm for dev mode local proving
COPY --from=builder /usr/local/cargo/bin/r0vm /usr/local/bin/r0vm

COPY --from=builder /src/broker /app/broker
COPY scripts/localnet-broker.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

ENTRYPOINT ["/app/entrypoint.sh"]
