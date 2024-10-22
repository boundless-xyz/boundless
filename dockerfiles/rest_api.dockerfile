FROM rust:1.79.0-bookworm AS builder

RUN apt-get -qq update && apt-get install -y -q clang

FROM builder AS rust-builder

WORKDIR /src/
COPY Cargo.toml .
COPY Cargo.lock .
COPY crates/ ./crates/
COPY rust-toolchain.toml .
COPY .sqlx/ ./.sqlx/

COPY ./dockerfiles/sccache-setup.sh .
RUN ./sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.1"
COPY ./dockerfiles/sccache-config.sh .
SHELL ["/bin/bash", "-c"]

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4228

RUN cargo

RUN \
    --mount=type=cache,target=/root/.cache/sccache/,id=bndlss_api_sccache \
    source ./sccache-config.sh && \
    cargo build --release -p api --bin rest_api && \
    cp /src/target/release/rest_api /src/rest_api


FROM rust:1.79.0-bookworm AS runtime

RUN mkdir /app/ && \
    apt -qq update && \
    apt install -y -q openssl

COPY --from=rust-builder /src/rest_api /app/rest_api
ENTRYPOINT ["/app/rest_api"]
