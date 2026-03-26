# Proving artifacts image — contains risc0-groth16 and blake3-groth16 artifacts.
# Built once and cached; only rebuilds when artifacts change.
# Used by agent.dockerfile via COPY --from=artifacts.
FROM debian:bookworm-slim

RUN apt-get -qq update && \
    apt-get install -y -q --no-install-recommends curl xz-utils ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Install risc0-groth16 via rzup
RUN curl -fSL https://risczero.com/install | bash && \
    /root/.risc0/bin/rzup install risc0-groth16 && \
    mkdir -p /artifacts/risc0-groth16 && \
    cp /root/.risc0/extensions/v0.1.0-risc0-groth16/* /artifacts/risc0-groth16/ && \
    rm -rf /root/.risc0 /tmp/* /var/tmp/*

# Download blake3-groth16 artifacts
ARG BLAKE3_GROTH16_ARTIFACTS_URL=https://staging-signal-artifacts.beboundless.xyz/v3/proving/blake3_groth16_artifacts.tar.xz
RUN mkdir -p /artifacts/blake3-groth16 && \
    curl -fSL -o /tmp/artifacts.tar.xz "$BLAKE3_GROTH16_ARTIFACTS_URL" && \
    tar -xf /tmp/artifacts.tar.xz -C /artifacts/blake3-groth16 --strip-components=1 && \
    rm -f /tmp/artifacts.tar.xz

# Provide risc0 settings.toml for proper RISC0_HOME setup
RUN mkdir -p /artifacts/risc0-home/extensions /artifacts/risc0-home/tmp && \
    printf '[default_versions]\nrisc0-groth16 = "0.1.0"\n' > /artifacts/risc0-home/settings.toml && \
    ln -s /artifacts/risc0-groth16 /artifacts/risc0-home/extensions/v0.1.0-risc0-groth16
