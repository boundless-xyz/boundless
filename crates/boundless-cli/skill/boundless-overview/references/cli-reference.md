# Boundless CLI Reference

## Global Options

All commands accept these options:

| Option                | Env Var      | Description                                    |
| --------------------- | ------------ | ---------------------------------------------- |
| `--tx-timeout <SECS>` | `TX_TIMEOUT` | Ethereum transaction timeout (default: 300s)   |
| `--log-level <LEVEL>` | `LOG_LEVEL`  | Log verbosity: error, warn, info, debug, trace |

## Requestor Commands

### `boundless requestor submit`

Submit a proof request with an offer, input, and guest image.

| Option                       | Env Var                    | Description                                 |
| ---------------------------- | -------------------------- | ------------------------------------------- |
| `--requestor-rpc-url`        | `REQUESTOR_RPC_URL`        | RPC endpoint                                |
| `--requestor-private-key`    | `REQUESTOR_PRIVATE_KEY`    | Private key for transactions                |
| `--boundless-market-address` | `BOUNDLESS_MARKET_ADDRESS` | Market contract (has default per network)   |
| `--set-verifier-address`     | `SET_VERIFIER_ADDRESS`     | Verifier contract (has default per network) |

### `boundless requestor status`

Check the status of a proof request by ID.

### `boundless requestor deposit` / `withdraw` / `balance`

Manage funds deposited in the market escrow.

## Prover Commands

### `boundless prover fulfill`

Fulfill one or more proof requests.

| Option                 | Env Var              | Description                  |
| ---------------------- | -------------------- | ---------------------------- |
| `--prover-rpc-url`     | `PROVER_RPC_URL`     | RPC endpoint                 |
| `--prover-private-key` | `PROVER_PRIVATE_KEY` | Private key for transactions |
| `--bento-api-url`      | `BENTO_API_URL`      | Bento/Bonsai API URL         |
| `--bento-api-key`      | `BENTO_API_KEY`      | Bento/Bonsai API key         |

### `boundless prover generate-config`

Interactive wizard to generate optimized `broker.toml` and `compose.yml` for running a prover node.

### `boundless prover benchmark`

Run benchmark suite to measure proving performance.

## Rewards Commands

### `boundless rewards stake-zkc`

Stake ZKC tokens for staking rewards.

| Option                  | Env Var               | Description                      |
| ----------------------- | --------------------- | -------------------------------- |
| `--reward-rpc-url`      | `REWARD_RPC_URL`      | RPC endpoint for rewards network |
| `--staking-private-key` | `STAKING_PRIVATE_KEY` | Private key for staking          |
| `--staking-address`     | `STAKING_ADDRESS`     | Staking address (read-only)      |

### `boundless rewards prepare-mining`

Prepare a mining work log update from Bento proving receipts.

| Option                | Env Var             | Description               |
| --------------------- | ------------------- | ------------------------- |
| `--mining-state-file` | `MINING_STATE_FILE` | Path to mining state file |

### `boundless rewards submit-mining`

Submit mining work updates on-chain.

### `boundless rewards claim-mining-rewards` / `claim-staking-rewards`

Claim accumulated rewards.

## Configuration Precedence

Configuration values are resolved in this order (highest priority first):

1. **CLI arguments** - Flags passed directly to the command
2. **Environment variables** - e.g., `REQUESTOR_RPC_URL`
3. **Config file** - `~/.boundless/secrets.toml` and `~/.boundless/config.toml`
4. **Network defaults** - Built-in contract addresses for known networks

## Setup Wizards

Each module has an interactive setup wizard:

```
boundless requestor setup
boundless prover setup
boundless rewards setup
```

These guide you through network selection, credential configuration, and write the appropriate config/secrets files.
