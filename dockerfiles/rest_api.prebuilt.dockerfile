# Dockerfile for pre-built bento-rest-api binary
# Usage: docker build -f dockerfiles/rest_api.prebuilt.dockerfile --build-arg BINARY_URL=<url> -t bento-rest-api:prebuilt .

FROM rust:1.85.0-bookworm

ARG BINARY_URL
RUN apt-get update && \
    apt-get install -y curl tar && \
    rm -rf /var/lib/apt/lists/*

# Download and extract bento bundle tar.gz
RUN if [ -z "$BINARY_URL" ]; then echo "ERROR: BINARY_URL is required" && exit 1; fi && \
    mkdir -p /app && \
    curl -L -o /tmp/bento-bundle.tar.gz "$BINARY_URL" && \
    tar -xzf /tmp/bento-bundle.tar.gz -C /tmp && \
    mv /tmp/bento-bundle/bento-rest-api /app/rest_api && \
    rm -rf /tmp/*

WORKDIR /app
ENTRYPOINT ["/app/rest_api"]