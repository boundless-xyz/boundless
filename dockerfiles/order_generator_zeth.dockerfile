FROM rust:1.85.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

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

RUN cargo install cargo-chef

FROM init AS planner

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

RUN cargo chef prepare  --recipe-path recipe.json

FROM init as builder

WORKDIR /src

COPY --from=planner /src/recipe.json /src/recipe.json

RUN cargo chef cook --release --recipe-path recipe.json

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


RUN cargo build --release --bin order-generator-zeth -F zeth

# Use init as we need r0vm to run the executor
FROM init AS runtime

COPY --from=builder /src/target/release/order-generator-zeth /app/order-generator-zeth

ENTRYPOINT ["/app/order-generator-zeth"]
