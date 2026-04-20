#!/bin/bash

set -eo pipefail

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
CONTRACTS_DIR="${SCRIPT_DIR:?}/../.."
REPO_ROOT_DIR="${SCRIPT_DIR:?}/../../.."

export FOUNDRY_PROFILE=shanghai

if [ -z "$ETHERSCAN_API_KEY" ]; then
    echo -n 'ETHERSCAN_API_KEY from deployment_secrets.toml: ' > /dev/stderr
    ETHERSCAN_API_KEY=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].etherscan-api-key" $CONTRACTS_DIR/deployment_secrets.toml)
    export ETHERSCAN_API_KEY
else
    echo -n "ETHERSCAN_API_KEY from env $ETHERSCAN_API_KEY"
fi

CHAIN_ID=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].id" $CONTRACTS_DIR/deployment_verifier.toml)
ADMIN_ADDRESS=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].admin" $CONTRACTS_DIR/deployment_verifier.toml)
RISC0_ROUTER=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].risc0-router" $CONTRACTS_DIR/deployment_verifier.toml)
RISC0_TIMELOCK_CONTROLLER=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].risc0-timelock-controller" $CONTRACTS_DIR/deployment_verifier.toml)
RISC0_TIMELOCK_DELAY=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].risc0-timelock-delay" $CONTRACTS_DIR/deployment_verifier.toml)

export CHAIN_ID ADMIN_ADDRESS RISC0_ROUTER RISC0_TIMELOCK_CONTROLLER RISC0_TIMELOCK_DELAY

# NOTE: forge verify-contract seems to fail if an absolute path is used for the contract address.
cd $REPO_ROOT_DIR

# Run forge build to ensure artifacts are available and built with the right options.
forge build

# Verify the RiscZeroVerifierRouter
CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(address)' \
    ${RISC0_TIMELOCK_CONTROLLER:?} \
)"
forge verify-contract --watch \
    --chain-id=${CHAIN_ID:?} \
    --constructor-args=${CONSTRUCTOR_ARGS} \
    --etherscan-api-key=${ETHERSCAN_API_KEY:?} \
    ${RISC0_ROUTER:?} \
    contracts/shanghai/src/verifier/RiscZeroVerifierRouter.sol:RiscZeroVerifierRouter

# Verify the TimelockController
CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(uint256,address[],address[],address)' \
    ${RISC0_TIMELOCK_DELAY:?} \
    "[${ADMIN_ADDRESS:?}]" \
    "[${ADMIN_ADDRESS:?}]" \
    ${ADMIN_ADDRESS:?} \
)"
forge verify-contract --watch \
    --chain-id=${CHAIN_ID:?} \
    --constructor-args=${CONSTRUCTOR_ARGS:?} \
    --etherscan-api-key=${ETHERSCAN_API_KEY:?} \
    ${RISC0_TIMELOCK_CONTROLLER:?} \
    lib/openzeppelin-contracts/contracts/governance/TimelockController.sol:TimelockController
