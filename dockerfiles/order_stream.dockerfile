FROM rust:1.85.0-bookworm AS init

RUN apt-get -qq update && \
    apt-get install -y -q clang

SHELL ["/bin/bash", "-c"]

RUN curl -L https://foundry.paradigm.xyz | bash && \
    source /root/.bashrc && \
    foundryup

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

WORKDIR /src/

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

ENV PATH="$PATH:/root/.foundry/bin"
RUN forge build

SHELL ["/bin/bash", "-c"]

RUN cargo build --release -p order-stream --bin order_stream && \
    cp /src/target/release/order_stream /src/order_stream

FROM rust:1.85.0-bookworm AS runtime

RUN mkdir /app/

RUN apt-get -qq update && \
    apt install -y postgresql-client

COPY --from=builder /src/order_stream /app/order_stream

ENTRYPOINT ["/app/order_stream"]
