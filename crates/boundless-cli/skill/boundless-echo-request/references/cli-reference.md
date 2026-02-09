# Boundless CLI — Requestor Command Reference

## Installation

```bash
cargo install --locked --git https://github.com/boundless-xyz/boundless \
  boundless-cli --branch release-1.2 --bin boundless
```

Verify: `boundless --version`

## Configuration

### Interactive Setup

```bash
boundless requestor setup
```

Walks through configuring network, RPC URL, private key, and storage provider. Stores config in `~/.boundless/`.

### Environment Variables

| Variable | Description |
|----------|-------------|
| `REQUESTOR_RPC_URL` | RPC endpoint (e.g. Base mainnet or Sepolia) |
| `REQUESTOR_PRIVATE_KEY` | Private key for transactions (hex, with or without 0x prefix) |
| `BOUNDLESS_MARKET_ADDRESS` | Market contract address (optional — has built-in default) |
| `SET_VERIFIER_ADDRESS` | Verifier contract address (optional — has built-in default) |
| `TX_TIMEOUT` | Ethereum transaction timeout in seconds |
| `LOG_LEVEL` | Log level: `error`, `warn`, `info`, `debug`, `trace` |

Env vars override values from `~/.boundless/` config. You can also pass them as CLI flags (e.g. `--requestor-rpc-url`, `--requestor-private-key`).

## Commands

### `boundless requestor config`

Show current requestor configuration status.

```bash
boundless requestor config
```

### `boundless requestor deposit <AMOUNT>`

Deposit ETH into the Boundless Market contract. Required before submitting proof requests.

```bash
boundless requestor deposit 0.005
```

- `AMOUNT` — ETH amount as a decimal string (e.g. `0.005`)

### `boundless requestor withdraw <AMOUNT>`

Withdraw deposited ETH from the market.

```bash
boundless requestor withdraw 0.005
```

### `boundless requestor balance`

Check your deposited balance in the market contract.

```bash
boundless requestor balance
```

Alias: `boundless requestor deposited-balance`

### `boundless requestor submit`

Submit a proof request with inline parameters — program, input, and offer.

```bash
boundless requestor submit \
  --program-url <URL> \
  --input "Hello, Boundless!" \
  --wait
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--program-url <URL>` | Public URL to the guest program ELF (IPFS, HTTP) |
| `-p, --program <PATH>` | Local path to guest program ELF (requires storage provider) |
| `--input <STRING>` | Input string passed as stdin to the guest |
| `--input-file <PATH>` | Input from a file instead of inline string |
| `--encode-input` | Use `risc0_zkvm::serde` to encode input as `Vec<u8>` |
| `-w, --wait` | Block until the request is fulfilled |
| `-o, --offchain` | Submit offchain via order stream (faster, less censorship-resistant) |
| `--no-preflight` | Skip preflight execution and pricing checks (not recommended) |
| `--min-price <WEI>` | Minimum offer price in wei |
| `--max-price <WEI>` | Maximum offer price in wei |
| `--timeout <SECS>` | Request timeout in seconds (default varies) |
| `--lock-timeout <SECS>` | How long a prover can hold the lock |
| `--ramp-up-period <SECS>` | Duration of price ramp-up from min to max |
| `--lock-collateral <WEI>` | Collateral a prover must post when locking |
| `--callback-address <ADDR>` | On-chain callback contract address |
| `--callback-gas-limit <GAS>` | Gas limit for callback execution |
| `--proof-type <TYPE>` | `any` (default), `inclusion`, `groth16`, `blake3-groth16` |

**Storage provider flags** (for `--program` with local ELF):

| Flag | Description |
|------|-------------|
| `--storage-uploader <TYPE>` | `pinata` or `s3` |
| `--pinata-jwt <TOKEN>` | Pinata JWT for IPFS uploads |
| `--s3-bucket <BUCKET>` | S3 bucket name |
| `--s3-region <REGION>` | S3 region |

### `boundless requestor submit-file <YAML_FILE>`

Submit a proof request defined in a YAML file.

```bash
boundless requestor submit-file request.yaml --wait
```

**Flags:**

| Flag | Description |
|------|-------------|
| `-w, --wait` | Block until fulfilled |
| `-o, --offchain` | Submit offchain |
| `--no-preflight` | Skip preflight checks |

See `examples/echo-request.yaml` for the YAML format.

### `boundless requestor status <REQUEST_ID> [EXPIRES_AT]`

Check the status of a proof request.

```bash
boundless requestor status 0x1234...
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--timeline` | Show detailed timeline and order parameters |
| `--search-blocks <N>` | Number of blocks to search backwards (default: 100000) |
| `--search-to-block <N>` | Lower bound block for event search |
| `--search-from-block <N>` | Upper bound block for event search |

### `boundless requestor get-proof <REQUEST_ID>`

Retrieve the journal and seal for a fulfilled request.

```bash
boundless requestor get-proof 0x1234...
```

Returns JSON with:
- **Fulfillment Data** — image ID, journal bytes, metadata
- **Seal** — the cryptographic proof

### `boundless requestor verify-proof <REQUEST_ID>`

Verify the proof of a fulfilled request against the on-chain verifier.

```bash
boundless requestor verify-proof 0x1234...
```

## Networks

Boundless runs on **Base** (mainnet). The CLI defaults to Base mainnet contract addresses. For testing, use Base Sepolia with custom contract addresses.

## Verbose Logging

Prefix any command with `RUST_LOG=info` (or `debug`, `trace`) for detailed output:

```bash
RUST_LOG=info boundless requestor submit --program-url ... --input "test" --wait
```
