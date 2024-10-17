FROM rust:1.79.0-bookworm AS builder

RUN apt-get -qq update && apt-get install -y -q clang

FROM builder AS rust-builder

WORKDIR /src/
COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY .sqlx/ ./.sqlx/

RUN cargo

RUN \
    --mount=type=cache,target=target,id=bndlss_api_target \
    cargo build --release -p api --bin rest_api && \
    cp /src/target/release/rest_api /src/rest_api

FROM rust:1.79.0-bookworm AS runtime

RUN mkdir /app/ && \
    apt -qq update && \
    apt install -y -q openssl

COPY --from=rust-builder /src/rest_api /app/rest_api
ENTRYPOINT ["/app/rest_api"]
