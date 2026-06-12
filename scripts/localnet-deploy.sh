#!/bin/bash
set -e

# Configuration. The defaults target the compose deployer container; the paths/URL are
# env-overridable so the script can also run directly on the host (e.g. on arm64 machines,
# where the amd64-only builder-base image blocks the deployer container build).
ANVIL_RPC="${ANVIL_RPC:-http://anvil:8545}"
DEPLOYER_PRIVATE_KEY="${DEPLOYER_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
CHAIN_KEY="${CHAIN_KEY:-anvil}"
RISC0_DEV_MODE="${RISC0_DEV_MODE:-1}"
BOUNDLESS_MARKET_OWNER="${BOUNDLESS_MARKET_OWNER:-0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266}"
CHAIN_ID="${CHAIN_ID:-31337}"
DEPOSIT_AMOUNT="${DEPOSIT_AMOUNT:-100000000000000000000}"
DEFAULT_ADDRESS="${DEFAULT_ADDRESS:-0x90F79bf6EB2c4f870365E785982E1f101E93b906}"
SHARED_DIR="${SHARED_DIR:-/shared}"
DEPLOYER_ENV="$SHARED_DIR/deployer.env"
HOST_ENV_FILE="${HOST_ENV_FILE:-/host/.env.localnet}"
TEMPLATE_FILE="${TEMPLATE_FILE:-/src/.env.localnet-template}"
ANVIL_PORT=8545

# Generate .env.localnet from template with current addresses.
# A single sed pass into the target (via temp file, since the host file may be bind-mounted);
# avoids `sed -i`, whose syntax differs between GNU and BSD sed (the script also runs on macOS).
generate_env_localnet() {
    local tmpfile
    tmpfile=$(mktemp)
    sed \
        -e "s/^export VERIFIER_ADDRESS=.*/export VERIFIER_ADDRESS=$VERIFIER_ADDRESS/" \
        -e "s/^export SET_VERIFIER_ADDRESS=.*/export SET_VERIFIER_ADDRESS=$SET_VERIFIER_ADDRESS/" \
        -e "s/^export BOUNDLESS_MARKET_ADDRESS=.*/export BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS/" \
        -e "s/^export COLLATERAL_TOKEN_ADDRESS=.*/export COLLATERAL_TOKEN_ADDRESS=$COLLATERAL_TOKEN_ADDRESS/" \
        -e "s|^export RPC_URL=.*|export RPC_URL=\"http://localhost:$ANVIL_PORT\"|" \
        -e "s|^export PROVER_RPC_URL=.*|export PROVER_RPC_URL=\"http://localhost:$ANVIL_PORT\"|" \
        -e "s|^export REQUESTOR_RPC_URL=.*|export REQUESTOR_RPC_URL=\"http://localhost:$ANVIL_PORT\"|" \
        -e "s/^export RISC0_DEV_MODE=.*/export RISC0_DEV_MODE=$RISC0_DEV_MODE/" \
        "$TEMPLATE_FILE" > "$tmpfile"
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

# Deploy the BoundlessRouter first: the market dispatches verification through it,
# so its address must exist before BoundlessMarket is deployed.
echo "Deploying BoundlessRouter..."
ROUTER_ADMIN="${ROUTER_ADMIN:-$BOUNDLESS_MARKET_OWNER}"
DEPLOYER_PRIVATE_KEY="$DEPLOYER_PRIVATE_KEY" \
ROUTER_ADMIN="$ROUTER_ADMIN" \
forge script contracts/scripts/Deploy.Router.s.sol \
    --rpc-url "$ANVIL_RPC" \
    --broadcast -vv || { echo "Failed to deploy BoundlessRouter"; exit 1; }

ROUTER_BROADCAST="./broadcast/Deploy.Router.s.sol/$CHAIN_ID/run-latest.json"
BOUNDLESS_ROUTER=$(jq -re '.transactions[] | select(.contractName == "ERC1967Proxy") | .contractAddress' "$ROUTER_BROADCAST" | head -n 1)
export BOUNDLESS_ROUTER
echo "  BOUNDLESS_ROUTER=$BOUNDLESS_ROUTER"

echo "Deploying contracts..."
DEPLOYER_PRIVATE_KEY="$DEPLOYER_PRIVATE_KEY" \
CHAIN_KEY="$CHAIN_KEY" \
RISC0_DEV_MODE="$RISC0_DEV_MODE" \
BOUNDLESS_MARKET_OWNER="$BOUNDLESS_MARKET_OWNER" \
BOUNDLESS_ROUTER="$BOUNDLESS_ROUTER" \
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

# Configure the BoundlessRouter in one idempotent run: all proof-type classes plus every
# entry this chain serves — the set-inclusion verifier adapter at the set verifier's own
# selector, and both assessor entries (the R0 STARK assessor bound to ASSESSOR_IMAGE_ID at
# 0x00000024 and the native OnChainAssessor at 0x00000022, which brokers prepend to the
# assessor seal). The canonical groth16 / blake3 selectors are skipped here: the dev-mode
# upstream verifiers use dynamic selectors that never match them.
ASSESSOR_IMAGE_ID="0x$(r0vm --id --elf "$ASSESSOR_PATH")"
# The R0 assessor selector as bytes4 right-padded to bytes32, for configs read via
# `bytes4(readBytes32(...))`. Must match RouterConfig.R0_ASSESSOR_SELECTOR.
ASSESSOR_SELECTOR_BYTES32="0x0000002400000000000000000000000000000000000000000000000000000000"

echo "Bootstrapping BoundlessRouter classes and entries..."
DEPLOYER_PRIVATE_KEY="$DEPLOYER_PRIVATE_KEY" \
BOUNDLESS_ROUTER="$BOUNDLESS_ROUTER" \
R0_ROUTER="$VERIFIER_ADDRESS" \
SET_VERIFIER="$SET_VERIFIER_ADDRESS" \
ASSESSOR_IMAGE_ID="$ASSESSOR_IMAGE_ID" \
forge script contracts/scripts/Manage.Router.s.sol:BootstrapRouter \
    --rpc-url "$ANVIL_RPC" \
    --broadcast -vv || { echo "Failed to bootstrap BoundlessRouter"; exit 1; }

echo "Contract deployed at addresses:"
echo "  BOUNDLESS_ROUTER=$BOUNDLESS_ROUTER"
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
BOUNDLESS_ROUTER=$BOUNDLESS_ROUTER
COLLATERAL_TOKEN_ADDRESS=$COLLATERAL_TOKEN_ADDRESS
EOF

# Generate .env.localnet on the host
echo "Generating .env.localnet..."
generate_env_localnet
echo ".env.localnet generated successfully."

# --- CI-specific: create deployment_secrets.toml and update deployment.toml ---
# The contracts/scripts/manage script needs these to run deploy/upgrade/rollback tests.
if [ "${CI:-0}" = "1" ]; then
    echo "CI mode: updating deployment config for manage script..."

    # Create deployment_secrets.toml (host-accessible Anvil URL)
    cat > /src/contracts/deployment_secrets.toml <<EOF
[chains.anvil]
rpc-url = "http://localhost:${ANVIL_PORT}"
etherscan-api-key = "none"
EOF
    echo "Created contracts/deployment_secrets.toml"

    # Use host path for guest URL (manage script runs on the host, not in Docker)
    ASSESSOR_BIN="target/riscv-guest/guest-assessor/assessor-guest/riscv32im-risc0-zkvm-elf/release/assessor-guest.bin"
    if [ -n "${REPO_ROOT:-}" ]; then
        ASSESSOR_GUEST_URL="file://${REPO_ROOT}/${ASSESSOR_BIN}"
    else
        ASSESSOR_GUEST_URL="file://$(realpath "$ASSESSOR_BIN")"
    fi

    # Update deployment.toml with deployed addresses. The assessor image id and selector
    # match the R0 assessor adapter registered in the router above; ASSESSOR_SELECTOR_BYTES32
    # is the bytes4 right-padded to bytes32 so the config's `bytes4(readBytes32(...))` recovers it.
    CHAIN_KEY="${CHAIN_KEY:-anvil}" python3 contracts/update_deployment_toml.py \
        --verifier "$VERIFIER_ADDRESS" \
        --application-verifier "$VERIFIER_ADDRESS" \
        --set-verifier "$SET_VERIFIER_ADDRESS" \
        --boundless-market "$BOUNDLESS_MARKET_ADDRESS" \
        --boundless-router "$BOUNDLESS_ROUTER" \
        --collateral-token "$COLLATERAL_TOKEN_ADDRESS" \
        --assessor-image-id "$ASSESSOR_IMAGE_ID" \
        --assessor-guest-url "$ASSESSOR_GUEST_URL" \
        --r0-assessor-selector "$ASSESSOR_SELECTOR_BYTES32"

    echo "CI deployment config updated."
fi

echo "Deployment complete!"
