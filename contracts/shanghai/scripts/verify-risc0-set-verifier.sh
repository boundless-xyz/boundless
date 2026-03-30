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

# Read the set verifier and estop addresses from risc0-verifiers config.
# VERIFIER_SELECTOR must be set (e.g. 0x242f9d5b for RiscZeroSetVerifier).
VERIFIER_ADDRESS=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].risc0-verifiers[] | select(.selector == \"${VERIFIER_SELECTOR:?}\").verifier" $CONTRACTS_DIR/deployment_verifier.toml)
ESTOP_ADDRESS=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].risc0-verifiers[] | select(.selector == \"${VERIFIER_SELECTOR:?}\").estop" $CONTRACTS_DIR/deployment_verifier.toml)

export CHAIN_ID ADMIN_ADDRESS RISC0_ROUTER VERIFIER_ADDRESS ESTOP_ADDRESS

# SET_BUILDER_IMAGE_ID and SET_BUILDER_GUEST_URL must be provided as env vars,
# matching the values used at deployment time.
: "${SET_BUILDER_IMAGE_ID:?SET_BUILDER_IMAGE_ID must be set}"
: "${SET_BUILDER_GUEST_URL:?SET_BUILDER_GUEST_URL must be set}"

# NOTE: forge verify-contract seems to fail if an absolute path is used for the contract address.
cd $REPO_ROOT_DIR

# Run forge build to ensure artifacts are available and built with the right options.
forge build

# Verify the RiscZeroSetVerifier
CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(address,bytes32,string)' \
    ${RISC0_ROUTER:?} \
    ${SET_BUILDER_IMAGE_ID:?} \
    "${SET_BUILDER_GUEST_URL:?}" \
)"
forge verify-contract --watch \
    --chain-id=${CHAIN_ID:?} \
    --constructor-args=${CONSTRUCTOR_ARGS} \
    --etherscan-api-key=${ETHERSCAN_API_KEY:?} \
    ${VERIFIER_ADDRESS:?} \
    lib/risc0-ethereum/contracts/src/RiscZeroSetVerifier.sol:RiscZeroSetVerifier

# Verify the RiscZeroVerifierEmergencyStop wrapping the set verifier
CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(address,address)' \
    ${VERIFIER_ADDRESS:?} \
    ${ADMIN_ADDRESS:?} \
)"
forge verify-contract --watch \
    --chain-id=${CHAIN_ID:?} \
    --constructor-args=${CONSTRUCTOR_ARGS:?} \
    --etherscan-api-key=${ETHERSCAN_API_KEY:?} \
    ${ESTOP_ADDRESS:?} \
    lib/risc0-ethereum/contracts/src/RiscZeroVerifierEmergencyStop.sol:RiscZeroVerifierEmergencyStop
