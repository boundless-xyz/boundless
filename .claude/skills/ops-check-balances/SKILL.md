---
name: ops-check-balances
description: Internal — for Boundless team members only. Audit native ETH, market deposit, prover collateral, and distributor ZKC reserve balances for every operator-managed address (provers, distributor, order generators, signal). Use when the user wants to know which addresses need topping up, asks about the balance of provers/OGs/distributor/signal signers, says something is "running low" or "out of gas", or wants a periodic operational health check on operator wallets. Defaults to prod env (mainnets + prod testnets) — pass `--all` to also include staging.
---

# Check Operator Balances

Audit balances on every Boundless-operated address: native ETH, market deposit (`balanceOf`), prover collateral (`balanceOfCollateral`), and the distributor's bridged-ZKC ERC20 reserve.

The list of addresses, per-role thresholds, and which checks apply (gas vs market deposit) are loaded at runtime from the **runbooks** repo. The skill itself contains only public protocol data (chain → market/collateral-token addresses, public RPCs).

## Prerequisites

1. **CLI tools**: `cast` (from [foundry](https://book.getfoundry.sh/)), `jq`, `python3`, `curl`, and standard Unix utilities. `python3` is used for wei → human-readable conversion (`awk` silently truncates large integers) and for parsing `deployment.toml`. `curl` fetches the contract registry from the public boundless repo.

2. **`gh` CLI authenticated against boundless-xyz** (required). The canonical address list and thresholds live in the private `boundless-xyz/runbooks` repo and are fetched at runtime via `gh api` — no local clone needed. Verify with:
   ```bash
   gh api -H "Accept: application/vnd.github.raw" \
     repos/boundless-xyz/runbooks/contents/addresses/operator_addresses.json | head -5
   ```
   If that fails with a 401/403, run `gh auth login` and ensure your account has access to `boundless-xyz/runbooks`.

3. **`network_secrets.toml`** at the repo root (optional). Public RPCs are usually sufficient. When a balance comes back as exactly `0` it may be rate-limited rather than actually empty — in that case retry against the private QuikNode endpoint from `[networks.<chain>.rpc]`. The runbook has setup instructions.

## What to check

For each address in `operator_addresses.json`, on every chain it operates on:

1. **Native ETH balance** (when `needs_gas: true`) — `cast balance --rpc-url <RPC> <addr>`. Required to pay gas.
2. **Market deposit** (when `needs_market_check: true`) — `cast call <market> "balanceOf(address)(uint256)" <addr>`. Funds available to pay for orders (requestor) or to claim as rewards (prover).
3. **Prover collateral** (when `role: prover`) — `cast call <market> "balanceOfCollateral(address)(uint256)" <addr>`. Stake the prover has put up.
4. **Distributor ZKC reserve** (when `role: distributor`) — `cast call <collateral_token> "balanceOf(address)(uint256)" <distributor_addr>`. The distributor's raw ERC20 balance of bridged ZKC, used to top up provers' collateral. **Not the same as `balanceOfCollateral`** — that's deposited stake; this is the unstaked reserve.

## Thresholds

Loaded from `operator_addresses.json`'s `thresholds` block. Resolved field-by-field with priority:

```
per-entry override → by_role[role] → by_chain[chain] → default
```

ETH thresholds for **distributor-managed roles** (provers, OGs) come from `by_chain` because the distributor's `ETH_THRESHOLD` varies by ~2 orders of magnitude across chains (0.008 ETH on Taiko vs 1 ETH on Base Sepolia staging). Audit thresholds are tied to those values: WARN ≈ `0.5 × ETH_THRESHOLD` (distributor missed at least one cycle), CRIT ≈ `0.125 × ETH_THRESHOLD` (auto-top-up clearly broken).

ETH thresholds for **non-distributor-managed roles** (`distributor` itself, `signal` signers) come from `by_role` and ignore the chain. Distributor's WARN matches the operational `DISTRIBUTOR_ETH_ALERT_THRESHOLD` of 0.5; signal signers use a smaller threshold tuned to their ~0.0001 ETH/day burn.

ZKC thresholds (prover collateral, distributor reserve) come from `by_role.prover` and `by_role.distributor` respectively, since they're uniform across chains.

`eth_warn`/`eth_crit` gate the **native-ETH gas** balance. To alert on an address's **market deposit** (`balanceOf`) instead of (or in addition to) gas, set `deposit_warn`/`deposit_crit` (ETH-equivalent units). These are **opt-in**: there is no default, so the script flags a deposit (`DEPOSIT-LOW` / `DEPOSIT-CRIT`) only for entries that resolve a deposit threshold. Used for requestors whose deposit is the operational signal (e.g. KOG, deposit WARN 1 / CRIT 0.1, while its gas stays on the base-mainnet default).

The resolved values the skill consumes live in `operator_addresses.json`'s `thresholds` block. The runbook README no longer mirrors a values table — the operational source of truth is the distributor Pulumi config in the boundless repo (`infra/distributor/Pulumi.l-prod-{chain_id}.yaml` / `Pulumi.l-staging-{chain_id}.yaml`, keys like `ETH_THRESHOLD` / `DISTRIBUTOR_ETH_ALERT_THRESHOLD`). The `deposit_warn`/`deposit_crit` overrides are runbook-local and exist only in `operator_addresses.json`.

A specific entry can override any field by setting a `thresholds: { eth_warn: ..., ... }` block on it — only the fields you set are overridden; the rest fall through.

## Public chain → contract addresses

Contract addresses (market, collateral token) are **fetched at runtime** from the public `contracts/deployment.toml` in the boundless repo on GitHub. The skill never hardcodes them — when a chain is added or addresses rotate, it's a single edit to `deployment.toml` and the skill picks it up on next run, no code change.

```bash
DEPLOYMENT_TOML_URL="https://raw.githubusercontent.com/boundless-xyz/boundless/main/contracts/deployment.toml"
```

The chain label used in `operator_addresses.json` must match a `[deployment.<label>]` section in `deployment.toml`. Today that's `base-mainnet`, `taiko-mainnet`, `base-sepolia-staging`, `taiko-staging` (the prod testnets `base-sepolia` and `ethereum-sepolia` were discontinued).

The only inline data the skill keeps is the **chain label → public RPC URL** mapping below — RPC URLs aren't in `deployment.toml` and don't rotate.

| Chain label            | Public RPC                                    |
| ---------------------- | --------------------------------------------- |
| `base-mainnet`         | `https://mainnet.base.org`                    |
| `taiko-mainnet`        | `https://rpc.taiko.xyz`                       |
| `base-sepolia-staging` | `https://sepolia.base.org`                    |
| `taiko-staging`        | `https://rpc.taiko.xyz`                       |

## The check script

Save as `/tmp/balance_check.sh` and run. Defaults to **prod env** (mainnets + prod testnets); pass `--all` to also include staging.

```bash
#!/usr/bin/env bash
set -euo pipefail

# Fetch the operator address registry directly from the private runbooks repo
# via gh CLI (no local clone needed). RUNBOOKS_REF can be set to fetch from a
# specific branch/tag/sha (default: main).
RUNBOOKS_REF="${RUNBOOKS_REF:-main}"
ADDRS_FILE=$(mktemp)
trap 'rm -f "$ADDRS_FILE" "${DEPLOYMENT_TOML:-}"' EXIT
if ! gh api -H "Accept: application/vnd.github.raw" \
       "repos/boundless-xyz/runbooks/contents/addresses/operator_addresses.json?ref=$RUNBOOKS_REF" \
       > "$ADDRS_FILE" 2>/dev/null; then
  echo "ERROR: failed to fetch operator_addresses.json from boundless-xyz/runbooks" >&2
  echo "       Run 'gh auth login' and ensure your account has access to that repo." >&2
  exit 1
fi

# Default: prod only. --all also includes staging.
INCLUDE_STAGING="${INCLUDE_STAGING:-false}"
[ "${1:-}" = "--all" ] && INCLUDE_STAGING=true

# Public RPC URLs (the only inline data — they're not in deployment.toml and
# don't rotate). Function-based for macOS bash 3.2 compatibility.
chain_rpc() {
  case "$1" in
    base-mainnet)           echo "https://mainnet.base.org" ;;
    taiko-mainnet)          echo "https://rpc.taiko.xyz" ;;
    base-sepolia-staging)   echo "https://sepolia.base.org" ;;
    taiko-staging)          echo "https://rpc.taiko.xyz" ;;
    *) echo "" ;;
  esac
}

# Fetch the canonical contract address registry from the public boundless repo
# and build a chain_label -> "market|collateral" map. Keeps the skill self-updating
# when contracts/deployment.toml changes.
DEPLOYMENT_TOML_URL="${DEPLOYMENT_TOML_URL:-https://raw.githubusercontent.com/boundless-xyz/boundless/main/contracts/deployment.toml}"
DEPLOYMENT_TOML=$(mktemp)
if ! curl -fsSL "$DEPLOYMENT_TOML_URL" -o "$DEPLOYMENT_TOML"; then
  echo "ERROR: failed to fetch $DEPLOYMENT_TOML_URL" >&2
  exit 1
fi

# Build a flat "label\tmarket\tcollateral" table by parsing the TOML once.
CHAIN_CONTRACTS=$(python3 - "$DEPLOYMENT_TOML" <<'PY'
import re, sys
t = open(sys.argv[1]).read()
zero = "0x0000000000000000000000000000000000000000"
for s in re.split(r'\n(?=\[deployment\.)', t):
    m = re.match(r'\[deployment\.([^\]]+)\]', s)
    if not m: continue
    name = m.group(1)
    market = re.search(r'^boundless-market\s*=\s*"(0x[a-fA-F0-9]{40})"', s, re.MULTILINE)
    coll   = re.search(r'^collateral-token\s*=\s*"(0x[a-fA-F0-9]{40})"', s, re.MULTILINE)
    if not market or market.group(1).lower() == zero: continue
    coll_addr = coll.group(1) if coll and coll.group(1).lower() != zero else ""
    print(f"{name}\t{market.group(1)}\t{coll_addr}")
PY
)

# label -> "market|collateral" lookup using the fetched data.
chain_contracts() {
  echo "$CHAIN_CONTRACTS" | awk -v c="$1" -F'\t' '$1==c {print $2"|"$3; exit}'
}

decode_uint() { echo "$1" | head -1 | awk '{print $1}' | grep -oE '^[0-9]+'; }
fmt_eth()     { python3 -c "v='${1:-}'; print(f'{int(v)/1e18:.6f}' if v else '?')" 2>/dev/null; }
fmt_zkc()     { python3 -c "v='${1:-}'; print(f'{int(v)/1e18:.4f}' if v else '?')" 2>/dev/null; }
flt_lt()      { python3 -c "import sys; sys.exit(0 if float('$1') < float('$2') else 1)" 2>/dev/null; }

# Build (entry × chain) tuples filtered by env, into TSV:
# label \t address \t role \t chain \t needs_gas \t needs_market \t eth_warn \t eth_crit \t deposit_warn \t deposit_crit
#
# Threshold resolution (field-by-field): per-entry override → by_role[role] → by_chain[chain] → default.
# `//` works correctly here because thresholds are numbers, not booleans (no false-collapse risk).
# But for the boolean flags `needs_gas` / `needs_market_check`, `//` would mistreat `false`,
# so we use `has()` for those.
# eth_* gate native ETH (gas). deposit_* gate the market balanceOf and are OPT-IN: no default,
# so they resolve to "" when unset and the consumer skips deposit flagging for that entry.
TUPLES=$(jq -r --arg incl "$INCLUDE_STAGING" '
  . as $root
  | .addresses[]
  | . as $e
  | select($incl == "true" or .env == "prod" or .env == "all")
  | .chains[] as $c
  | (
      (($e.thresholds // {}).eth_warn)
      // (($root.thresholds.by_role[$e.role] // {}).eth_warn)
      // (($root.thresholds.by_chain[$c]    // {}).eth_warn)
      // ($root.thresholds.default.eth_warn)
      // 0.02
    ) as $eth_warn
  | (
      (($e.thresholds // {}).eth_crit)
      // (($root.thresholds.by_role[$e.role] // {}).eth_crit)
      // (($root.thresholds.by_chain[$c]    // {}).eth_crit)
      // ($root.thresholds.default.eth_crit)
      // 0.005
    ) as $eth_crit
  | (
      (($e.thresholds // {}).deposit_warn)
      // (($root.thresholds.by_role[$e.role] // {}).deposit_warn)
      // (($root.thresholds.by_chain[$c]    // {}).deposit_warn)
      // ($root.thresholds.default.deposit_warn)
      // ""
    ) as $dep_warn
  | (
      (($e.thresholds // {}).deposit_crit)
      // (($root.thresholds.by_role[$e.role] // {}).deposit_crit)
      // (($root.thresholds.by_chain[$c]    // {}).deposit_crit)
      // ($root.thresholds.default.deposit_crit)
      // ""
    ) as $dep_crit
  | [.label, .address, .role, $c,
     (if has("needs_gas") then .needs_gas else true end),
     (if has("needs_market_check") then .needs_market_check else false end),
     $eth_warn, $eth_crit, $dep_warn, $dep_crit]
  | @tsv
' "$ADDRS_FILE")

printf "%-36s %-44s %-12s %-22s %-12s %-13s %-12s %s\n" SERVICE ADDRESS ROLE CHAIN ETH DEPOSIT COLLATERAL FLAG
echo "--------------------------------------------------------------------------------------------------------------------------------------------------------------------"

while IFS=$'\t' read -r LABEL ADDR ROLE CHAIN NEEDS_GAS NEEDS_MARKET ETH_WARN ETH_CRIT DEP_WARN DEP_CRIT; do
  RPC=$(chain_rpc "$CHAIN")
  CHAIN_INFO=$(chain_contracts "$CHAIN")
  if [ -z "$RPC" ] || [ -z "$CHAIN_INFO" ]; then
    printf "%-36s %-44s %-12s %-22s %s\n" "$LABEL" "$ADDR" "$ROLE" "$CHAIN" "(chain not found — RPC or deployment.toml entry missing)"
    continue
  fi
  MKT="${CHAIN_INFO%%|*}"

  E="-"; D="-"; C="-"
  FLAG=""

  if [ "$NEEDS_GAS" = "true" ]; then
    E_RAW=$(cast balance --rpc-url "$RPC" "$ADDR" 2>/dev/null || true)
    E=$(fmt_eth "$E_RAW")
    if [ "$E" != "?" ] && [ "$E" != "-" ]; then
      if flt_lt "$E" "$ETH_CRIT"; then FLAG="$FLAG ETH-CRIT"
      elif flt_lt "$E" "$ETH_WARN"; then FLAG="$FLAG ETH-LOW"
      fi
    fi
  fi

  if [ "$NEEDS_MARKET" = "true" ] && [ -n "$MKT" ]; then
    D_RAW=$(decode_uint "$(cast call --rpc-url "$RPC" "$MKT" 'balanceOf(address)(uint256)' "$ADDR" 2>/dev/null || true)")
    D=$(fmt_eth "$D_RAW")
    # Market-deposit thresholds are opt-in (no default); flag only when resolved for this entry.
    if [ "$D" != "?" ] && [ "$D" != "-" ]; then
      if [ -n "$DEP_CRIT" ] && flt_lt "$D" "$DEP_CRIT"; then FLAG="$FLAG DEPOSIT-CRIT"
      elif [ -n "$DEP_WARN" ] && flt_lt "$D" "$DEP_WARN"; then FLAG="$FLAG DEPOSIT-LOW"
      fi
    fi
  fi

  if [ "$ROLE" = "prover" ] && [ -n "$MKT" ]; then
    C_RAW=$(decode_uint "$(cast call --rpc-url "$RPC" "$MKT" 'balanceOfCollateral(address)(uint256)' "$ADDR" 2>/dev/null || true)")
    C=$(fmt_zkc "$C_RAW")
    # Collateral thresholds are uniform; pulled from by_role.prover (or default).
    C_WARN=$(jq -r '.thresholds.by_role.prover.collateral_warn // .thresholds.default.collateral_warn // 100' "$ADDRS_FILE")
    C_CRIT=$(jq -r '.thresholds.by_role.prover.collateral_crit // .thresholds.default.collateral_crit // 50'  "$ADDRS_FILE")
    if [ "$C" != "?" ] && [ "$C" != "-" ]; then
      if flt_lt "$C" "$C_CRIT"; then FLAG="$FLAG ZKC-CRIT"
      elif flt_lt "$C" "$C_WARN"; then FLAG="$FLAG ZKC-LOW"
      fi
    fi
  fi

  printf "%-36s %-44s %-12s %-22s %-12s %-13s %-12s%s\n" "$LABEL" "$ADDR" "$ROLE" "$CHAIN" "$E" "$D" "$C" "$FLAG"
done <<<"$TUPLES"

# Distributor bridged-ZKC reserve (one row per (distributor address × its chains))
echo
echo "=== Distributor bridged-ZKC ERC20 reserve ==="
printf "%-36s %-44s %-22s %-14s %s\n" DISTRIBUTOR ADDRESS CHAIN ZKC_RESERVE FLAG
echo "------------------------------------------------------------------------------------------------------------------------"

DISTRIB_TUPLES=$(jq -r --arg incl "$INCLUDE_STAGING" '
  .addresses[]
  | select(.role == "distributor")
  | select($incl == "true" or .env == "prod" or .env == "all")
  | .chains[] as $c
  | [.label, .address, $c] | @tsv
' "$ADDRS_FILE")

while IFS=$'\t' read -r LABEL ADDR CHAIN; do
  RPC=$(chain_rpc "$CHAIN")
  CHAIN_INFO=$(chain_contracts "$CHAIN")
  if [ -z "$RPC" ] || [ -z "$CHAIN_INFO" ]; then continue; fi
  TOKEN="${CHAIN_INFO##*|}"
  if [ -z "$TOKEN" ]; then continue; fi
  RAW=$(decode_uint "$(cast call --rpc-url "$RPC" "$TOKEN" 'balanceOf(address)(uint256)' "$ADDR" 2>/dev/null || true)")
  ZKC=$(fmt_zkc "$RAW")
  R_WARN=$(jq -r '.thresholds.by_role.distributor.reserve_zkc_warn // .thresholds.default.reserve_zkc_warn // 100' "$ADDRS_FILE")
  R_CRIT=$(jq -r '.thresholds.by_role.distributor.reserve_zkc_crit // .thresholds.default.reserve_zkc_crit // 50'  "$ADDRS_FILE")
  FLAG=""
  if [ "$ZKC" != "?" ] && [ "$ZKC" != "-" ]; then
    if flt_lt "$ZKC" "$R_CRIT"; then FLAG="ZKC-CRIT"
    elif flt_lt "$ZKC" "$R_WARN"; then FLAG="ZKC-LOW"
    fi
  fi
  printf "%-36s %-44s %-22s %-14s %s\n" "$LABEL" "$ADDR" "$CHAIN" "$ZKC" "$FLAG"
done <<<"$DISTRIB_TUPLES"
```

## Output format

Render the script's output as **two separate markdown tables grouped by environment tier**, in this order so operators can scan the highest-stakes rows first:

1. **Prod mainnet** — chains `base-mainnet`, `taiko-mainnet`. Real funds at stake; flagged rows here matter most.
2. **Staging** — chains `base-sepolia-staging`, `taiko-staging`. Internal testing; flagged rows are usually not paging-worthy unless they block an active staging change.

(The prod testnets `base-sepolia` and `ethereum-sepolia` were discontinued, so there's no prod-testnet tier.)

Within each tier, **flagged rows in bold** (CRIT and LOW). The Distributor's bridged-ZKC reserve table can stay as a single third table at the end (or split by tier too if it's noisy).

Follow with a short **"action list"** ordered by urgency (CRIT before LOW), with prod-mainnet items first since they map directly to operational impact. Skip the section if nothing is flagged.

## Common gotchas

- **Don't conflate `balanceOfCollateral` with `balanceOf` for distributors.** Distributors don't deposit stake — their ZKC sits in the bridged-ERC20 directly. The script renders both: market deposit (`needs_market_check: true`) and the per-distributor ZKC reserve in the second table.
- **Chain id reused across prod and staging**. Chain 167000 (Taiko) runs both a prod market (`taiko-mainnet`) and a staging market (`taiko-staging`); chain 84532 (Base Sepolia) now only carries the staging market (`base-sepolia-staging`) since the prod `base-sepolia` testnet was discontinued. The runbook disambiguates by chain label, and the skill's chain lookup table mirrors that.
- **Decommissioned hosts may still have CloudWatch log groups** (e.g. `/boundless/bento/prover-01`, `prover-02`, `bento-prover-{1,2}`). The audit reflects the runbook's `operator_addresses.json` (which mirrors `infra/cw-monitoring/Pulumi.production.yaml` for active provers), not historical log-group existence.
- **Public RPCs sometimes return 0** when rate-limited or transiently unhealthy. If a balance comes back as exactly 0, retry once with a private RPC (see Prerequisites: `network_secrets.toml`'s QuikNode keys) before declaring it actually empty.
- **`awk` truncates wei to scientific notation.** Always pipe through Python or `printf "%.6f"` for any wei→ETH math.
- **`cast call` quoting.** The function signature `'balanceOf(address)(uint256)'` must be a single-quoted string; the parens are part of the type signature.

## Adding a new operator address

Edit the runbook, not this skill:

1. Find the address in `infra/<service>/Pulumi.l-prod-*.yaml` (preferred) or `ansible/inventory.yml`.
2. Add a row to `runbooks/addresses/operator_addresses.json` (and the corresponding markdown table in `runbooks/addresses/README.md`) with the right role, chains, and `needs_gas` / `needs_market_check` flags.
3. The skill picks it up on the next run; no skill changes needed.

## Adding a new chain

1. Add the new chain to `contracts/deployment.toml` in the boundless repo (the canonical contract registry). The skill picks up market + collateral-token addresses automatically on the next run.
2. If the chain has a public RPC the skill needs to query, add a row to the `chain_rpc` function (the only inline thing left — RPC endpoints aren't in `deployment.toml`).
3. Reference the new chain label in `runbooks/addresses/operator_addresses.json` for any operator addresses operating on that chain.
