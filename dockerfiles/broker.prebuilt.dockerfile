# Dockerfile for pre-built broker binary
# Usage: docker build -f dockerfiles/broker.prebuilt.dockerfile --build-arg BINARY_URL=<url> -t broker:prebuilt .

FROM rust:1.88.0-bookworm

ARG BINARY_URL

# Install dependencies (matching non-prebuilt version)
RUN apt-get update && \
    apt-get install -y awscli curl && \
    rm -rf /var/lib/apt/lists/*

# Download broker binary directly
RUN if [ -z "$BINARY_URL" ]; then echo "ERROR: BINARY_URL is required" && exit 1; fi && \
    mkdir -p /app && \
    curl -L -o /app/broker "$BINARY_URL" && \
    chmod +x /app/broker

WORKDIR /app
ENTRYPOINT ["/app/broker"]