# Order Generator

The order generator submits compute requests to the Boundless Market. It runs in a loop, creating and submitting requests at a configurable interval. It supports single-key operation or address rotation for higher throughput and privacy.

## Overview

The order generator:

- **Builds requests** with a guest program (loop or Zeth) and input, uploads them to storage, and submits to the Boundless Market
- **Submits onchain or offchain** — onchain writes directly to the market contract; offchain sends to an order stream (requires `order_stream_url` in deployment)
- **Runs in two modes**: single key (one address for all requests) or rotation (multiple addresses derived from a mnemonic)

## Modes

### Single-Key Mode

Use `--private-key` (or `PRIVATE_KEY` env) with a funded key. The generator submits all requests from that address.

```bash
order-generator \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --deployment-chain-id 84532
```

### Address Rotation

Use `--mnemonic` with `--derive-rotation-keys N` to derive multiple addresses. The generator rotates through them on a schedule (e.g., daily), with one key as the funding source that tops up the current address.

```bash
order-generator \
  --rpc-url $RPC_URL \
  --mnemonic "your twelve word phrase here" \
  --derive-rotation-keys 3 \
  --deployment-chain-id 84532
```

See [Address Rotation](#address-rotation) for details.

## Request Types

### Loop Guest (default)

Uses the built-in loop guest. You can specify:

- `--input` — fixed cycle count
- `--input-max-mcycles` — max Mcycles for random cycle count (default 1000)
- `--program` — path to custom guest binary (otherwise uses default from IPFS)

### Zeth Guest

Use `--use-zeth` to submit Zeth-style requests. Requires a pre-built input from IPFS. Cycle count is randomized based on `--input-max-mcycles` (default 3000 Mcycles).

## Key Options

| Option                      | Description                                                          |
| --------------------------- | -------------------------------------------------------------------- |
| `--rpc-url`                 | Ethereum RPC endpoint                                                |
| `--rpc-urls`                | Comma-separated RPC URLs for failover                                |
| `--private-key`             | Signing key (or funding source when rotation enabled)                |
| `--deployment-chain-id`     | Chain ID (auto-selects deployment)                                   |
| `--interval`                | Seconds between requests (default: 60)                               |
| `--count`                   | Number of requests, then exit (default: run indefinitely)            |
| `--submit-offchain`         | Submit to order stream instead of onchain                            |
| `--auto-deposit`            | Auto-deposit ETH when market balance below threshold (offchain only) |
| `--tx-timeout`              | Transaction confirmation timeout in seconds (default: 45)            |
| `--submit-retry-attempts`   | Retries for failed submissions (default: 3)                          |
| `--submit-retry-delay-secs` | Delay between retries (default: 2)                                   |

Storage and deployment options are passed via `StorageUploaderConfig` and `Deployment` (see `--help`).

## Address Rotation

The order generator can optionally rotate through multiple addresses when submitting requests. Instead of using a single key for all submissions, it switches to a new address on a schedule (e.g., daily).

### How It Works

- **Multiple keys** are derived from a BIP-39 mnemonic (same derivation as MetaMask, Ledger, etc.).
- **One key** (index 0) acts as the funding source and tops up the current address before each request.
- **Rotation** happens on a fixed interval (e.g., every 24 hours). The current address is chosen by time.
- **Withdrawals** from rotated addresses happen only after all requests from that address have expired. Funds are returned to the funding address, which then tops up the current address as needed.

### Enabling Rotation

Provide a mnemonic and the number of rotation keys:

```bash
--mnemonic "your twelve word phrase here" --derive-rotation-keys 3
```

You also need a funding source. Either:

- Pass `--private-key` with a pre-funded key (e.g., for tests), or
- Use the derived funding key (index 0) and fund it separately.

### Rotation Configuration

| Option                        | Default       | Description                                                      |
| ----------------------------- | ------------- | ---------------------------------------------------------------- |
| `--address-rotation-interval` | 86400 (1 day) | Seconds between rotations                                        |
| `--top-up-market-threshold`   | 0.01          | Market balance threshold for top-up (ETH)                        |
| `--top-up-native-threshold`   | 0.05          | Native ETH threshold for top-up (covers deposit + gas)           |
| `--withdrawal-tx-timeout`     | 60            | Seconds to wait for each sweep transaction before timing out     |
| `--withdrawal-sweep-retries`  | 3             | Number of retry attempts for the native ETH sweep                |

### Safe Withdrawal via Indexer

The generator is stateless — no state file is needed. Instead, before sweeping a rotated address, it queries the Boundless indexer to check whether all requests from that address have settled:

| Request status | Safe to withdraw?          |
| -------------- | -------------------------- |
| `fulfilled`    | Always                     |
| `expired`      | Always                     |
| `locked`       | Always                     |
| `submitted`    | After `now > lock_end`     |

The indexer URL is taken from the deployment config (`INDEXER_URL` env var or `--deployment-chain-id` default). If the indexer is unreachable, the sweep is skipped for that iteration and retried on the next loop.

### Deployment

- Set the `MNEMONIC` environment variable (or pass `--mnemonic`) when using rotation.
- No persistent storage is required — rotation state is derived from the clock and the indexer.
- Fund the funding key; it will top up the rotation addresses as needed.

### Security

- **Mnemonic handling:** Store the mnemonic in a secrets manager or secure env; avoid logging it. The order generator never logs the mnemonic.
