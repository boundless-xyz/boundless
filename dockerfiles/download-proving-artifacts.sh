#!/bin/bash
set -e

BLAKE3_DIR="${BLAKE3_GROTH16_DIR:-/data/blake3-groth16}"
RISC0_DIR="${RISC0_GROTH16_DIR:-/data/risc0-groth16}"
BLAKE3_URL="${BLAKE3_GROTH16_ARTIFACTS_URL:-https://staging-signal-artifacts.beboundless.xyz/v3/proving/blake3_groth16_artifacts.tar.xz}"

# Download BLAKE3 groth16 artifacts if not present
if [ -f "$BLAKE3_DIR/verify_for_guest_final.zkey" ]; then
    echo "BLAKE3 groth16 artifacts already present, skipping download"
else
    echo "Downloading BLAKE3 groth16 artifacts from $BLAKE3_URL ..."
    mkdir -p "$BLAKE3_DIR"
    curl -fSL -o /tmp/blake3_groth16_artifacts.tar.xz "$BLAKE3_URL"
    tar -xf /tmp/blake3_groth16_artifacts.tar.xz -C "$BLAKE3_DIR" --strip-components=1
    rm -f /tmp/blake3_groth16_artifacts.tar.xz
    echo "BLAKE3 groth16 artifacts downloaded to $BLAKE3_DIR"
fi

# Download risc0-groth16 artifacts if not present
if [ -f "$RISC0_DIR/stark_verify_final.zkey" ]; then
    echo "risc0-groth16 artifacts already present, skipping download"
else
    echo "Installing risc0-groth16 via rzup..."
    mkdir -p "$RISC0_DIR"

    # Install rzup if not already available
    if ! command -v rzup &> /dev/null; then
        curl -fSL https://risczero.com/install | bash
        export PATH="$PATH:/root/.risc0/bin"
    fi

    # Install risc0-groth16 to a temp location and copy the artifacts
    RISC0_HOME=$(mktemp -d)
    export RISC0_HOME
    rzup install risc0-groth16

    # Copy just the artifact files
    ARTIFACTS_DIR=$(find "$RISC0_HOME/extensions" -maxdepth 1 -name '*risc0-groth16' -type d | head -1)
    if [ -z "$ARTIFACTS_DIR" ]; then
        echo "ERROR: risc0-groth16 artifacts not found after rzup install"
        exit 1
    fi
    cp "$ARTIFACTS_DIR"/* "$RISC0_DIR/"
    rm -rf "$RISC0_HOME"
    echo "risc0-groth16 artifacts downloaded to $RISC0_DIR"
fi

echo "All proving artifacts ready."
