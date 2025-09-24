#!/usr/bin/env bash
set -euo pipefail

# Script to update SHA256 hashes for prebuilt binaries
# This script downloads the binaries and calculates their SHA256 hashes

echo "🔍 Updating SHA256 hashes for prebuilt binaries..."

# Function to get SHA256 hash of a URL
get_hash() {
    local url="$1"
    local temp_file=$(mktemp)

    echo "Downloading $url..."
    curl -L -o "$temp_file" "$url"

    local hash=$(nix-prefetch-url --type sha256 "$url" 2>/dev/null || echo "Failed to get hash")

    rm -f "$temp_file"
    echo "$hash"
}

# Get hashes for different binaries
echo "Getting hash for bento bundle..."
BENTO_HASH=$(get_hash "https://github.com/boundless-xyz/boundless/releases/download/bento-v1.0.1/bento-bundle-linux-amd64.tar.gz")

echo "Getting hash for broker binary..."
BROKER_HASH=$(get_hash "https://github.com/boundless-xyz/boundless/releases/download/broker-v1.0.0/broker")

echo ""
echo "📋 Update these hashes in your package files:"
echo ""
echo "Bento Bundle Hash: $BENTO_HASH"
echo "Broker Hash: $BROKER_HASH"
echo ""

# Update the package files with the correct hashes
if [ "$BENTO_HASH" != "Failed to get hash" ]; then
    echo "Updating package files with correct hashes..."

    # Update manager.nix
    sed -i.bak "s/sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=/$BENTO_HASH/g" packages/manager.nix
    rm -f packages/manager.nix.bak

    # Update prover.nix
    sed -i.bak "s/sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=/$BENTO_HASH/g" packages/prover.nix
    rm -f packages/prover.nix.bak

    # Update execution.nix
    sed -i.bak "s/sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=/$BENTO_HASH/g" packages/execution.nix
    rm -f packages/execution.nix.bak

    # Update aux.nix
    sed -i.bak "s/sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=/$BENTO_HASH/g" packages/aux.nix
    rm -f packages/aux.nix.bak

    echo "✅ Updated manager.nix, prover.nix, execution.nix, and aux.nix with bento bundle hash"
fi

if [ "$BROKER_HASH" != "Failed to get hash" ]; then
    # Update broker.nix
    sed -i.bak "s/sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=/$BROKER_HASH/g" packages/broker.nix
    rm -f packages/broker.nix.bak

    echo "✅ Updated broker.nix with broker hash"
fi

echo ""
echo "🎉 Hash update complete!"
echo ""
echo "You can now build the packages:"
echo "  nix build .#manager"
echo "  nix build .#prover"
echo "  nix build .#execution"
echo "  nix build .#aux"
echo "  nix build .#broker"
