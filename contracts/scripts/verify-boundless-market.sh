#!/bin/bash

set -eo pipefail

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
CONTRACTS_DIR="${SCRIPT_DIR:?}/.."

if [ -z "$ETHERSCAN_API_KEY" ]; then
    echo -n 'ETHERSCAN_API_KEY from deployment_secrets.toml: ' > /dev/stderr
    export ETHERSCAN_API_KEY=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].etherscan-api-key" $CONTRACTS_DIR/deployment_secrets.toml)
else
    echo -n "ETHERSCAN_API_KEY from env $ETHERSCAN_API_KEY"
fi

export CHAIN_ID=$(yq eval -e ".deployment.[\"${CHAIN_KEY:?}\"].id" $CONTRACTS_DIR/deployment.toml)
export ADMIN_ADDRESS=$(yq eval -e ".deployment[\"${CHAIN_KEY:?}\"].admin-2" $CONTRACTS_DIR/deployment.toml)
export VERIFIER_ROUTER=$(yq eval -e ".deployment[\"${CHAIN_KEY:?}\"].verifier" $CONTRACTS_DIR/deployment.toml)
export APPLICATION_VERIFIER_ROUTER=$(yq eval -e ".deployment[\"${CHAIN_KEY:?}\"].application-verifier" $CONTRACTS_DIR/deployment.toml)
export BOUNDLESS_MARKET_IMPL=$(yq eval -e ".deployment[\"${CHAIN_KEY:?}\"].boundless-market-impl" $CONTRACTS_DIR/deployment.toml)
export COLLATERAL_TOKEN=$(yq eval -e ".deployment[\"${CHAIN_KEY:?}\"].collateral-token" $CONTRACTS_DIR/deployment.toml)
export ASSESSOR_IMAGE_ID=$(yq eval -e ".deployment[\"${CHAIN_KEY:?}\"].assessor-image-id" $CONTRACTS_DIR/deployment.toml)
export DEPRECATED_ASSESSOR_DURATION=$(yq eval -e ".deployment[\"${CHAIN_KEY:?}\"].deprecated-assessor-duration" $CONTRACTS_DIR/deployment.toml)
export DEPRECATED_ASSESSOR_ID=$(cast call --rpc-url ${RPC_URL:?} ${BOUNDLESS_MARKET_IMPL:?} 'DEPRECATED_ASSESSOR_ID()(bytes32)')

# NOTE: forge verify-contract seems to fail if an absolute path is used for the contract address.
cd $CONTRACTS_DIR

# Run forge build to ensure artifacts are available and built with the right options.
forge build

CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(address, address, bytes32, bytes32, uint32, address)' \
    ${VERIFIER_ROUTER:?} \
    ${APPLICATION_VERIFIER_ROUTER:?} \
    ${ASSESSOR_IMAGE_ID:?} \
    ${DEPRECATED_ASSESSOR_ID:?} \
    ${DEPRECATED_ASSESSOR_DURATION:?} \
    ${COLLATERAL_TOKEN:?} \
)"
forge verify-contract --watch \
    --chain-id=${CHAIN_ID:?} \
    --constructor-args=${CONSTRUCTOR_ARGS} \
    --etherscan-api-key=${ETHERSCAN_API_KEY:?} \
    ${BOUNDLESS_MARKET_IMPL:?} \
    contracts/src/BoundlessMarket.sol:BoundlessMarket
