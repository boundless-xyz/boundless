#!/bin/bash

set -euo pipefail

# Paths to the files
TOML_FILE="contracts/deployment.toml"
SOLIDITY_ASSESSOR_FILE="contracts/src/AssessorImageID.sol"
SOLIDITY_SET_BUILDER_FILE="contracts/src/SetBuilderImageID.sol"

# Extract the assessor-image-id and set-builder-image-id for Ethereum Sepolia from the TOML file
SEPOLIA_ASSESSOR_ID=$(grep -A 20 '\[chains.ethereum-sepolia\]' "$TOML_FILE" | grep "assessor-image-id" | awk -F' = ' '{print $2}' | tr -d '"')
SEPOLIA_SET_BUILDER_ID=$(grep -A 20 '\[chains.ethereum-sepolia\]' "$TOML_FILE" | grep "set-builder-image-id" | awk -F' = ' '{print $2}' | tr -d '"')

echo "Assessor Image ID from TOML: $SEPOLIA_ASSESSOR_ID"
echo "Set Builder Image ID from TOML: $SEPOLIA_SET_BUILDER_ID"

# Extract the ASSESSOR_GUEST_ID and SET_BUILDER_GUEST_ID values from the Solidity files
SOLIDITY_ASSESSOR_ID=$(grep -A 2 "ASSESSOR_GUEST_ID" "$SOLIDITY_ASSESSOR_FILE" | awk -F'(' '{print $2}' | awk -F')' '{print $1}' | tr -d '[:space:]')
SOLIDITY_SET_BUILDER_IMAGE_ID=$(grep -A 2 "SET_BUILDER_GUEST_ID" "$SOLIDITY_SET_BUILDER_FILE" | awk -F'(' '{print $2}' | awk -F')' '{print $1}' | tr -d '[:space:]')

echo "Assessor Guest ID from Solidity: $SOLIDITY_ASSESSOR_ID"
echo "Set Builder Guest ID from Solidity: $SOLIDITY_SET_BUILDER_IMAGE_ID"

STATUS=0

# Compare the values for Assessor IDs
if [[ "$SEPOLIA_ASSESSOR_ID" == "$SOLIDITY_ASSESSOR_ID" ]]; then
    echo "The ASSESSOR_GUEST_ID matches the assessor-image-id from the Sepolia deployment."
else
    echo "Mismatch found!"
    echo "ASSESSOR_GUEST_ID in Solidity file: $SOLIDITY_ASSESSOR_ID"
    echo "assessor-image-id in TOML file: $SEPOLIA_ASSESSOR_ID"
    STATUS=1
fi

# Compare the values for Set Builder IDs
if [[ "$SEPOLIA_SET_BUILDER_ID" == "$SOLIDITY_SET_BUILDER_IMAGE_ID" ]]; then
    echo "The SET_BUILDER_GUEST_ID matches the set-builder-image-id from the Sepolia deployment."
else
    echo "Mismatch found!"
    echo "SET_BUILDER_GUEST_ID in Solidity file: $SOLIDITY_SET_BUILDER_IMAGE_ID"
    echo "set-builder-image-id in TOML file: $SEPOLIA_SET_BUILDER_ID"
    STATUS=1
fi

exit $STATUS
