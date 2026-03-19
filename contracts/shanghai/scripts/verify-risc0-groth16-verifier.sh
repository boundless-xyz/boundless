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

# Read the groth16 verifier and estop addresses from risc0-verifiers config.
# VERIFIER_SELECTOR must be set (e.g. 0x73c457ba for ZKVM_V3.0).
VERIFIER_ADDRESS=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].risc0-verifiers[] | select(.selector == \"${VERIFIER_SELECTOR:?}\").verifier" $CONTRACTS_DIR/deployment_verifier.toml)
ESTOP_ADDRESS=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].risc0-verifiers[] | select(.selector == \"${VERIFIER_SELECTOR:?}\").estop" $CONTRACTS_DIR/deployment_verifier.toml)

# Read control IDs from the upstream Groth16 verifier library.
CONTROL_ID_FILE="lib/risc0-ethereum/contracts/src/groth16/ControlID.sol"
CONTROL_ROOT=$(grep "CONTROL_ROOT" "$REPO_ROOT_DIR/$CONTROL_ID_FILE" | sed -E 's/.*hex"([^"]+)".*/0x\1/')
BN254_CONTROL_ID=$(grep "BN254_CONTROL_ID" "$REPO_ROOT_DIR/$CONTROL_ID_FILE" | sed -E 's/.*hex"([^"]+)".*/0x\1/')

export CHAIN_ID ADMIN_ADDRESS VERIFIER_ADDRESS ESTOP_ADDRESS CONTROL_ID_FILE CONTROL_ROOT BN254_CONTROL_ID

# NOTE: forge verify-contract seems to fail if an absolute path is used for the contract address.
cd $REPO_ROOT_DIR

# Run forge build to ensure artifacts are available and built with the right options.
forge build

# Verify the RiscZeroGroth16Verifier
CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(bytes32,bytes32)' \
    ${CONTROL_ROOT:?} \
    ${BN254_CONTROL_ID:?} \
)"
forge verify-contract --watch \
    --chain-id=${CHAIN_ID:?} \
    --constructor-args=${CONSTRUCTOR_ARGS} \
    --etherscan-api-key=${ETHERSCAN_API_KEY:?} \
    ${VERIFIER_ADDRESS:?} \
    lib/risc0-ethereum/contracts/src/groth16/RiscZeroGroth16Verifier.sol:RiscZeroGroth16Verifier

# Verify the RiscZeroVerifierEmergencyStop wrapping the Groth16 verifier
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
