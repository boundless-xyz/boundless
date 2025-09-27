#!/bin/bash

# Script to test running rewards-indexer and API locally with SQLite

set -e

echo "=== Local Indexer Testing Script ==="
echo ""

# Load .env file if it exists
if [ -f .env ]; then
    echo "Loading environment variables from .env file..."
    set -a
    source .env
    set +a
fi

# Configuration
DB_PATH="local_test.db"
DB_URL="sqlite:$DB_PATH"
API_PORT=${PORT:-3005}

# Ethereum mainnet configuration
# Set ETH_RPC_URL environment variable before running this script
if [ -z "$ETH_RPC_URL" ]; then
    echo "Error: ETH_RPC_URL environment variable is not set"
    echo "Please set it to your Ethereum RPC endpoint"
    exit 1
fi
RPC_URL="$ETH_RPC_URL"
VEZKC_ADDRESS="0xE8Ae8eE8ffa57F6a79B6Cbe06BAFc0b05F3ffbf4"
ZKC_ADDRESS="0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555"
POVW_ACCOUNTING_ADDRESS="0x319bd4050b2170a7aE3Ead3E6d5AB8a5c7cFBDF8"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}Configuration:${NC}"
echo "Database: $DB_URL"
echo "API Port: $API_PORT"
echo "RPC URL: $RPC_URL"
echo ""

# Clean up existing database
if [ -f "$DB_PATH" ]; then
    echo -e "${YELLOW}Removing existing database...${NC}"
    rm "$DB_PATH"
fi

# Create empty database file for SQLite
echo -e "${YELLOW}Creating SQLite database file...${NC}"
touch "$DB_PATH"

# Step 1: Build the projects (from repo root)
echo -e "${GREEN}Step 1: Building projects...${NC}"
cd ../../.. || exit 1
cargo build -p boundless-indexer --bin rewards-indexer
cargo build -p indexer-api --bin local-server
cd - > /dev/null || exit 1
echo ""

# Step 2: Run the rewards indexer for a short time to populate data
echo -e "${GREEN}Step 2: Running rewards-indexer to populate data...${NC}"
echo "This will fetch some blockchain data and compute rewards."
echo "Press Ctrl+C after a minute or when you see 'Sleeping for 600 seconds'"
echo ""

# Run the indexer in background with a time limit
DATABASE_URL="$DB_URL" \
VEZKC_ADDRESS="$VEZKC_ADDRESS" \
ZKC_ADDRESS="$ZKC_ADDRESS" \
POVW_ACCOUNTING_ADDRESS="$POVW_ACCOUNTING_ADDRESS" \
RUST_LOG=debug \
../../../target/debug/rewards-indexer \
    --rpc-url "$RPC_URL" \
    --vezkc-address "$VEZKC_ADDRESS" \
    --zkc-address "$ZKC_ADDRESS" \
    --povw-accounting-address "$POVW_ACCOUNTING_ADDRESS" \
    --db "$DB_URL" \
    --interval 600 &

INDEXER_PID=$!

# Wait for 30 seconds or until the indexer starts sleeping
echo "Letting indexer run for 30 seconds to populate data..."
for i in {1..30}; do
    if ! kill -0 $INDEXER_PID 2>/dev/null; then
        echo "Indexer stopped unexpectedly"
        break
    fi
    sleep 1
    echo -n "."
done
echo ""

# Stop the indexer
if kill -0 $INDEXER_PID 2>/dev/null; then
    echo "Stopping indexer..."
    kill $INDEXER_PID 2>/dev/null || true
    wait $INDEXER_PID 2>/dev/null || true
fi

echo ""
echo -e "${GREEN}Data population complete!${NC}"
echo ""

# Step 3: Start the API server
echo -e "${GREEN}Step 3: Starting API server...${NC}"
echo "The API will be available at http://localhost:$API_PORT"
echo ""
echo "Try these endpoints:"
echo "  - http://localhost:$API_PORT/health"
echo "  - http://localhost:$API_PORT/v1/povw"
echo "  - http://localhost:$API_PORT/v1/staking"
echo "  - http://localhost:$API_PORT/v1/povw/epochs/4"
echo "  - http://localhost:$API_PORT/v1/staking/addresses"
echo ""
echo "Press Ctrl+C to stop the server"
echo ""

DB_URL="$DB_URL" \
PORT="$API_PORT" \
RUST_LOG=info \
../../../target/debug/local-server