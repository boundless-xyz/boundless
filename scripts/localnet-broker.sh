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

export BOUNDLESS_MARKET_ADDRESS
export SET_VERIFIER_ADDRESS

echo "Starting broker with BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS"
exec /app/broker --config-file /app/broker.toml
