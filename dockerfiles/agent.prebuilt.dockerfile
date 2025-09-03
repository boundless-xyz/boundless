# Dockerfile for pre-built bento-agent binary
# Usage: docker build -f dockerfiles/agent.prebuilt.dockerfile --build-arg BINARY_URL=<url> -t bento-agent:prebuilt .

ARG CUDA_RUNTIME_IMG=nvidia/cuda:12.9.1-runtime-ubuntu24.04
FROM ${CUDA_RUNTIME_IMG}

ARG BINARY_URL

# Install runtime dependencies matching non-prebuilt version
RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 curl tar && \
    rm -rf /var/lib/apt/lists/*

# Download and extract bento bundle tar.gz
RUN if [ -z "$BINARY_URL" ]; then echo "ERROR: BINARY_URL is required" && exit 1; fi && \
    mkdir -p /app && \
    curl -L -o /tmp/bento-bundle.tar.gz "$BINARY_URL" && \
    tar -xzf /tmp/bento-bundle.tar.gz -C /tmp && \
    mv /tmp/bento-bundle/bento-agent /app/agent && \
    rm -rf /tmp/*

WORKDIR /app
ENTRYPOINT ["/app/agent"]