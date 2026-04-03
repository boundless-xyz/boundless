#!/bin/bash
set -e

SHARED_DIR="/shared"
DEPLOYER_ENV="$SHARED_DIR/deployer.env"

if [ ! -f "$DEPLOYER_ENV" ]; then
    echo "ERROR: $DEPLOYER_ENV not found. Deployer must run first."
    exit 1
fi

echo "Loading contract addresses from $DEPLOYER_ENV..."
source "$DEPLOYER_ENV"

if [ -z "$BOUNDLESS_MARKET_ADDRESS" ]; then
    echo "ERROR: BOUNDLESS_MARKET_ADDRESS not set in $DEPLOYER_ENV"
    exit 1
fi

echo "Starting order-stream with BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS"
exec /app/order_stream \
    --rpc-url http://anvil:8545 \
    --min-balance-raw 0 \
    --bypass-addrs="0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f" \
    --boundless-market-address "$BOUNDLESS_MARKET_ADDRESS"
