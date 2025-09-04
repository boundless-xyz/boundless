# Dockerfile for pre-built broker binary
# Usage: docker build -f dockerfiles/broker.prebuilt.dockerfile --build-arg BINARY_URL=<url> -t broker:prebuilt .

# Use Ubuntu 24.04 for GLIBC 2.38+ compatibility
FROM ubuntu:24.04

ARG BINARY_URL

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y curl ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Download broker binary directly
RUN if [ -z "$BINARY_URL" ]; then echo "ERROR: BINARY_URL is required" && exit 1; fi && \
    mkdir -p /app && \
    curl -L -o /app/broker "$BINARY_URL" && \
    chmod +x /app/broker

WORKDIR /app
ENTRYPOINT ["/app/broker"]