ARG S3_CACHE_PREFIX="shared/boundless/rust-cache-docker-Linux-X64/sccache"

FROM rust:1.85.0-bookworm AS builder

RUN apt-get -qq update && apt-get install -y -q clang

FROM builder AS rust-builder

ARG S3_CACHE_PREFIX

WORKDIR /src/
# TODO switch this to a tagged commit once one released with bento (https://github.com/boundless-xyz/boundless/issues/570)
RUN git clone --depth=1 --branch main https://github.com/risc0/risc0.git
COPY rust-toolchain.toml .

WORKDIR /src/risc0/bento/

COPY ./dockerfiles/sccache-setup.sh .
RUN ./sccache-setup.sh "x86_64-unknown-linux-musl" "v0.8.2"
COPY ./dockerfiles/sccache-config.sh .
SHELL ["/bin/bash", "-c"]

# Prevent sccache collision in compose-builds
ENV SCCACHE_SERVER_PORT=4230

RUN \
    --mount=type=secret,id=ci_cache_creds,target=/root/.aws/credentials \
    --mount=type=cache,target=/root/.cache/sccache/,id=bndlss_api_sccache \
    source ./sccache-config.sh ${S3_CACHE_PREFIX} && \
    cargo build --release -p api --bin rest_api && \
    cp /src/risc0/bento/target/release/rest_api /src/rest_api && \
    sccache --show-stats

FROM rust:1.85.0-bookworm AS runtime

RUN mkdir /app/ && \
    apt -qq update && \
    apt install -y -q openssl

COPY --from=rust-builder /src/rest_api /app/rest_api
ENTRYPOINT ["/app/rest_api"]
