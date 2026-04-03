---
name: localnet
description: >-
  Start and interact with the Boundless localnet (docker compose-based local
  development network). Covers dev mode (fake proofs, broker included) and full
  proving mode, submitting requests, and debugging order flow. Use when the user
  wants to start localnet, run the local network, test proof requests locally,
  or debug broker/order-stream issues.
---

# Boundless Localnet

Guide the user through starting and using the docker compose-based localnet.

## Prerequisites

Before starting, ensure:

- Docker is running
- Contracts and guest binaries are built: `forge build && cargo check`
- Guest ELF binaries exist at `target/riscv-guest/` (built by `cargo check`)

## Choose Mode

Ask the user which mode they want (default to dev mode):

### Dev Mode (default, fast — fake proofs)

Uses `RISC0_DEV_MODE=1`. Proofs are faked so everything is instant. The broker runs as part of the docker compose cluster — no separate terminal needed. Best for testing request flow, contract interactions, and integration.

### Full Proving Mode (real proofs, requires GPU or Bento cluster)

Uses `RISC0_DEV_MODE=false`. The broker is NOT included in the localnet cluster. Start it separately with `just prover` (Bento + broker). Requires a proving backend (local GPU or Bento cluster).

## Starting Localnet

### Dev Mode

**Run `just localnet` in the foreground** (not as a background task). It handles deploying contracts and starting all services — wait for it to complete before submitting requests. This avoids needing to poll container status while the deployer runs.

**Explicitly set `RISC0_DEV_MODE` on the `just localnet` command** to avoid inheriting a stale value from the terminal environment. Other commands don't need it — `source .env.localnet` sets it.

```bash
# Start the full localnet (anvil + postgres + minio + order-stream + broker)
# Run in foreground — wait for it to finish before proceeding
RISC0_DEV_MODE=1 just localnet
```

#### Submitting a test request

Submit in a **background task**, then monitor status through order-stream, on-chain, and broker logs while it waits for fulfillment.

For **immediate broker acceptance**, the offer's `min-price` must exceed the broker's minimum profitable price (gas costs + min_mcycle_price). On anvil, gas costs are ~0.00003 ETH, so `0.0001 ETH` is safely above the threshold. The price starts at `min-price` before `rampUpStart` (defaults to `now() + 30s`), so setting `min-price` high enough means the broker accepts before the ramp even begins.

```bash
source .env.localnet && cargo run --example submit_echo -- \
  --bidding-start "$(date +%s)" \
  --min-price "0.0001 ETH" \
  --max-price "0.0004 ETH"
```

**Why these values:**

- `--bidding-start "$(date +%s)"`: Sets `rampUpStart` to now. Without this, the default is `now() + 30s` (`DEFAULT_BASE_RAMP_UP_DELAY`), which delays when the auction begins.
- `--min-price "0.0001 ETH"`: Above broker's minimum profitable price (~0.00003 ETH gas on anvil). If min-price is too low, the broker schedules a delayed lock attempt and waits for the auction price to ramp up past its threshold.
- `--max-price "0.0004 ETH"`: ~4x min-price, reasonable ceiling

### Full Proving Mode

**Explicitly set `RISC0_DEV_MODE=false` on the `just localnet` command** to avoid inheriting a stale value from the terminal environment. Other commands don't need it — `source .env.localnet` sets it.

```bash
# Terminal 1: Start the localnet (anvil + postgres + minio + order-stream, NO broker)
RISC0_DEV_MODE=false just localnet

# Terminal 2: Start the prover cluster (Bento + broker)
source .env.localnet && just prover

# Terminal 3: Submit a test request
source .env.localnet && cargo run --example submit_echo -- \
  --bidding-start "$(date +%s)" \
  --min-price "0.0001 ETH" \
  --max-price "0.0004 ETH"
```

## Checking Order Status

After submitting a request, you'll see a request ID like:

```
Submitted request: 90f79bf6eb2c4f870365e785982e1f101e93b9061d289271
```

### Query the order-stream

Note: the `limit` query parameter is required.

```bash
# List recent orders
curl -s "http://localhost:8585/api/v1/orders?limit=10" | jq .

# Query a specific order by request ID
curl -s "http://localhost:8585/api/v1/orders?request_id=<REQUEST_ID>&limit=10" | jq .
```

### Check request status from broker logs

Use broker logs to extract the requestor-relevant milestones. The user cares about **when** the request was submitted, locked, and fulfilled, plus the **fulfilled price** — not the internal proving pipeline steps.

```bash
# Dev mode: broker runs in compose
docker compose -f dockerfiles/compose.localnet.yml --profile dev-broker logs broker 2>&1 | grep <REQUEST_ID>
```

Key log lines to look for:

- `Locked request <ID>` — when the broker locked it
- `Completed order: <ID> eth_reward: <PRICE>` — when it was fulfilled and at what price
- The order's `created_at` field from the order-stream query shows submission time

Present status to the user as:

| Event     | Time                                |
| --------- | ----------------------------------- |
| Submitted | (from order-stream `created_at`)    |
| Locked    | (from broker log "Locked request")  |
| Fulfilled | (from broker log "Completed order") |

**Fulfilled price:** (from `eth_reward` in the "Completed order" log line)

### Check order-stream logs

```bash
docker compose -f dockerfiles/compose.localnet.yml logs order-stream

# Watch live
docker compose -f dockerfiles/compose.localnet.yml logs -f order-stream
```

## Stopping

```bash
# Stop localnet (preserves anvil state for restart)
just localnet down

# Stop and wipe all state (fresh start)
just localnet clean
```

## Troubleshooting

### Request stuck at "Unknown"

- Check order-stream logs — did it receive the order?
- If `Broadcasted order ... to 0 clients`, the broker isn't connected to the WebSocket
- In dev mode, check broker container is running: `docker compose -f dockerfiles/compose.localnet.yml --profile dev-broker ps`

### Broker delays lock ("scheduled for lock attempt in Ns")

The broker waits until the auction price exceeds its minimum profitable price (gas costs + min_mcycle_price). To avoid this delay, set `--min-price` above the broker's gas cost threshold (~0.0001 ETH on anvil). The price sits at `min-price` before `rampUpStart` (default `now() + 30s`), so a sufficiently high `min-price` means instant acceptance.

### Broker not picking up orders

- Check that `--min-price` is high enough to cover gas + min_mcycle_price
- Check broker's `min_mcycle_price` in `broker.toml` vs the offer price

### Price oracle errors on anvil

- Ensure `broker.toml` has the `[price_oracle]` section with static prices and chainlink disabled. `just localnet` adds this automatically.

### Port conflicts

- Localnet uses ports: 8545 (anvil), 8585 (order-stream), 5435 (postgres), 9100/9101 (minio)
- Check for conflicts: `ss -tlnp | grep -E '8545|8585|5435|9100'`
