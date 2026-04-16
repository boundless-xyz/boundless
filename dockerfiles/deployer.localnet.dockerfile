# Deployer container for localnet contract deployment.
# Copies forge/cast from foundry, r0vm from builder-base, adds python3/git/jq/mc.
FROM ghcr.io/foundry-rs/foundry:stable AS foundry

FROM ghcr.io/boundless-xyz/boundless/builder-base:latest AS risc0

FROM debian:bookworm-slim

# Install dependencies
RUN apt-get -qq update && \
    apt-get install -y -q --no-install-recommends \
    ca-certificates curl git jq python3 python3-pip bash && \
    pip3 install --break-system-packages tomlkit && \
    rm -rf /var/lib/apt/lists/*

# Copy foundry binaries
COPY --from=foundry /usr/local/bin/forge /usr/local/bin/forge
COPY --from=foundry /usr/local/bin/cast /usr/local/bin/cast

# Copy r0vm from builder-base (installed via rzup into cargo bin)
COPY --from=risc0 /usr/local/cargo/bin/r0vm /usr/local/bin/r0vm

# Install MinIO client for uploading guest binaries
RUN curl -fsSL https://dl.min.io/client/mc/release/linux-amd64/mc -o /usr/local/bin/mc && \
    chmod +x /usr/local/bin/mc

WORKDIR /src

ENTRYPOINT ["/src/scripts/localnet-deploy.sh"]
