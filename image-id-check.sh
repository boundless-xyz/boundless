#!/bin/bash

set -euo pipefail

# Paths to the files
TOML_FILE="contracts/deployment.toml"
SOLIDITY_ASSESSOR_FILE="contracts/src/AssessorImageID.sol"

# Extract the assessor-image-id for Ethereum Sepolia from the TOML file
SEPOLIA_ASSESSOR_ID=$(grep -A 20 '\[chains.ethereum-sepolia\]' "$TOML_FILE" | grep "assessor-image-id" | awk -F' = ' '{print $2}' | tr -d '"')

echo "Assessor Image ID from TOML: $SEPOLIA_ASSESSOR_ID"

# Extract the ASSESSOR_GUEST_ID values from the Solidity files
SOLIDITY_ASSESSOR_ID=$(grep -A 2 "ASSESSOR_GUEST_ID" "$SOLIDITY_ASSESSOR_FILE" | awk -F'(' '{print $2}' | awk -F')' '{print $1}' | tr -d '[:space:]')

echo "Assessor Guest ID from Solidity: $SOLIDITY_ASSESSOR_ID"

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

exit $STATUS
