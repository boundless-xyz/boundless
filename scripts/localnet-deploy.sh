#!/bin/bash
set -e

# Configuration
ANVIL_RPC="http://anvil:8545"
DEPLOYER_PRIVATE_KEY="${DEPLOYER_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
CHAIN_KEY="${CHAIN_KEY:-anvil}"
RISC0_DEV_MODE="${RISC0_DEV_MODE:-1}"
BOUNDLESS_MARKET_OWNER="${BOUNDLESS_MARKET_OWNER:-0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266}"
CHAIN_ID="${CHAIN_ID:-31337}"
DEPOSIT_AMOUNT="${DEPOSIT_AMOUNT:-100000000000000000000}"
DEFAULT_ADDRESS="${DEFAULT_ADDRESS:-0x90F79bf6EB2c4f870365E785982E1f101E93b906}"
SHARED_DIR="/shared"
DEPLOYER_ENV="$SHARED_DIR/deployer.env"
HOST_ENV_FILE="/host/.env.localnet"
TEMPLATE_FILE="/src/.env.localnet-template"
ANVIL_PORT=8545

# Generate .env.localnet from template with current addresses.
# Uses a temp file because sed -i doesn't work on bind-mounted files.
generate_env_localnet() {
    local tmpfile
    tmpfile=$(mktemp)
    cp "$TEMPLATE_FILE" "$tmpfile"
    sed -i "s/^export VERIFIER_ADDRESS=.*/export VERIFIER_ADDRESS=$VERIFIER_ADDRESS/" "$tmpfile"
    sed -i "s/^export SET_VERIFIER_ADDRESS=.*/export SET_VERIFIER_ADDRESS=$SET_VERIFIER_ADDRESS/" "$tmpfile"
    sed -i "s/^export BOUNDLESS_MARKET_ADDRESS=.*/export BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS/" "$tmpfile"
    sed -i "s/^export COLLATERAL_TOKEN_ADDRESS=.*/export COLLATERAL_TOKEN_ADDRESS=$COLLATERAL_TOKEN_ADDRESS/" "$tmpfile"
    sed -i "s|^export RPC_URL=.*|export RPC_URL=\"http://localhost:$ANVIL_PORT\"|" "$tmpfile"
    sed -i "s|^export PROVER_RPC_URL=.*|export PROVER_RPC_URL=\"http://localhost:$ANVIL_PORT\"|" "$tmpfile"
    sed -i "s|^export REQUESTOR_RPC_URL=.*|export REQUESTOR_RPC_URL=\"http://localhost:$ANVIL_PORT\"|" "$tmpfile"
    sed -i "s/^export RISC0_DEV_MODE=.*/export RISC0_DEV_MODE=$RISC0_DEV_MODE/" "$tmpfile"
    cat "$tmpfile" > "$HOST_ENV_FILE"
    rm "$tmpfile"
}

# --- Skip-if-deployed check ---
if [ -f "$DEPLOYER_ENV" ]; then
    echo "Found existing deployer.env, checking if contracts are still deployed..."
    source "$DEPLOYER_ENV"
    if [ -n "$BOUNDLESS_MARKET_ADDRESS" ]; then
        MARKET_CODE=$(cast code "$BOUNDLESS_MARKET_ADDRESS" --rpc-url "$ANVIL_RPC" 2>/dev/null || echo "0x")
        if [ "$MARKET_CODE" != "0x" ] && [ ${#MARKET_CODE} -gt 4 ]; then
            echo "Contracts still deployed at $BOUNDLESS_MARKET_ADDRESS. Skipping deployment."
            generate_env_localnet
            echo ".env.localnet regenerated from saved addresses."
            exit 0
        else
            echo "Contracts not found at saved address. Proceeding with fresh deployment."
        fi
    fi
fi

# --- Full deployment ---

# Verify guest binaries exist
ASSESSOR_PATH="target/riscv-guest/guest-assessor/assessor-guest/riscv32im-risc0-zkvm-elf/release/assessor-guest.bin"
SET_BUILDER_PATH="target/riscv-guest/guest-set-builder/set-builder/riscv32im-risc0-zkvm-elf/release/set-builder.bin"
if [ ! -f "$ASSESSOR_PATH" ]; then
    echo "ERROR: Guest binary not found at $ASSESSOR_PATH"
    echo "Please build guest binaries on the host first: cargo build"
    exit 1
fi
if [ ! -f "$SET_BUILDER_PATH" ]; then
    echo "ERROR: Guest binary not found at $SET_BUILDER_PATH"
    echo "Please build guest binaries on the host first: cargo build"
    exit 1
fi

# Upload guest binaries to MinIO if configured
MINIO_URL="${MINIO_URL:-}"
MINIO_HOST_URL="${MINIO_HOST_URL:-}"
MINIO_BUCKET="${MINIO_BUCKET:-localnet-guests}"
if [ -n "$MINIO_URL" ]; then
    echo "Uploading guest binaries to MinIO..."
    mc alias set localnet "$MINIO_URL" "${MINIO_ROOT_USER:-admin}" "${MINIO_ROOT_PASSWORD:-password}" --api S3v4 2>/dev/null

    mc cp "$ASSESSOR_PATH" "localnet/$MINIO_BUCKET/assessor-guest.bin"
    mc cp "$SET_BUILDER_PATH" "localnet/$MINIO_BUCKET/set-builder.bin"

    # Use host-accessible MinIO URL for guest URLs stored in contracts
    export ASSESSOR_GUEST_URL="${MINIO_HOST_URL}/${MINIO_BUCKET}/assessor-guest.bin"
    export SET_BUILDER_GUEST_URL="${MINIO_HOST_URL}/${MINIO_BUCKET}/set-builder.bin"
    echo "Guest binaries uploaded. URLs:"
    echo "  Assessor:    $ASSESSOR_GUEST_URL"
    echo "  Set Builder: $SET_BUILDER_GUEST_URL"
fi

# Use pre-built artifacts from host if available, otherwise build
if [ -d "out" ] && [ "$(ls -A out 2>/dev/null)" ]; then
    echo "Using pre-built contract artifacts from host..."
else
    echo "Building contracts..."
    forge build || { echo "Failed to build contracts"; exit 1; }
fi

echo "Deploying contracts..."
DEPLOYER_PRIVATE_KEY="$DEPLOYER_PRIVATE_KEY" \
CHAIN_KEY="$CHAIN_KEY" \
RISC0_DEV_MODE="$RISC0_DEV_MODE" \
BOUNDLESS_MARKET_OWNER="$BOUNDLESS_MARKET_OWNER" \
ASSESSOR_GUEST_URL="${ASSESSOR_GUEST_URL:-}" \
SET_BUILDER_GUEST_URL="${SET_BUILDER_GUEST_URL:-}" \
forge script contracts/scripts/Deploy.s.sol \
    --rpc-url "$ANVIL_RPC" \
    --broadcast -vv || { echo "Failed to deploy contracts"; exit 1; }

echo "Fetching contract addresses..."
BROADCAST_FILE="./broadcast/Deploy.s.sol/$CHAIN_ID/run-latest.json"
VERIFIER_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "RiscZeroVerifierRouter") | .contractAddress' "$BROADCAST_FILE" | head -n 1)
SET_VERIFIER_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "RiscZeroSetVerifier") | .contractAddress' "$BROADCAST_FILE" | head -n 1)
BOUNDLESS_MARKET_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "ERC1967Proxy") | .contractAddress' "$BROADCAST_FILE" | head -n 1)

# Extract collateral token from deployment.toml or fallback to broadcast JSON
COLLATERAL_TOKEN_ADDRESS=$(grep -A 20 '\[deployment.anvil\]' contracts/deployment.toml | grep '^collateral-token' | sed 's/.*= *"\([^"]*\)".*/\1/' | tr -d ' ')
if [ -z "$COLLATERAL_TOKEN_ADDRESS" ] || [ "$COLLATERAL_TOKEN_ADDRESS" = "0x0000000000000000000000000000000000000000" ]; then
    COLLATERAL_TOKEN_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "HitPoints") | .contractAddress' "$BROADCAST_FILE" 2>/dev/null | head -n 1 || echo "")
fi

echo "Contract deployed at addresses:"
echo "  VERIFIER_ADDRESS=$VERIFIER_ADDRESS"
echo "  SET_VERIFIER_ADDRESS=$SET_VERIFIER_ADDRESS"
echo "  BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS"
echo "  COLLATERAL_TOKEN_ADDRESS=$COLLATERAL_TOKEN_ADDRESS"

# Mint HP tokens to the default prover address and deposit collateral
DEFAULT_PRIVATE_KEY="0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"
if [ -n "$COLLATERAL_TOKEN_ADDRESS" ] && [ "$COLLATERAL_TOKEN_ADDRESS" != "0x0000000000000000000000000000000000000000" ]; then
    echo "Minting HP for prover address $DEFAULT_ADDRESS..."
    cast send --private-key "$DEPLOYER_PRIVATE_KEY" \
        --rpc-url "$ANVIL_RPC" \
        "$COLLATERAL_TOKEN_ADDRESS" "mint(address, uint256)" "$DEFAULT_ADDRESS" "$DEPOSIT_AMOUNT" || \
        echo "Warning: HP mint failed (non-critical)"

    echo "Approving and depositing collateral..."
    cast send --private-key "$DEFAULT_PRIVATE_KEY" \
        --rpc-url "$ANVIL_RPC" \
        "$COLLATERAL_TOKEN_ADDRESS" "approve(address, uint256)" "$BOUNDLESS_MARKET_ADDRESS" "$DEPOSIT_AMOUNT" || \
        echo "Warning: HP approve failed (non-critical)"
    cast send --private-key "$DEFAULT_PRIVATE_KEY" \
        --rpc-url "$ANVIL_RPC" \
        "$BOUNDLESS_MARKET_ADDRESS" "depositCollateral(uint256)" "$DEPOSIT_AMOUNT" || \
        echo "Warning: Collateral deposit failed (non-critical)"
else
    echo "Warning: COLLATERAL_TOKEN_ADDRESS not found. Skipping HP mint and deposit."
fi

# Write deployer.env to shared volume
echo "Writing deployer.env..."
cat > "$DEPLOYER_ENV" <<EOF
VERIFIER_ADDRESS=$VERIFIER_ADDRESS
SET_VERIFIER_ADDRESS=$SET_VERIFIER_ADDRESS
BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS
COLLATERAL_TOKEN_ADDRESS=$COLLATERAL_TOKEN_ADDRESS
EOF

# Generate .env.localnet on the host
echo "Generating .env.localnet..."
generate_env_localnet
echo ".env.localnet generated successfully."

echo "Deployment complete!"
