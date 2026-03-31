# Dockerfile for pre-built CPU-only bento-agent binary
# Usage: docker build -f dockerfiles/agent.cpu.prebuilt.dockerfile --build-arg BINARY_URL=<url> -t bento-agent-cpu:prebuilt .

# Use Ubuntu 24.04 for GLIBC 2.38+ compatibility (matches CI build environment)
FROM ubuntu:24.04

ARG BINARY_URL

# Install minimal runtime dependencies
RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 curl tar && \
    rm -rf /var/lib/apt/lists/*

# Download and extract CPU agent binary from bento bundle
RUN if [ -z "$BINARY_URL" ]; then echo "ERROR: BINARY_URL is required" && exit 1; fi && \
    mkdir -p /app && \
    curl -L -o /tmp/bento-bundle.tar.gz "$BINARY_URL" && \
    tar -xzf /tmp/bento-bundle.tar.gz -C /tmp && \
    mv /tmp/bento-bundle/bento-agent-cpu /app/agent && \
    rm -rf /tmp/*

WORKDIR /app
ENTRYPOINT ["/app/agent"]
