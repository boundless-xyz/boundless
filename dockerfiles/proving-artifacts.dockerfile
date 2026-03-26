# Proving artifacts image — contains risc0-groth16 and blake3-groth16 artifacts.
# Built once and cached; only rebuilds when artifacts change.
# Used by agent.dockerfile via COPY --from=artifacts.
FROM rust:1.89.0-bookworm

# Install risc0-groth16 via rzup and copy to /artifacts
RUN curl -fSL https://risczero.com/install | bash && \
    /root/.risc0/bin/rzup install risc0-groth16 && \
    mkdir -p /artifacts/risc0-home/extensions /artifacts/risc0-home/tmp && \
    cp -r /root/.risc0/extensions/v0.1.0-risc0-groth16 /artifacts/risc0-home/extensions/ && \
    cp /root/.risc0/settings.toml /artifacts/risc0-home/ && \
    touch /artifacts/risc0-home/.rzup && \
    touch /artifacts/risc0-home/tmp/.risc0-groth16-0.1.0.lock && \
    rm -rf /root/.risc0 /tmp/* /var/tmp/*

# Download blake3-groth16 artifacts
ARG BLAKE3_GROTH16_ARTIFACTS_URL=https://staging-signal-artifacts.beboundless.xyz/v3/proving/blake3_groth16_artifacts.tar.xz
RUN mkdir -p /artifacts/blake3-groth16 && \
    curl -fSL -o /tmp/artifacts.tar.xz "$BLAKE3_GROTH16_ARTIFACTS_URL" && \
    tar -xf /tmp/artifacts.tar.xz -C /artifacts/blake3-groth16 --strip-components=1 && \
    rm -f /tmp/artifacts.tar.xz
