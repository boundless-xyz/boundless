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

| Step                 | Command                                                                                                                      |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Check prerequisites  | `bash scripts/check-prerequisites.sh` (from this skill's directory)                                                          |
| Create wallet        | `cast wallet new`                                                                                                            |
| Check balance        | `cast balance <ADDRESS> --rpc-url https://mainnet.base.org`                                                                  |
| Install CLI          | `cargo install --locked --git https://github.com/boundless-xyz/boundless boundless-cli --branch release-1.2 --bin boundless` |
| Configure            | `boundless requestor setup`                                                                                                  |
| Deposit              | `boundless requestor deposit 0.005`                                                                                          |
| Check deposit        | `boundless requestor balance`                                                                                                |
| Discover programs    | `bash scripts/discover-programs.sh` (from this skill's directory)                                                            |
| Build YAML           | Use `scripts/build-request-yaml.sh` or build manually (see Phase 6)                                                          |
| Submit proof request | `boundless requestor submit-file request.yaml --no-preflight`                                                                |
| Check status         | `boundless requestor status <REQUEST_ID>`                                                                                    |
| Get proof            | `boundless requestor get-proof <REQUEST_ID>`                                                                                 |

For full CLI docs, see `references/cli-reference.md`.

## CRITICAL: Use `submit-file`, NOT `submit`

There are two submit commands in the CLI:

| Command                           | Preflight Behavior                                                                                                                                                       | Use For This Skill |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------ |
| `boundless requestor submit`      | **Always runs preflight** — spawns `r0vm` locally to execute the guest program. Can hang for 10+ minutes on even small programs. **No `--no-preflight` flag available.** | ❌ Do NOT use      |
| `boundless requestor submit-file` | Has `--no-preflight` flag to skip local execution. Completes in seconds.                                                                                                 | ✅ Use this        |

The `submit` command (defined in `submit_offer.rs`) passes the request through `client.build_request()` which invokes the `PreflightLayer`. This layer downloads the program, spawns an `r0vm` child process, and executes the guest program locally to compute cycle counts and journal output. On a typical machine this can run at 100% CPU for 10+ minutes and will appear to hang with no output.

The `submit-file` command (defined in `submit.rs`) also calls `build_request()`, but passes `with_skip_preflight(self.no_preflight)` to the client builder, which skips the `PreflightLayer` when `--no-preflight` is set.

**Always use `submit-file` with `--no-preflight` for this skill.**

### Environment Variable: AWS_EC2_METADATA_DISABLED

Set `AWS_EC2_METADATA_DISABLED=true` on all `boundless` commands. Without this, the AWS SDK embedded in the CLI tries to contact the EC2 Instance Metadata Service (IMDS) on startup, causing 2–4 second timeouts with warning spam on non-EC2 machines. This is cosmetic but annoying and adds latency.

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
AWS_EC2_METADATA_DISABLED=true boundless requestor deposit 0.005
```

Check the deposited balance:

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor balance
```

### Discover a Program to Prove

Rather than hardcoding a program URL (which breaks when risc0-zkvm versions change), we query the Boundless indexer for recently fulfilled proof requests and replay one. This guarantees version compatibility — if it was fulfilled recently, current provers can handle it.

Run the discovery script:

```bash
bash /path/to/first-request/scripts/discover-programs.sh
```

This queries the Boundless indexer API, filters for recently fulfilled requests with accessible IPFS program URLs, and outputs the 5 smallest (cheapest/fastest) verified options.

**Present the results to the developer** with these details for each option:

- Program cycles (proxy for complexity/cost)
- Proof cost (the lock price from when it was last fulfilled)
- Image ID (first 16 chars)
- When it was last fulfilled

Let the developer pick one, or enter a custom program URL if they have one.

### Build the YAML Request File

**This is the key step.** We must use `submit-file` with a YAML request to avoid the preflight hang.

Extract from the discovery script's JSON output for the selected request:

- `image_url` — the IPFS URL for the program
- `image_id` — the 64-char hex image ID
- `input_data` — the hex-encoded input (with `0x` prefix)

Then build the YAML request file. Use `scripts/build-request-yaml.sh` or construct it manually:

```bash
bash /path/to/first-request/scripts/build-request-yaml.sh \
  "<IMAGE_URL>" \
  "<IMAGE_ID>" \
  "<INPUT_DATA_HEX>" \
  > /tmp/boundless-request.yaml
```

Or write the YAML manually (see `examples/request.yaml` for the template):

```yaml
id: 0
imageUrl: "<IMAGE_URL>"
input:
  inputType: Inline
  data: "<INPUT_DATA_HEX>"    # Include the 0x prefix
requirements:
  imageId: "<IMAGE_ID>"        # 64-char hex, no 0x prefix
  predicate:
    predicateType: PrefixMatch
    data: "0x<IMAGE_ID>"       # 0x + the same 64-char hex image ID
  callback:
    addr: "0x0000000000000000000000000000000000000000"
    gasLimit: 0
  selector: "00000000"
offer:
  minPrice: 100000000000000     # 0.0001 ETH
  maxPrice: 2000000000000000    # 0.002 ETH
  rampUpStart: <UNIX_TIMESTAMP> # Must be a future timestamp (current time + 30s)
  rampUpPeriod: 300             # 5 min ramp from min to max price
  timeout: 3600                 # 1 hour before request expires
  lockTimeout: 2700             # 45 min for prover to deliver after locking
  lockCollateral: 5000000000000000000  # 5 ETH prover collateral
```

**YAML field gotchas (these WILL cause errors if wrong):**

| Field                         | Requirement                                                                                                                            | Error if Wrong                             |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| `offer.rampUpStart`           | Must be a **future Unix timestamp** (e.g. `$(date +%s) + 30`). Cannot be `0`.                                                          | `offer rampUpStart must be greater than 0` |
| `requirements.predicate.data` | Must be `0x` + the image ID hex (64 chars). For PrefixMatch with empty prefix, this is just the imageId bytes. Cannot be empty string. | `malformed predicate data`                 |
| `requirements.imageId`        | The 64-char hex image ID **without** `0x` prefix.                                                                                      | Image ID mismatch errors                   |
| `input.data`                  | Hex-encoded input data **with** `0x` prefix.                                                                                           | Invalid input / empty journal              |

### Generate the rampUpStart Timestamp

The `rampUpStart` must be a future Unix timestamp. Generate one ~30 seconds in the future:

```bash
echo $(( $(date +%s) + 30 ))
```

Use this value in the YAML. The request will fail if this timestamp is in the past by the time it's submitted.

### Submit the Request

```bash
AWS_EC2_METADATA_DISABLED=true RUST_LOG=info boundless requestor submit-file /tmp/boundless-request.yaml --no-preflight
```

**Why `--no-preflight`?** Without it, the CLI downloads the program and spawns `r0vm` to execute it locally — computing cycle counts and journal output. This is useful for pricing estimation but takes 10+ minutes and will appear to hang. With `--no-preflight`, the CLI skips local execution and submits the request directly. The trade-off is less accurate pricing, but since we're replaying a known-good request with reasonable offer parameters, this is fine.

**Do NOT use `--wait` initially.** If you want to wait for fulfillment, use `--wait`, but be aware this polls indefinitely until fulfillment or expiry. For a first request, it's safer to submit without `--wait` and then poll status manually so you can see what's happening.

**What happens after submission:**

1. **Broadcast** — the request is submitted on-chain to the Boundless Market contract on Base
2. **Auction** — a reverse Dutch auction starts at `rampUpStart`. Price ramps from `minPrice` to `maxPrice` over `rampUpPeriod`
3. **Lock** — a prover bids and locks the request, posting collateral
4. **Prove** — the prover executes the guest program inside the zkVM and generates a proof
5. **Fulfill** — the proof is submitted on-chain, verified by the market contract, and the prover is paid

The output will show a **Request ID** — save this for checking status.

### Poll for Fulfillment

After submitting, check the status:

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor status <REQUEST_ID>
```

Run this every 30–60 seconds. Typical fulfillment takes 1–5 minutes after a prover locks the request.

| Status        | Meaning                                           |
| ------------- | ------------------------------------------------- |
| **Submitted** | Request broadcast, auction running                |
| **Locked**    | A prover has committed to fulfilling this request |
| **Fulfilled** | Proof delivered and verified on-chain ✅          |
| **Expired**   | Request timed out before fulfillment              |

### SDK Code (Educational Context)

For developers who want to understand what the CLI does under the hood, here's the equivalent SDK code. Note the use of `with_skip_preflight(true)` to avoid local execution:

```rust
use boundless_market::Client;

let client = Client::builder()
    .with_rpc_url(rpc_url)
    .with_private_key(private_key)
    .with_skip_preflight(true)  // Skip local r0vm execution
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

## Phase 7: View Results & Celebrate

Once the status shows **Fulfilled**, give the developer the full picture.

### Show the Explorer Link

The Boundless explorer has a rich UI for each order. Build the link from the request ID:

```
https://explorer.boundless.network/orders/<REQUEST_ID>
```

Share this with the developer — it shows the full order lifecycle visually.

### Show the Timeline Summary

Run the status command with `--timeline` to get detailed timing:

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor status <REQUEST_ID> --timeline
```

This outputs timestamps for each lifecycle event: Submitted → Locked → ProofDelivered → Fulfilled.

**Parse the timeline and present a summary like this to the developer:**

```
🎉 Your first Boundless proof is complete!

  🔗 Explorer:   https://explorer.boundless.network/orders/<REQUEST_ID>

  ⏱️  Time to lock:     8s    (how fast a prover picked it up)
  ⏱️  Proving time:     34s   (how long the prover took to generate the proof)
  ⏱️  Total time:       42s   (submit → fulfilled, end to end)

  💰 Price paid:        ~0.0001 ETH
  🖥️  Prover:           0x95566cb7...
```

Calculate the durations by subtracting timestamps from the timeline:

- **Time to lock** = Locked timestamp − Submitted timestamp
- **Proving time** = ProofDelivered timestamp − Locked timestamp
- **Total time** = Fulfilled timestamp − Submitted timestamp

### Retrieve the Proof

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor get-proof <REQUEST_ID>
```

**Journal** — the public output from the guest program. Since we replayed a request with the same input, the journal should match the output from the original request.

**Seal** — the cryptographic proof. Smart contracts use this to verify the computation on-chain.

To verify the proof against the on-chain verifier:

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor verify-proof <REQUEST_ID>
```

### Understanding the Output

The journal bytes contain the output committed by the guest program during execution. Since we replayed a previously fulfilled request with the same program and input, the journal should match the original — confirming that the proof lifecycle worked end to end.

## Additional Resources

- `references/guest-program-explainer.md` — deep dive on guest programs, ZK concepts, and building an ELF locally
- `references/cli-reference.md` — full Boundless CLI requestor command reference
- `references/troubleshooting.md` — common errors and fixes
- [Boundless Docs](https://docs.boundless.network)
- [Boundless GitHub](https://github.com/boundless-xyz/boundless)
- [RISC Zero Developer Docs](https://dev.risczero.com)
