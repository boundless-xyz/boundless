ARG S3_CACHE_PREFIX="public/rust-cache-docker-Linux-X64/sccache"

FROM rust:1.89.0-bookworm AS builder

RUN apt-get -qq update && apt-get install -y -q clang
RUN cargo install cargo-chef
RUN cargo install sccache

FROM builder AS planner
WORKDIR /src/
COPY bento .
RUN cargo chef prepare --recipe-path recipe.json

FROM builder AS rust-builder
ARG S3_CACHE_PREFIX
# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4230
WORKDIR /src/
COPY --from=planner /src/recipe.json recipe.json
COPY dockerfiles/sccache-config.sh dockerfiles/sccache-config.sh
SHELL ["/bin/bash", "-c"]
RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_api_sccache \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo chef cook --release -p api --bin rest_api && \
    sccache --show-stats

COPY bento .

RUN --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bento_api_sccache \
    source dockerfiles/sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release -p api --bin rest_api && \
    cp target/release/rest_api /src/rest_api && \
    sccache --show-stats

FROM rust:1.89.0-bookworm AS runtime

RUN mkdir /app/ && \
    apt -qq update && \
    apt install -y -q openssl

COPY --from=rust-builder /src/rest_api /app/rest_api
ENTRYPOINT ["/app/rest_api"]
