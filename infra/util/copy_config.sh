#!/bin/bash

# Useful when creating new stacks for new chain deployments

# Check if both arguments are provided
if [ $# -ne 2 ]; then
    echo "Usage: $0 <source_stack> <dest_stack>"
    echo "Example: $0 prod-11155111 prod-8453"
    echo "Note: does not copy ETH_RPC_URL secrets"
    echo "Note: does not copy pulumi:tags"
    exit 1
fi

SOURCE_STACK="$1"
DEST_STACK="$2"

# Get all config keys from source stack
CONFIG_KEYS=$(pulumi config --stack "$SOURCE_STACK" --json | jq -r 'keys[]')

for key in $CONFIG_KEYS; do
    # Skip ETH_RPC_URL secrets
    if [[ "$key" == *"ETH_RPC_URL"* ]]; then
        echo "Skipping key with ETH_RPC_URL: $key"
        continue
    fi

    # also skip pulumi:tags
    if [[ "$key" == *"pulumi:tags"* ]]; then
        echo "Skipping key with pulumi:tags: $key"
        continue
    fi

    # Check if key is a secret
    IS_SECRET=$(pulumi config --stack "$SOURCE_STACK" --json | jq -r --arg key "$key" '.[$key].secret')

    # Get the plain-text value
    VALUE=$(pulumi config get "$key" --stack "$SOURCE_STACK")

    if [[ "$IS_SECRET" == "true" ]]; then
        echo "Copying secret: $key"
        pulumi config set "$key" "$VALUE" --secret --stack "$DEST_STACK"
    else
        echo "Copying non-secret: $key"
        pulumi config set "$key" "$VALUE" --stack "$DEST_STACK"
    fi
done