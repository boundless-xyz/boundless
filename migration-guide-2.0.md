# Boundless Prover: Migration Guide v1.x to v2.0

This guide covers upgrading a Boundless prover from v1.x to v2.0. Allow 30-60 minutes of downtime for the upgrade. If anything goes wrong, the [rollback procedure](#rollback) restores your v1.x stack in under a minute.

## What's Changing

**Multi-chain support** -- a single broker instance now serves multiple chains simultaneously. v2.0 ships with support for Base Mainnet and Taiko Mainnet. Your proving cluster is shared across chains; the broker routes work automatically.

**New default proving backend** -- v2.0 introduces a leaner Redis-only **prover stack** as the default. It does three things for provers:

- **Higher profit per GPU** — orders complete 1.5–2x faster on small and mid-size cycle workloads (more revenue), with fewer proving failures and slashings (lower operational cost).
- **Removes the cluster mining penalty** — the Redis-backed TaskDB rewrite eliminates the overheads that made running large clusters inefficient.
- **Multi-chain from one node** — one prover node can now monitor and fulfill Proof Requests across multiple chains, allowing provers to fulfill more orders (more revenue). Taiko alongside Base ships in this release.

|                      | Prover stack (default)                                                           | Legacy bento stack (opt-in)                              |
| -------------------- | -------------------------------------------------------------------------------- | -------------------------------------------------------- |
| Compose file         | `prover-compose.yml`                                                             | `compose.yml`                                            |
| Compose project name | `prover`                                                                         | `bento`                                                  |
| Start with           | `just prover up`                                                                 | `PROVER_STACK=legacy just prover up`                     |
| Infrastructure       | Redis only                                                                       | Redis + PostgreSQL + MinIO                               |
| Docker images        | `prover-*` (`prover-agent`, `prover-agent-cpu`, `prover-rest-api`, `prover-cli`) | `bento-*`                                                |
| Status               | **Recommended**                                                                  | Supported for backward compatibility, will be deprecated |

The legacy bento stack is supported via `PROVER_STACK=legacy` for hosts that aren't ready to cut over (e.g. anything with persistent PostgreSQL/MinIO state), but will be removed in a future release.

> **The two stacks have separate Compose project namespaces.** v1.x ran under project name `bento`; v2.0's prover stack runs under project name `prover`. That means **Redis volumes are not shared between the two stacks** — migrating from bento to the prover stack creates a fresh Redis volume rather than re-using `bento_redis-data`. Same in reverse on rollback. See [Step 1: Back Up](#step-1-back-up) and [Rollback](#rollback) for the implications.

**New default chain-monitor implementation** -- v2.0 ships a rewritten chain-watching layer (`eth_getBlockReceipts`-based) that reduces **RPC call volume by up to ~5×** in head-to-head testing against v1.x, lowering paid RPC plan costs. Selection is now per-chain via the `[market] rpc_mode` field in `broker.toml`:

- `"auto"` (default) -- uses the chain-specific default: `v2` for Base and most chains, `legacy` for Taiko (167000).
- `"v2"` -- force the new `ChainMonitorV2` polling loop.
- `"legacy"` -- force the old `ChainMonitorService` + `MarketMonitor` pair (`eth_getLogs`-based).

Override per-chain in `chain-overrides/broker.{chain_id}.toml` if you need to pin a mode.

## Prerequisites

- Running Boundless prover v1.x
- Docker Compose v2+
- NVIDIA GPU with drivers installed (unchanged from v1.x)

> ## ⚠️ Submit outstanding PoVW work receipts before upgrading
>
> **The two stacks store PoVW work receipts in different backends, and there is no migration path between them.** Any unsubmitted work receipts in your current MinIO will be left behind when you switch to the prover stack. Submit them on-chain using the steps below before you continue past [Step 1](#step-1-back-up).
>
> **Background.** Each completed proving job writes a PoVW work receipt to `work_receipts/{job_id}.bincode` in the prover's object store (the constant `WORK_RECEIPTS_BUCKET_DIR` in `bento/crates/workflow-common/src/s3.rs` and `prover/crates/workflow-common/src/storage.rs`). The legacy bento stack stores these in the MinIO bucket; the new prover stack stores them on the `rest_api` service's filesystem under `./data/object_store/work_receipts/`. After upgrading, the bento MinIO volume is no longer mounted by any service in the prover stack, and the prover stack's `rest_api` starts with an empty `work_receipts/` directory. Receipts not consumed by `prepare-mining` before the switch never make it into a log update and can no longer be claimed.
>
> **What to do.** We will stop the full stack (`just prover down`), bring up only the storage/backend services (`just bento up`), submit outstanding PoVW receipts, then tear that down before proceeding with the rest of the migration.
>
> ```bash
> # 1. Gracefully stop the v1.x stack (broker + agents).
> just prover down
>
> # 2. Bring up bento backend only — REST API + MinIO + supporting services.
> #    Use the same env / PROVER_STACK (if any) as for `just prover` so Compose picks the right file.
> just bento up
>
> # 3. Build a log-update proof from every unsubmitted work receipt
> #    currently in MinIO. Reads via the Bento REST API at /work-receipts.
> boundless rewards prepare-mining
>
> # 4. Submit the prepared log update on-chain.
> boundless rewards submit-mining
>
> # 5. (Optional) Confirm there's nothing left to submit.
> boundless rewards inspect-mining-state
>
> # 6. Take bento back down before continuing with the migration.
> just bento down
> ```
>
> If `prepare-mining` reports `0 new mining work receipts`, you're clear to proceed. If it produces an update, submit it before continuing — receipts left behind in the bento MinIO volume can't be replayed against the new stack's REST API.

## Step 1: Back Up

The v1.x stack should already be stopped from the PoVW step above. If not, bring it down now:

```bash
just prover down
```

Back up configuration:

```bash
cp broker.toml broker.toml.v1.bak
cp compose.yml compose.yml.v1.bak
[ -f .env ] && cp .env .env.v1.bak || true    # only if you use a .env file
```

v2.0 doesn't touch your existing `broker.db` — it runs under a separate Compose project (`prover`) and writes new per-chain DBs (`broker.8453.db`, `broker.167000.db`) alongside it, so rollback is a clean swap with no overlapping state.

PostgreSQL and MinIO data don't need to be backed up: the only non-recoverable contents were PoVW work receipts, which were submitted on-chain in the [prerequisite step above](#submit-outstanding-povw-work-receipts-before-upgrading); everything else (proving artifacts, request inputs) is recreated on demand. If you don't plan to roll back to the bento stack, you can reclaim the disk later with `docker volume rm bento_postgres-data bento_minio-data`.

> **Redis state does not carry across the bento → prover-stack switch.** v1.x's bento stack stored Redis under the Compose project `bento` (`bento_redis-data` volume); v2.0's prover stack uses project `prover` (`prover_redis-data`). After you bring the prover stack up, Redis starts fresh — any in-flight worker tasks tracked in the bento Redis volume are left behind. The broker's SQLite DB holds the canonical order state, so order tracking is unaffected; what's lost is in-flight proving work in the worker pool, which the broker will retry or time out as appropriate. The bento Redis volume is _kept_ by Docker after `just prover down` — volumes aren't auto-removed — so a rollback can pick it up again.

## Step 2: Update the Repository

<!-- TODO: replace v2.0.0 with the actual release tag before publishing -->

Check out the v2.0 release tag to get the updated compose file, justfile, and dockerfiles:

```bash
git fetch origin
git checkout v2.0.0
```

## Step 3: Update broker.toml

The recommended approach is to install the latest CLI and re-run the configuration wizard, which has been updated for v2.0 with multi-chain support:

<!-- TODO: replace v2.0.0 with the actual release tag before publishing -->

```bash
# Install the v2.0 Boundless CLI
cargo install --locked --git https://github.com/boundless-xyz/boundless boundless-cli --tag boundless-v2.0.0 --bin boundless

# Re-run the config wizard
boundless prover generate-config
```

The wizard will prompt for chain selection (Base, Taiko, or both), collect per-chain RPC URLs, keys, and pricing, and generate a base `broker.toml`, per-chain overrides in `chain-overrides/`, and v2.0 `prover-compose.yml` / `compose.yml` (the same derived settings are written to both, so you only need to edit the file matching the stack you actually run).

> **Taiko RPC option:** Taiko offer a free RPC endpoint at: `https://rpc.mainnet.taiko.xyz` (for a full list: `https://chainlist.org/?search=taiko`). Use at your own risk. See [Step 4](#step-4-configure-multi-chain) for details.

> **Taiko ZKC:** If you enable Taiko, bridge ZKC to your prover wallet on Taiko and deposit market collateral before going live. See [Taiko: Bridge and deposit ZKC](#taiko-bridge-and-deposit-zkc-when-enabling-taiko).

If you used the wizard, skip to [Step 5](#step-5-start-the-stack) after completing any Taiko bridging and collateral deposit you need.

### Manual config migration

If you prefer to migrate your existing config manually instead of using the wizard:

#### Renamed fields

Several config field names have been renamed. The old names are **no longer accepted** in v2.0 — the backward-compatibility aliases have been removed.

**Before (v1.x)**:

```toml
[market]
mcycle_price = "0.00000001"
max_stake = "200"
max_concurrent_locks = 1
stake_balance_warn_threshold = "100"

[batcher]
batch_size = 2
```

**After (v2.0)**:

```toml
[market]
min_mcycle_price = "0.00000001"
max_collateral = "200"
max_concurrent_proofs = 1
collateral_balance_warn_threshold = "100"

[batcher]
min_batch_size = 2
```

Full rename table:

| Old name (v1.x)                      | New name (v2.0)                      |
| ------------------------------------ | ------------------------------------ |
| `mcycle_price`                       | `min_mcycle_price`                   |
| `max_stake`                          | `max_collateral`                     |
| `stake_balance_warn_threshold`       | `collateral_balance_warn_threshold`  |
| `stake_balance_error_threshold`      | `collateral_balance_error_threshold` |
| `max_concurrent_locks`               | `max_concurrent_proofs`              |
| `batch_size`                         | `min_batch_size`                     |
| `expired_order_fulfillment_priority` | `order_commitment_priority`          |

> If your v1.x config already uses the new names (most v1.3+ configs do), no changes are needed. You can verify with:
>
> ```bash
> grep -E 'mcycle_price\b[^_]|max_stake|stake_balance|max_concurrent_locks|^batch_size|expired_order_fulfillment' broker.toml
> ```

#### Per-chain config overrides

For most setups, the base `broker.toml` is sufficient for all chains — gas estimation is auto-tuned per chain, and pricing is typically the same. If you do need chain-specific settings, you can place override files in `chain-overrides/`:

```
chain-overrides/broker.167000.toml   # overrides for Taiko
chain-overrides/broker.8453.toml     # overrides for Base
```

Any field set in an override takes precedence over the base `broker.toml` for that chain. Fields not specified inherit from the base. Some fields are **global-only** and cannot be overridden per chain: `peak_prove_khz`, `max_concurrent_proofs`, `order_pricing_priority` — these apply to the entire proving cluster.

## Step 4: Configure Multi-Chain

To enable multi-chain, set per-chain RPC endpoints. These are configured via environment variables — either shell exports, a `.env` file, or however you currently manage your environment.

**What changes from v1.x:**

- `PROVER_RPC_URL` becomes `PROVER_RPC_URL_8453` and `PROVER_RPC_URL_167000`
- `BOUNDLESS_MARKET_ADDRESS` and `SET_VERIFIER_ADDRESS` can be removed — the broker has built-in deployment addresses for known chains that are resolved automatically from the chain ID
- Optionally, `PROVER_PRIVATE_KEY_8453` / `PROVER_PRIVATE_KEY_167000` for per-chain wallets (or keep a single global `PROVER_PRIVATE_KEY`)

The new variables:

```bash
# Per-chain RPC endpoints (replaces PROVER_RPC_URL)
export PROVER_RPC_URL_8453=https://your-base-rpc.example.com
export PROVER_RPC_URL_167000=https://rpc.mainnet.taiko.xyz
```

> **Taiko RPC option:** Taiko offer a free RPC endpoint at: `https://rpc.mainnet.taiko.xyz` (for a full list: `https://chainlist.org/?search=taiko`). Use at your own risk. See [Step 4](#step-4-configure-multi-chain) for details.

That's it. The broker auto-discovers chains from `PROVER_RPC_URL_{chain_id}` variables, resolves the contract addresses for known chains, and creates separate SQLite databases per chain automatically (`broker.8453.db`, `broker.167000.db`).

Old v1.x variables like `BOUNDLESS_MARKET_ADDRESS`, `SET_VERIFIER_ADDRESS`, `POSTGRES_HOST`, `MINIO_HOST`, etc. are harmless if still present. When per-chain `PROVER_RPC_URL_{chain_id}` variables are set, the global `PROVER_RPC_URL` is ignored.

## Taiko: Bridge and deposit ZKC (when enabling Taiko)

If your v2.0 setup includes Taiko (`PROVER_RPC_URL_167000`), the Boundless Market on Taiko settles collateral in **ZKC**. That balance must live on Taiko—the same Ethereum mainnet ZKC you may already hold for Base does not automatically appear on Taiko.

1. **Bridge ZKC to Taiko** — Use the Taiko native bridge to move ZKC from **Ethereum Mainnet** to **Taiko Mainnet**: [https://bridge.taiko.xyz/](https://bridge.taiko.xyz/). Bridge to the **same wallet address** you configure for the prover on Taiko (`PROVER_PRIVATE_KEY_167000` or your shared `PROVER_PRIVATE_KEY`). Complete any claim or finalization steps the UI requires so the tokens are visible on Taiko.

2. **Keep gas on Taiko** — You need Taiko **ETH** (or the native gas token Taiko uses for transactions) to approve and deposit collateral. Bridge or fund that separately if needed.

3. **Deposit collateral into the market** — Enable Taiko in the Boundless CLI via the prover setup wizard so `deposit-collateral` targets the Taiko market (this configures the CLI module separately from `generate-config`, which produces `broker.toml` and compose files):

   ```bash
   boundless prover setup
   ```

   Follow the prompts to enable **Taiko Mainnet** (Taiko RPC URL and prover private key as needed). Then deposit:

   ```bash
   boundless prover deposit-collateral "<amount>"
   ```

   Confirm the on-chain market balance with `boundless prover balance-collateral`.

## Step 5: Start the Stack

```bash
just prover up
```

> v2.0 starts the prover stack by default. To opt back into the legacy bento stack, prefix `PROVER_STACK=legacy` on every `just prover` invocation. The `just prover` command creates `chain-overrides/` and `broker.toml` from the template if missing. To skip the mining container, add `BOUNDLESS_MINING=false`.

You should see startup messages like:

```
INFO broker: Starting pipeline for chain chain_id=8453 ...
INFO broker: Starting pipeline for chain chain_id=167000 ...
INFO broker: Configured to run with Bento backend
INFO chain{chain_id=8453}: broker::market_monitor: Starting up market monitor
INFO chain{chain_id=8453}: broker::market_monitor: Detected new on-chain request 0x...
INFO chain{chain_id=167000}: broker::chain_monitor: Starting ChainMonitor service
```

> You may see AWS IMDS timeout warnings if running on non-AWS hardware. These are harmless:
>
> ```
> WARN aws_config::imds::region: failed to load region from IMDS ...
> ```

## Step 6: Verify

**Services are running:**

```bash
just prover logs
```

Look for `Starting pipeline for chain` lines — one per chain — followed by `Detected new on-chain request` messages.

With the prover stack (default) you should see `redis`, `rest_api`, `exec_agent`, `aux_agent`, `gpu_prove_agent`, `broker`, `prometheus`, `grafana` — no `postgres` or `minio`. The legacy bento stack will additionally include `postgres`, `minio`, and `minio-init`.

**REST API is healthy** — the broker logs will show `Configured to run with Bento backend` once it successfully connects to the REST API.

## Rollback

<!-- TODO: replace v1.4.0 with the actual previous release tag before publishing -->

```bash
# Stop v2.0 stack (prover-compose, project name "prover")
just prover down

# Restore v1.x repo and config
git checkout v1.4.0
cp broker.toml.v1.bak broker.toml
cp compose.yml.v1.bak compose.yml
[ -f .env.v1.bak ] && cp .env.v1.bak .env || true    # only if you backed up .env

# Start v1.x stack (back to project name "bento")
just prover up
```

Your v1.x broker SQLite database is untouched — v2.0 uses a separate sqlite DB. The per-chain environment variables (`PROVER_RPC_URL_8453`, etc.) are ignored by the v1.x broker.

> **Redis state since the upgrade is left behind on rollback** — the v2.0 prover stack ran under Compose project `prover` with its own `prover_redis-data` volume; v1.x bento uses `bento_redis-data`. When you start the v1.x stack again it picks up the bento volume right where you left it before Step 1, but anything that proved during your v2.0 window is in the prover volume and won't be visible to v1.x. Same caveat as in [Step 1](#step-1-back-up): the broker's canonical order state is in SQLite, so order tracking carries through; only in-flight worker-pool tasks are lost. If you also moved partway to the legacy bento stack on v2.0 (via `PROVER_STACK=legacy`), prefix `PROVER_STACK=legacy` on the stop command above.

## FAQ

### Do I need to export data from PostgreSQL or MinIO?

No. PostgreSQL held proving state that v2.0's Redis-backed TaskDB recreates on demand. MinIO held PoVW work receipts and request inputs — receipts are submitted on-chain in the [prerequisite step](#submit-outstanding-povw-work-receipts-before-upgrading) before the upgrade, and inputs are re-fetched as needed. The default v2.0 prover stack drops PostgreSQL and MinIO entirely; if you opt back into the legacy bento stack with `PROVER_STACK=legacy`, both continue working as before.

### Do I need to migrate my broker database for 2.0?

No. The broker creates new per-chain databases (`broker.8453.db`, `broker.167000.db`) under a separate docker compose namespace and does not touch the existing `broker.db`.

### How do I add a new chain later?

Add the chain's RPC URL (`PROVER_RPC_URL_{chain_id}`) and restart the broker. Contract addresses are resolved automatically for supported chains. The proving agents don't need to restart.

### What are the supported chains?

Base Mainnet (8453) and Taiko Mainnet (167000). Sepolia (11155111) and Base Sepolia (84532) are available for testing.

### I'm not ready to leave the bento stack — can I stay on it?

Yes. Set `PROVER_STACK=legacy` and every `just prover` command keeps using `compose.yml`. The bento path still gets v2.0's multi-chain broker and the new default chain-watching layer — only the storage stack stays the same. Plan to migrate to the prover stack before the next release; bento will be removed.

### How do I select the chain-monitor implementation?

In v2.0 it's a per-chain TOML field, not a CLI flag. Set `rpc_mode` under `[market]` in your base `broker.toml` or in a `chain-overrides/broker.{chain_id}.toml` override:

- `"auto"` (default) -- chain-specific default: `v2` for most chains, `legacy` for Taiko (167000) where `eth_getBlockReceipts` might get rate limited on the public RPC (`https://rpc.mainnet.taiko.xyz`, see [Step 4](#step-4-configure-multi-chain)).
- `"v2"` -- pin to `ChainMonitorV2` (`eth_getBlockReceipts`).
- `"legacy"` -- pin to the old `ChainMonitorService` + `MarketMonitor` pair (`eth_getLogs`).
