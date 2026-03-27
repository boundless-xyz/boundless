# Base image for all Boundless service Docker builds.
# Contains: Rust toolchain, clang, mold, cargo-chef, and RISC Zero toolchain.
# Rebuild when rust-toolchain.toml or RISC Zero version changes.
FROM rust:1.89.0-bookworm

RUN apt-get -qq update && \
    apt-get install -y -q clang mold && \
    rm -rf /var/lib/apt/lists/*

SHELL ["/bin/bash", "-c"]

RUN cargo install cargo-chef --locked

ARG CACHE_DATE=2026-03-25
# Github token can be provided as a secret with the name githubTokenSecret. Useful
# for shared build environments where Github rate limiting is an issue.
RUN --mount=type=secret,id=githubTokenSecret,target=/run/secrets/githubTokenSecret \
    if [ -f /run/secrets/githubTokenSecret ]; then \
    GITHUB_TOKEN=$(cat /run/secrets/githubTokenSecret) curl -L https://risczero.com/install | bash && \
    GITHUB_TOKEN=$(cat /run/secrets/githubTokenSecret) PATH="$PATH:/root/.risc0/bin" rzup install; \
    else \
    curl -L https://risczero.com/install | bash && \
    PATH="$PATH:/root/.risc0/bin" rzup install; \
    fi
