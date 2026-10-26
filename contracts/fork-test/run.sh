#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
report_dir="${SUNSET_REPORT_DIR:-$PWD/out/sunset-fork-reports}"
mkdir -p "$report_dir"
run() {
  local label="$1" profile="$2" rpc="$3" block="$4" contract="$5"
  FOUNDRY_FFI=false FOUNDRY_TEST=contracts/fork-test FOUNDRY_PROFILE="$profile" forge test --offline \
    --out "out/sunset-fork-$label" --cache-path "cache/sunset-fork-$label" \
    --fork-url "$rpc" --fork-block-number "$block" --match-contract "$contract" -vv \
    2>&1 | tee "$report_dir/$label.log"
}
case "${1:-all}" in
  base) run base default "${BASE_RPC_URL:-https://mainnet.base.org}" 53113412 MarketForkTest ;;
  taiko) run taiko shanghai "${TAIKO_RPC_URL:-https://rpc.taiko.xyz}" 13186463 MarketForkTest ;;
  povw) run povw povw-deploy "${ETHEREUM_RPC_URL:-https://ethereum-rpc.publicnode.com}" 26276732 PovwForkTest ;;
  all) "$0" base; "$0" taiko; "$0" povw ;;
  *) echo "Usage: $0 [base|taiko|povw|all]" >&2; exit 2 ;;
esac
