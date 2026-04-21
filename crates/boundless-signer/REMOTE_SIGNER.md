# Remote Signer Setup

By default the Boundless broker and CLI read a private key from an environment
variable (`PROVER_PRIVATE_KEY`, `REWARD_PRIVATE_KEY`).  On cloud GPU marketplace
nodes this exposes the key to an untrusted environment.

The **remote signer** option lets you delegate all signing to a separate
*fleet-node* process that owns the key.  The broker and CLI become HTTP
consumers: they POST a signing request and receive a signature or transaction
hash in return.  The private key never leaves the fleet-node.

---

## Quick start

### Broker

Set two environment variables before starting the broker — no config file
changes needed:

```bash
export SIGNER_BACKEND=http-remote
export SIGNER_HTTP_URL=http://fleet-node:3100   # your fleet-node address

# The broker still needs the RPC URL; the private key is NOT needed
export PROVER_RPC_URL=https://mainnet.base.org
```

Alternatively, add a `[signer]` section to your broker TOML config file:

```toml
[signer]
backend = "http-remote"

[signer.http]
url = "http://fleet-node:3100"
```

### CLI (prover commands)

```bash
export SIGNER_PROVER_BACKEND=http-remote
export SIGNER_PROVER_HTTP_URL=http://fleet-node:3100
export PROVER_RPC_URL=https://mainnet.base.org
```

### CLI (rewards commands)

```bash
export SIGNER_REWARDS_BACKEND=http-remote
export SIGNER_REWARDS_HTTP_URL=http://fleet-node:3100
export REWARD_RPC_URL=https://mainnet.base.org
```

---

## Configuration reference

### Environment variables

| Variable | Scope | Description |
|---|---|---|
| `SIGNER_BACKEND` | All roles | Backend for all roles: `local` or `http-remote` |
| `SIGNER_PROVER_BACKEND` | Prover role | Overrides `SIGNER_BACKEND` for the prover role |
| `SIGNER_REWARDS_BACKEND` | Rewards role | Overrides `SIGNER_BACKEND` for the rewards role |
| `SIGNER_HTTP_URL` | All roles | Fleet-node base URL when `SIGNER_BACKEND=http-remote` |
| `SIGNER_PROVER_HTTP_URL` | Prover role | Overrides `SIGNER_HTTP_URL` for the prover role |
| `SIGNER_REWARDS_HTTP_URL` | Rewards role | Overrides `SIGNER_HTTP_URL` for the rewards role |
| `PROVER_PRIVATE_KEY` | Prover role | Private key (hex). Used when backend is `local` or unset |
| `REWARD_PRIVATE_KEY` | Rewards role | Private key (hex). Used when backend is `local` or unset |
| `PRIVATE_KEY` | Prover role | Legacy alias for `PROVER_PRIVATE_KEY` |

> **Note:** `SIGNER_HTTP_URL` / `SIGNER_PROVER_HTTP_URL` / `SIGNER_REWARDS_HTTP_URL`
> are convenience shorthands.  The canonical way to set the URL is via the TOML
> `[signer.*.http] url` field or the env var approach shown in the TOML section below.

### TOML config (broker / CLI config files)

Role-specific settings override shared settings.

```toml
# ── Shared settings (apply to all roles that have no specific override) ──────
[signer]
backend = "http-remote"           # "local" | "http-remote"
private_key_env = "MY_KEY_VAR"    # env var name that holds the hex private key
                                  # (only used when backend = "local")

[signer.http]
url = "http://fleet-node:3100"    # fleet-node base URL

# ── Prover role (Base L2 — chain 8453) ───────────────────────────────────────
[signer.prover]
backend = "http-remote"

[signer.prover.http]
url = "http://fleet-node-prover:3100"

# ── Rewards role (Ethereum L1 — chain 1) ─────────────────────────────────────
[signer.rewards]
backend = "local"
private_key_env = "REWARD_PRIVATE_KEY"
```

Fields not present in a role section fall through to the shared `[signer]`
block, then to environment variables, then to the built-in key env var
defaults (`PROVER_PRIVATE_KEY`, `REWARD_PRIVATE_KEY`).

---

## Priority chain

`from_config()` resolves the backend for each role in this order:

1. Role-specific TOML block (`[signer.prover]` or `[signer.rewards]`)
2. Shared TOML block (`[signer]`)
3. Role-specific env var (`SIGNER_PROVER_BACKEND` / `SIGNER_REWARDS_BACKEND`)
4. Shared env var (`SIGNER_BACKEND`)
5. Key env var fallback — if `PROVER_PRIVATE_KEY` / `REWARD_PRIVATE_KEY` is
   set, a `local` backend is built automatically (zero-regression default)

If nothing is configured, startup fails with a descriptive error.

---

## Fleet-node HTTP API contract

The remote signer makes two calls to the fleet-node:

### `GET /sign/capabilities` (called at startup)

Discovers the wallet address for each role.  The broker fails fast at startup
if this call fails, so misconfigured fleet-node URLs surface immediately.

**Response**:

```json
{
  "roles": {
    "prover":  { "address": "0xabc..." },
    "rewards": { "address": "0xdef..." }
  }
}
```

### `POST /sign` (called per signing request)

**Request — hash signing** (used for EIP-191 / EIP-712 / permit / lock / fulfill):

```json
{
  "type": "hash",
  "role": "prover",
  "hash": "0x<32 bytes hex>"
}
```

**Request — message signing** (used for WebSocket auth):

```json
{
  "type": "message",
  "role": "prover",
  "message": "0x<bytes hex>"
}
```

**Request — transaction submission** (broker send path):

```json
{
  "type": "transaction",
  "role": "prover",
  "intent": {
    "to": "0x<address>",
    "data": "0x<calldata hex>",
    "value": "0x0",
    "gas_limit": 300000,
    "chain_id": 8453
  }
}
```

**Success response — hash / message signing**:

```json
{ "signature": "0x<65 bytes hex, r|s|v>" }
```

**Success response — transaction**:

```json
{ "tx_hash": "0x<32 bytes hex>" }
```

**Error response** (any non-200 status):

```json
{ "error": "short_code", "message": "human-readable detail" }
```

### HTTP status codes

| Code | Maps to | Broker behavior |
|---|---|---|
| `200` | Success | Normal |
| `403` | `SignerError::Rejected` | Log and skip the order |
| `408` | `SignerError::Timeout` | Log and skip the order |
| `502` | `SignerError::BroadcastFailed` | Single retry, then skip |
| `503` | `SignerError::Unavailable` | Exponential back-off, pause matching loop |
| Other | `SignerError::Transport` | Treat as transient; retry with back-off |

---

## Signing surfaces

The following operations trigger signing calls:

| Operation | Type | Role |
|---|---|---|
| Lock request | `hash` (EIP-712) | Prover |
| Fulfill request | `hash` (EIP-712) | Prover |
| Slash | `transaction` | Prover |
| Deposit collateral (permit) | `hash` (EIP-2612) | Prover |
| Withdraw collateral | `transaction` | Prover |
| Off-chain WebSocket auth | `message` (EIP-191) | Prover |
| Stake ZKC (permit) | `hash` (EIP-2612) | Rewards |
| Claim staking rewards | `transaction` | Rewards |
| Claim mining rewards | `transaction` | Rewards |
| Delegate reward power | `transaction` | Rewards |
| Submit mining work | `transaction` + proof sign | Rewards |

> **Note on `submit-mining`:** The internal RISC Zero proof (`prove_update`)
> also requires signing a work-log update.  This uses the same `Rewards` signing
> key.  When using `http-remote`, ensure the fleet-node's `rewards` role key
> matches the address stored in the mining state file (`log_id`).

---

## Troubleshooting

**`signer unavailable: role 'prover' not in /sign/capabilities response`**  
The fleet-node is running but the `prover` role is not configured.  Check the
fleet-node's role configuration.

**`transport error: error sending request for url`**  
The fleet-node URL is unreachable.  Verify the URL and network connectivity
between the broker host and the fleet-node.

**`http-remote backend for role 'prover' requires [signer.prover.http] url`**  
`backend = "http-remote"` is set but no URL was provided.  Add the `[signer.http]
url` field or set `SIGNER_HTTP_URL`.

**`unknown signer backend 'foo'`**  
`SIGNER_BACKEND` or `backend` in TOML is set to an unrecognised value.  Valid
values are `local` and `http-remote`.

**Broker starts but signs with local key instead of fleet-node**  
The priority chain fell through to the `PROVER_PRIVATE_KEY` env var fallback.
Check that `SIGNER_BACKEND=http-remote` (or the TOML equivalent) is set and
that `SIGNER_HTTP_URL` points to the fleet-node.  If both the env var and a
private key are present, the explicit backend setting takes precedence.
