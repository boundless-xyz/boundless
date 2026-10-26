#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
case "${1:-}" in
  base) profile=default; rpc="${BASE_RPC_URL:-https://mainnet.base.org}"; target=Market ;;
  taiko) profile=shanghai; rpc="${TAIKO_RPC_URL:-https://rpc.taiko.xyz}"; target=Market ;;
  povw) profile=povw-deploy; rpc="${ETHEREUM_RPC_URL:-https://eth.drpc.org}"; target=Povw ;;
  *) echo "Usage: $0 [base|taiko|povw]" >&2; exit 2 ;;
esac
label="$1"
report_dir="${SUNSET_REPORT_DIR:-$PWD/out/sunset-preparation-$label}"
mkdir -p "$report_dir"
block="${SUNSET_BLOCK_NUMBER:-$(cast block-number --rpc-url "$rpc")}"
SUNSET_ARTIFACT_ROOT="out/sunset-fork-$label" FOUNDRY_BUILD_INFO_PATH="out/sunset-fork-$label/build-info" FOUNDRY_FFI=false FOUNDRY_PROFILE="$profile" \
  forge script "contracts/scripts/sunset/Print${target}Sunset.s.sol:Print${target}Sunset" --offline \
  --out "out/sunset-fork-$label" --cache-path "cache/sunset-fork-$label" --rpc-url "$rpc" --fork-block-number "$block" \
  2>&1 | tee "$report_dir/prepare.log"
python3 contracts/scripts/sunset/simulate.py extract --log "$report_dir/prepare.log" --output "$report_dir/transaction.json"
python3 contracts/scripts/sunset/simulate.py simulate --input "$report_dir/transaction.json" --rpc-url "$rpc" --output "$report_dir/simulation.json"
