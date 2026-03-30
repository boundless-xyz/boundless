#!/bin/bash

set -eo pipefail

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
CONTRACTS_DIR="${SCRIPT_DIR:?}/.."
REPO_ROOT_DIR="${SCRIPT_DIR:?}/../.."

if [ -n "$STACK_TAG" ]; then
    DEPLOY_KEY=${CHAIN_KEY:?}-${STACK_TAG:?}
else
    DEPLOY_KEY=${CHAIN_KEY:?}
fi

if [ -z "$ETHERSCAN_API_KEY" ]; then
    echo -n 'ETHERSCAN_API_KEY from deployment_secrets.toml: ' > /dev/stderr
    export ETHERSCAN_API_KEY=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].etherscan-api-key" $CONTRACTS_DIR/deployment_secrets.toml)
else
    echo -n "ETHERSCAN_API_KEY from env $ETHERSCAN_API_KEY"
fi

if [ -z "$RPC_URL" ]; then
    echo -n 'RPC_URL from deployment_secrets.toml: ' > /dev/stderr
    export RPC_URL=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].rpc-url" $CONTRACTS_DIR/deployment_secrets.toml)
else
    echo -n "RPC_URL from env $RPC_URL"
fi

export CHAIN_ID=$(yq eval -e ".deployment.[\"${DEPLOY_KEY:?}\"].id" $CONTRACTS_DIR/deployment-version-registry.toml)
export VERSION_REGISTRY_IMPL=$(yq eval -e ".deployment[\"${DEPLOY_KEY:?}\"].version-registry-impl" $CONTRACTS_DIR/deployment-version-registry.toml)
export VERSION_REGISTRY_PROXY=$(yq eval -e ".deployment[\"${DEPLOY_KEY:?}\"].version-registry" $CONTRACTS_DIR/deployment-version-registry.toml)

echo "Chain ID: ${CHAIN_ID}"
echo "VersionRegistry proxy: ${VERSION_REGISTRY_PROXY}"
echo "VersionRegistry impl: ${VERSION_REGISTRY_IMPL}"

# NOTE: forge verify-contract seems to fail if an absolute path is used for the contract address.
cd $CONTRACTS_DIR

# Run forge build to ensure artifacts are available and built with the right options.
forge build

# Verify the implementation contract (no constructor args — constructor only calls _disableInitializers)
echo "Verifying VersionRegistry implementation at ${VERSION_REGISTRY_IMPL}..."
forge verify-contract --watch \
    --chain-id=${CHAIN_ID:?} \
    --etherscan-api-key=${ETHERSCAN_API_KEY:?} \
    ${VERSION_REGISTRY_IMPL:?} \
    contracts/src/VersionRegistry.sol:VersionRegistry

# Verify the proxy contract
echo "Verifying ERC1967Proxy at ${VERSION_REGISTRY_PROXY}..."
PROXY_CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(address, bytes)' \
    ${VERSION_REGISTRY_IMPL:?} \
    $(cast calldata 'initialize(address)' $(yq eval -e ".deployment[\"${DEPLOY_KEY:?}\"].admin" $CONTRACTS_DIR/deployment-version-registry.toml)) \
)"
forge verify-contract --watch \
    --chain-id=${CHAIN_ID:?} \
    --constructor-args=${PROXY_CONSTRUCTOR_ARGS} \
    --etherscan-api-key=${ETHERSCAN_API_KEY:?} \
    ${VERSION_REGISTRY_PROXY:?} \
    lib/openzeppelin-contracts/contracts/proxy/ERC1967/ERC1967Proxy.sol:ERC1967Proxy

echo "VersionRegistry verification complete!"
