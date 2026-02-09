---
name: first-request
description: Guides a developer through submitting their first ZK proof on the Boundless market. Triggers include "first request", "first proof", "try Boundless", "submit a proof", "request a ZK proof", "prove something on Boundless", "set up Boundless CLI", "hello world ZK proof".
---

# Your First Proof Request

Walk the developer through discovering a recently fulfilled proof request on the Boundless market and replaying it to get their first proof fulfilled.

## Important: This Costs Real Money

Boundless runs on **Base Mainnet**. Every transaction uses real ETH on Base.

- **Gas fees**: ~$0.01–0.05 per transaction
- **Proof cost**: ~$0.10–0.50 depending on prover auction dynamics
- **Recommended starting deposit**: $1–5 worth of ETH on Base
- Use a **fresh wallet** — do not use a wallet holding significant funds

Make sure the developer understands this before proceeding. If they want to experiment without cost, point them to the [Boundless SDK examples](https://github.com/boundless-xyz/boundless/tree/main/examples) which can run against a local test environment.

## How This Works

Rather than hardcoding a specific program, this skill discovers a recently fulfilled request from the Boundless market and replays it:

1. **Query the Boundless indexer** for recently fulfilled proof requests
2. **Pick one** with an accessible IPFS program URL and known-good inputs
3. **Replay it** — submit a new request with the same program and input
4. **Guaranteed to work** — if provers fulfilled it recently, they can handle it again

This approach avoids version-compatibility issues and always uses a program that current provers support.

For a deeper explanation of guest programs, journals, seals, and how proofs work, see `references/guest-program-explainer.md`.

## Quick Command Reference

| Step | Command |
|------|---------|
| Check prerequisites | `bash scripts/check-prerequisites.sh` (from this skill's directory) |
| Create wallet | `cast wallet new` |
| Check balance | `cast balance <ADDRESS> --rpc-url https://mainnet.base.org` |
| Install CLI | `cargo install --locked --git https://github.com/boundless-xyz/boundless boundless-cli --branch release-1.2 --bin boundless` |
| Configure | `boundless requestor setup` |
| Deposit | `boundless requestor deposit 0.005` |
| Check deposit | `boundless requestor balance` |
| Discover programs | `bash scripts/discover-programs.sh` (from this skill's directory) |
| Submit proof request | `boundless requestor submit --program-url <URL> --input-file <INPUT_FILE> --wait` |
| Check status | `boundless requestor status <REQUEST_ID>` |
| Get proof | `boundless requestor get-proof <REQUEST_ID>` |

For full CLI docs, see `references/cli-reference.md`.

## Phase 1: Check Prerequisites

Run the prerequisite checker to verify the developer has the required tools:

```bash
bash /path/to/first-request/scripts/check-prerequisites.sh
```

**Required tools:**
- **Rust** (`rustc`, `cargo`) — for building the Boundless CLI
- **Foundry** (`cast`) — for wallet management and chain interaction
- **rzup** — RISC Zero toolchain manager
- **Python 3** (`python3`) — for JSON parsing in the program discovery script

If anything is missing, the script prints the install command. Wait for the developer to install missing tools before continuing.

**Install commands if needed:**
```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Foundry
curl -L https://foundry.paradigm.xyz | bash && foundryup

# RISC Zero
curl -L https://risczero.com/install | bash && rzup install
```

After installing, re-run the check script to confirm.

## Phase 2: Wallet Setup

The developer needs a funded wallet on Base. Help them create one if they don't have one already.

**Ask the developer:** Do you already have a wallet with ETH on Base, or do you need to create one?

### Create a New Wallet

```bash
cast wallet new
```

This outputs an address and private key. Tell the developer to **save the private key securely** — they'll need it for configuration.

### Export the Private Key

If they have an existing wallet, they'll need to export the private key. This depends on their wallet software (MetaMask: Settings > Security > Reveal private key).

### Check Balance

```bash
cast balance <ADDRESS> --rpc-url https://mainnet.base.org
```

The balance needs to cover gas + proof costs. Minimum ~0.005 ETH recommended to start.

## Phase 3: Fund the Wallet

If the wallet balance is zero or insufficient, the developer needs to get ETH on Base.

**Options:**
1. **Bridge from Ethereum mainnet**: [bridge.base.org](https://bridge.base.org) — bridge ETH from L1 to Base
2. **CEX withdrawal**: Withdraw ETH directly to Base from Coinbase, Binance, etc. (select Base network)
3. **Already have Base ETH**: Transfer from another wallet

After funding, confirm the balance:

```bash
cast balance <ADDRESS> --rpc-url https://mainnet.base.org
```

Wait until the balance shows a non-zero value before proceeding.

## Phase 4: Install Boundless CLI

```bash
cargo install --locked --git https://github.com/boundless-xyz/boundless \
  boundless-cli --branch release-1.2 --bin boundless
```

This compiles from source and takes a few minutes. Verify the installation:

```bash
boundless --version
```

If the install fails, see `references/troubleshooting.md` for common cargo/build issues.

## Phase 5: Configure the CLI

Run the interactive setup wizard:

```bash
boundless requestor setup
```

The wizard prompts for:
1. **Network** — select Base Mainnet
2. **RPC URL** — Base mainnet RPC endpoint. Public: `https://mainnet.base.org`. For better reliability, use Alchemy or Infura with a free Base RPC key.
3. **Private key** — the wallet private key from Phase 2
4. **Storage provider** (optional) — Pinata for IPFS uploads. If the developer has a Pinata account, enter the JWT. Otherwise skip — not needed since we use a pre-uploaded program URL from the discovery script.

Configuration is stored in `~/.boundless/`. To verify:

```bash
boundless requestor config
```

Alternatively, set environment variables instead of using the wizard:

```bash
export REQUESTOR_RPC_URL="https://mainnet.base.org"
export REQUESTOR_PRIVATE_KEY="0x..."
```

## Phase 6: Submit Your First Proof Request

This is the core step. The developer submits a proof request to the Boundless market.

### Deposit Funds

First, deposit ETH into the Boundless Market contract. This is the balance provers are paid from.

```bash
boundless requestor deposit 0.005
```

Check the deposited balance:

```bash
boundless requestor balance
```

### Discover a Program to Prove

Rather than hardcoding a program URL (which breaks when risc0-zkvm versions change), we query the Boundless indexer for recently fulfilled proof requests and replay one. This guarantees version compatibility — if it was fulfilled recently, current provers can handle it.

Run the discovery script:

```bash
bash /path/to/first-request/scripts/discover-programs.sh
```

This queries the Boundless indexer API, filters for recently fulfilled requests with accessible IPFS program URLs, and outputs the 5 smallest (cheapest/fastest) verified options.

**Present the results to the developer using `AskUserQuestion`** with these details for each option:
- Program cycles (proxy for complexity/cost)
- Proof cost (the lock price from when it was last fulfilled)
- Image ID (first 16 chars)
- When it was last fulfilled

Let the developer pick one, or enter a custom program URL if they have one.

### Prepare the Input

Extract the `image_url` and `input_data` from the selected request. The `input_data` is hex-encoded and must be decoded to a binary file:

```bash
# Strip the 0x prefix if present and decode hex to binary
echo "<INPUT_DATA_HEX>" | sed 's/^0x//' | xxd -r -p > /tmp/boundless-input.bin
```

### Submit the Request

```bash
RUST_LOG=info boundless requestor submit \
  --program-url <SELECTED_IMAGE_URL> \
  --input-file /tmp/boundless-input.bin \
  --wait
```

We replay the original request's input exactly — this ensures the program receives valid input that it has already successfully processed.

**What happens next:**

1. **Broadcast** — the request is submitted on-chain to the Boundless Market contract on Base
2. **Auction** — a reverse Dutch auction starts. Price ramps from the minimum up to the maximum over the ramp-up period
3. **Lock** — a prover bids and locks the request, posting collateral
4. **Prove** — the prover executes the guest program inside the zkVM and generates a proof
5. **Fulfill** — the proof is submitted on-chain, verified by the market contract, and the prover is paid

The `--wait` flag blocks until fulfillment, polling every 5 seconds. This typically takes 1–5 minutes.

### SDK Code (Educational Context)

For developers who want to understand what the CLI does under the hood, here's the equivalent SDK code from the Boundless repo's counter example:

```rust
use boundless_market::Client;

let client = Client::builder()
    .with_rpc_url(rpc_url)
    .with_private_key(private_key)
    .build()
    .await?;

let request = client.new_request()
    .with_program_url(program_url)?
    .with_stdin(&input_bytes);

let (request_id, expires_at) = client.submit(request).await?;

let fulfillment = client
    .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
    .await?;
```

The SDK's `.submit()` attempts offchain submission first, falling back to on-chain.

## Phase 7: View Results

When the request is fulfilled, the CLI prints the fulfillment data and seal.

**Journal** — the public output from the guest program. Since we replayed a request with the same input, the journal should match the output from the original request.

**Seal** — the cryptographic proof. Smart contracts use this to verify the computation on-chain.

To retrieve the proof again later:

```bash
boundless requestor get-proof <REQUEST_ID>
```

To verify the proof against the on-chain verifier:

```bash
boundless requestor verify-proof <REQUEST_ID>
```

### Understanding the Output

The journal bytes contain the output committed by the guest program during execution. Since we replayed a previously fulfilled request with the same program and input, the journal should match the original — confirming that the proof lifecycle worked end to end.

## Status Reference

If using `boundless requestor status <REQUEST_ID>`:

| Status | Meaning |
|--------|---------|
| **Submitted** | Request broadcast, auction running |
| **Locked** | A prover has committed to fulfilling this request |
| **Fulfilled** | Proof delivered and verified on-chain |
| **Expired** | Request timed out before fulfillment |

## Additional Resources

- `references/guest-program-explainer.md` — deep dive on guest programs, ZK concepts, and building an ELF locally
- `references/cli-reference.md` — full Boundless CLI requestor command reference
- `references/troubleshooting.md` — common errors and fixes
- [Boundless Docs](https://docs.boundless.network)
- [Boundless GitHub](https://github.com/boundless-xyz/boundless)
- [RISC Zero Developer Docs](https://dev.risczero.com)
