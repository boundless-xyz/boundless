#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
: "${ACCOUNTING_SUNSET_IMPL:?set ACCOUNTING_SUNSET_IMPL}"
: "${MINT_SUNSET_IMPL:?set MINT_SUNSET_IMPL}"
: "${ZKC_SUNSET_IMPL:?set ZKC_SUNSET_IMPL}"
zkc_repo="${ZKC_REPO:-$PWD/../zkc}"
[[ -x "$zkc_repo/script/sunset/prepare.sh" ]] || { echo "Set ZKC_REPO to the local ZKC sunset checkout" >&2; exit 2; }
rpc="${ETHEREUM_RPC_URL:-https://eth.drpc.org}"
report_dir="${SUNSET_REPORT_DIR:-$PWD/out/sunset-ethereum-bundle}"
report_dir=$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$report_dir")
mkdir -p "$report_dir"
trap 'echo "Preparation failed; inspect $report_dir/*.log" >&2' ERR
export ETHEREUM_RPC_URL="$rpc"
export SUNSET_BLOCK_NUMBER="${SUNSET_BLOCK_NUMBER:-$(cast block-number --rpc-url "$rpc")}"
# Rebuild and verify both repository outputs at the exact same block. Sign neither intermediate file.
SUNSET_REPORT_DIR="$report_dir/povw" contracts/scripts/sunset/prepare.sh povw > "$report_dir/povw.log" 2>&1
SUNSET_IMPL="$ZKC_SUNSET_IMPL" SUNSET_REPORT_DIR="$report_dir/zkc" "$zkc_repo/script/sunset/prepare.sh" > "$report_dir/zkc.log" 2>&1
python3 contracts/scripts/sunset/bundle_ethereum.py --povw "$report_dir/povw/transaction.json" \
  --zkc "$report_dir/zkc/transaction.json" --rpc-url "$rpc" --output-dir "$report_dir/unverified" > "$report_dir/combine.log" 2>&1
SUNSET_BUNDLE="$report_dir/unverified/transaction.json" contracts/scripts/sunset/test-bundle.sh > "$report_dir/batch-tests.log" 2>&1
mv "$report_dir/unverified/transaction.json" "$report_dir/transaction.json"
mv "$report_dir/unverified/simulation.json" "$report_dir/simulation.json"
printf 'Atomic batch tests passed. Final Safe payload: %s/transaction.json\n' "$report_dir"
