ARG S3_CACHE_PREFIX="public/boundless/rust-cache-docker-Linux-X64/sccache"

FROM rust:1.88-bookworm AS builder

RUN apt-get -qq update && apt-get install -y -q clang mold && \
    cargo install cargo-chef --locked

FROM builder AS planner

WORKDIR /src/bento
COPY bento/ .
RUN cargo chef prepare --recipe-path /src/recipe.json

FROM builder AS rust-builder

ARG S3_CACHE_PREFIX
ARG S3_CACHE_BUCKET="boundless-sccache"
ENV SCCACHE_BUCKET=${S3_CACHE_BUCKET}

WORKDIR /src/

COPY --from=planner /src/recipe.json /src/recipe.json
COPY dockerfiles/sccache-setup.sh dockerfiles/sccache-config.sh ./dockerfiles/
RUN dockerfiles/sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
SHELL ["/bin/bash", "-c"]

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4230
ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_api_sccache \
    --mount=type=cache,target=/usr/local/cargo/registry,id=cargo_registry \
    --mount=type=cache,target=/src/bento/target,id=rest_api_target \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo chef cook --release --recipe-path recipe.json --package api && \
    sccache --show-stats

COPY . .

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_api_sccache \
    --mount=type=cache,target=/usr/local/cargo/registry,id=cargo_registry \
    --mount=type=cache,target=/src/bento/target,id=rest_api_target \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    (ulimit -n 65536 2>/dev/null || true) && \
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} && \
    cargo build --manifest-path bento/Cargo.toml --release -p api --bin rest_api && \
    cp bento/target/release/rest_api /src/rest_api && \
    sccache --show-stats

FROM rust:1.88-bookworm AS runtime

RUN mkdir /app/ && \
    apt -qq update && \
    apt install -y -q openssl

COPY --from=rust-builder /src/rest_api /app/rest_api
ENTRYPOINT ["/app/rest_api"]
