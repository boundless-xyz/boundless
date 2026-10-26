#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
report_dir="${SUNSET_REPORT_DIR:-$PWD/out/sunset-fork-reports}"
mkdir -p "$report_dir"
run() {
  local label="$1" profile="$2" rpc="$3" block="$4" contract="$5"
  SUNSET_ARTIFACT_ROOT="out/sunset-fork-$label" FOUNDRY_BUILD_INFO_PATH="out/sunset-fork-$label/build-info" FOUNDRY_FFI=false FOUNDRY_TEST=contracts/fork-test FOUNDRY_PROFILE="$profile" forge test --offline \
    --out "out/sunset-fork-$label" --cache-path "cache/sunset-fork-$label" \
    --fork-url "$rpc" --fork-block-number "${SUNSET_BLOCK_NUMBER:-$block}" --match-contract "$contract" -vv \
    2>&1 | tee "$report_dir/$label.log"
}
case "${1:-all}" in
  base) run base default "${BASE_RPC_URL:-https://mainnet.base.org}" 53113412 MarketForkTest ;;
  taiko) run taiko shanghai "${TAIKO_RPC_URL:-https://rpc.taiko.xyz}" 13186463 MarketForkTest ;;
  povw) run povw povw-deploy "${ETHEREUM_RPC_URL:-https://eth.drpc.org}" 26276732 PovwForkTest ;;
  all)
    if [[ -n "${SUNSET_BLOCK_NUMBER:-}${SUNSET_IMPL:-}${ACCOUNTING_SUNSET_IMPL:-}${MINT_SUNSET_IMPL:-}" ]]; then
      echo "Use base, taiko or povw separately when supplying a block or implementation addresses." >&2
      exit 2
    fi
    "$0" base; "$0" taiko; "$0" povw ;;

  *) echo "Usage: $0 [base|taiko|povw|all]" >&2; exit 2 ;;
esac
