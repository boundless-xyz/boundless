#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
: "${SUNSET_BUNDLE:?set SUNSET_BUNDLE to the combined transaction.json}"
: "${ETHEREUM_RPC_URL:?set ETHEREUM_RPC_URL}"
mkdir -p out/sunset-bundle-input
bundle_path="$PWD/out/sunset-bundle-input/transaction.json"
if [[ "$SUNSET_BUNDLE" != "$bundle_path" ]]; then cp "$SUNSET_BUNDLE" "$bundle_path"; fi
export SUNSET_BUNDLE="$bundle_path"
block=$(python3 -c 'import json,os; print(json.load(open(os.environ["SUNSET_BUNDLE"]))["blockNumber"])')
FOUNDRY_PROFILE=povw-deploy FOUNDRY_FFI=false FOUNDRY_TEST=contracts/fork-test \
  FOUNDRY_BUILD_INFO_PATH=out/sunset-bundle/build-info forge test --offline \
  --out out/sunset-bundle --cache-path cache/sunset-bundle --fork-url "$ETHEREUM_RPC_URL" \
  --fork-block-number "$block" --match-contract EthereumBatchForkTest -vv
