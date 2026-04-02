---
name: requesting
description: >-
  Submit proof requests on the Boundless ZK proof marketplace. Covers wallet
  setup, CLI configuration, building or discovering guest programs, self-hosting
  via Cloudflare Quick Tunnels (no Pinata/S3 needed), submitting, and retrieving
  results. Use when a developer wants to request a ZK proof, submit a proof
  request, get started with Boundless, try Boundless, or learn the requestor
  workflow.
---

# Requesting Proofs on Boundless

Guide a developer through submitting a ZK proof request on the Boundless market — from wallet setup through fulfilled proof.

## Important: This Costs Real Money

Boundless runs on **Base Mainnet**. Every transaction uses real ETH on Base.

- **Gas fees**: ~$0.01–0.05 per transaction
- **Proof cost**: ~$0.10–0.50 depending on prover auction dynamics
- **Recommended starting deposit**: $1–5 worth of ETH on Base
- Use a **fresh wallet** — do not use a wallet holding significant funds

If they want to experiment without cost, point them to the [Boundless SDK examples](https://github.com/boundless-xyz/boundless/tree/main/examples) which can run against a local test environment.

## Two Paths

Ask the developer which applies:

| Path | When to use | What happens |
|------|-------------|--------------|
| **A — Bring your own program** | Developer has (or wants to build) a guest program | Build the guest, self-host the binary via Cloudflare tunnel, submit |
| **B — Just trying it out** | Developer wants to see Boundless work end-to-end | Discover a recently fulfilled request and replay it |

Both paths share the same setup (Phases 1–4) and submission flow (Phase 6+). They differ only in where the program comes from (Phase 5).

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
| **Path A** — Self-host + submit | `bash scripts/self-host.sh ./program.bin ./input.bin --image-id <ID> --submit --wait` |
| **Path B** — Discover programs | `bash scripts/discover-programs.sh` |
| **Path B** — Build YAML | `bash scripts/build-request-yaml.sh <URL> <IMAGE_ID> <HEX>` |
| Submit (manual) | `boundless requestor submit-file request.yaml --no-preflight` |
| Check status | `boundless requestor status <REQUEST_ID>` |
| Get proof | `boundless requestor get-proof <REQUEST_ID>` |

For full CLI docs, see `references/cli-reference.md`.

## CRITICAL: Use `submit-file`, NOT `submit`

| Command | Preflight Behavior | Use |
|---------|-------------------|-----|
| `boundless requestor submit` | **Always runs preflight** — spawns `r0vm` locally. Can hang 10+ minutes. **No `--no-preflight` flag.** | ❌ Do NOT use |
| `boundless requestor submit-file` | Has `--no-preflight` flag to skip local execution. Completes in seconds. | ✅ Use this |

**Always use `submit-file` with `--no-preflight`.**

### Environment Variable: AWS_EC2_METADATA_DISABLED

Set `AWS_EC2_METADATA_DISABLED=true` on all `boundless` commands. Without this, the AWS SDK embedded in the CLI tries to contact the EC2 IMDS on startup, causing 2–4 second timeouts with warning spam on non-EC2 machines.

## Phase 1: Check Prerequisites

```bash
bash /path/to/requesting/scripts/check-prerequisites.sh
```

**Required tools:**

| Tool | Purpose | Install |
|------|---------|---------|
| Rust (`rustc`, `cargo`) | Building the Boundless CLI | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Foundry (`cast`) | Wallet management, chain interaction | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| `cloudflared` | Cloudflare tunnel for self-hosting (Path A) | `brew install cloudflared` |
| Python 3 | HTTP server, JSON parsing in scripts | Usually pre-installed |
| `curl` | Health checks | Usually pre-installed |

**Optional (installed during walkthrough):**

| Tool | Purpose | Install |
|------|---------|---------|
| Boundless CLI | Submitting requests | `cargo install --locked --git https://github.com/boundless-xyz/boundless boundless-cli --branch release-1.2 --bin boundless` |
| `rzup` | RISC Zero toolchain (Path A, if building a guest) | `curl -L https://risczero.com/install \| bash && rzup install` |

## Phase 2: Wallet Setup

The developer needs a funded wallet on Base.

**Ask the developer:** Do you already have a wallet with ETH on Base, or do you need to create one?

### Create a New Wallet

```bash
cast wallet new
```

Save the **address** and **private key** securely.

### Fund the Wallet

Options:
1. **Bridge from Ethereum mainnet**: [bridge.base.org](https://bridge.base.org)
2. **CEX withdrawal**: Withdraw ETH directly to Base from Coinbase, Binance, etc. (select Base network)
3. **Transfer from another wallet**

Confirm the balance:

```bash
cast balance <ADDRESS> --rpc-url https://mainnet.base.org
```

Minimum ~0.005 ETH recommended to start.

## Phase 3: Install & Configure CLI

### Install

```bash
cargo install --locked --git https://github.com/boundless-xyz/boundless \
  boundless-cli --branch release-1.2 --bin boundless
```

Verify: `boundless --version`

If install fails, see `references/troubleshooting.md`.

### Configure

Run the interactive setup wizard:

```bash
boundless requestor setup
```

The wizard prompts for:
1. **Network** — select Base Mainnet
2. **RPC URL** — `https://mainnet.base.org` (or Alchemy/Infura for better reliability)
3. **Private key** — from Phase 2
4. **Storage provider** — **skip this** (we self-host instead)

Verify: `boundless requestor config`

For non-interactive setup:

```bash
boundless requestor setup --change-network base-mainnet
boundless requestor setup --set-rpc-url "https://mainnet.base.org"
boundless requestor setup --set-private-key "0x<PRIVATE_KEY>"
```

## Phase 4: Deposit Funds

Deposit ETH into the Boundless Market contract (provers are paid from this balance):

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor deposit 0.005
```

Check the deposited balance:

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor balance
```

---

## Phase 5A: Bring Your Own Program (Self-Hosted)

Use this path when the developer has their own guest program (or wants to build one). Files are served via a Cloudflare Quick Tunnel — no Pinata, no S3, no third-party storage accounts.

### How It Works

```
[program.bin + input.bin] → [local HTTP server] → [cloudflared tunnel] → https://xyz.trycloudflare.com
                                                                                ↑
                                                                provers download from here
```

1. A local HTTP server serves the guest program and input
2. `cloudflared` creates a zero-auth quick tunnel exposing them at a public HTTPS URL
3. A proof request is built with `imageUrl` pointing to the tunnel
4. Provers download the files, generate the proof
5. The tunnel stays alive until fulfillment

No signup, no API keys, no JWT tokens.

### CRITICAL: File Format

The guest program must be the **`.bin` file** (R0BF wrapped format), NOT the raw `.elf`.

When you build a guest with `cargo build -p guests`, the build produces both:
- `is-even` — raw ELF (starts with `\x7fELF`) ❌ **Do NOT use**
- `is-even.bin` — R0BF wrapped format (starts with `R0BF`) ✅ **Use this**

The `.bin` file lives at:
```
target/riscv-guest/guests/<name>/riscv32im-risc0-zkvm-elf/release/<name>.bin
```

### Build Your Guest Program

If the developer already has a compiled `.bin` and image ID, skip to "Serve & Submit".

```bash
cd /path/to/your/project
cargo build -p guests
```

Get the image ID from the generated Solidity file:

```bash
cat contracts/src/ImageID.sol
```

Look for `bytes32 public constant <NAME>_ID = bytes32(0x...);` — the hex value (without `0x`) is the image ID.

### Prepare Input

The input file should be the raw bytes your guest program expects on stdin:

```bash
# ABI-encode the number 42 as U256
cast abi-encode "f(uint256)" 42 | xxd -r -p > input.bin
```

### Serve & Submit

**Full auto (serve + submit + wait):**

```bash
bash /path/to/requesting/scripts/self-host.sh \
    ./target/riscv-guest/guests/<name>/riscv32im-risc0-zkvm-elf/release/<name>.bin \
    ./input.bin \
    --image-id <64_CHAR_HEX_IMAGE_ID> \
    --submit --wait
```

The script will:
1. Start a local HTTP server
2. Start a Cloudflare Tunnel
3. Verify the tunnel is reachable
4. Build a proof request YAML
5. Submit via `boundless requestor submit-file --no-preflight`
6. Poll for fulfillment every 15 seconds
7. Print results when fulfilled

**The script must stay running until the proof is fulfilled.** Ctrl-C is guarded (3× to force quit) so you don't accidentally kill the tunnel.

**Serve only (no submit):**

```bash
bash /path/to/requesting/scripts/self-host.sh \
    ./program.bin ./input.bin --image-id <IMAGE_ID>
```

Prints the public URLs for use in your own YAML or SDK code.

**Pricing options:**

| Flag | Default | Description |
|------|---------|-------------|
| `--min-price` | `100000000000000` (0.0001 ETH) | Starting auction price |
| `--max-price` | `2000000000000000` (0.002 ETH) | Maximum auction price |
| `--timeout` | `3600` (1 hour) | Request expiry |
| `--poll-interval` | `15` | Seconds between status checks |

Skip to **Phase 6** once the script is running (or if you chose serve-only and want to submit manually).

---

## Phase 5B: Just Trying It Out (Replay a Recent Request)

Use this path when the developer doesn't have their own program and just wants to see Boundless work. We discover a recently fulfilled request and replay it — guaranteed to work since provers handled it recently.

### Discover a Program

```bash
bash /path/to/requesting/scripts/discover-programs.sh
```

This queries the Boundless indexer for recently fulfilled requests, filters for ones with accessible IPFS URLs, and outputs the 5 smallest (cheapest/fastest) verified options.

**Present the results** with:
- Program cycles (proxy for complexity/cost)
- Proof cost (lock price from last fulfillment)
- Image ID (first 16 chars)
- When it was last fulfilled

Let the developer pick one.

### Build the YAML Request File

Extract from the discovery script's JSON output:
- `image_url` — the IPFS URL for the program
- `image_id` — the 64-char hex image ID
- `input_data` — the hex-encoded input (with `0x` prefix)

```bash
bash /path/to/requesting/scripts/build-request-yaml.sh \
  "<IMAGE_URL>" "<IMAGE_ID>" "<INPUT_DATA_HEX>" \
  > /tmp/boundless-request.yaml
```

Or write YAML manually — see `examples/request.yaml` for the template.

**YAML field gotchas:**

| Field | Requirement | Error if Wrong |
|-------|-------------|----------------|
| `offer.rampUpStart` | Must be a **future Unix timestamp** (e.g. `$(date +%s) + 30`). Cannot be `0`. | `offer rampUpStart must be greater than 0` |
| `requirements.predicate.data` | Must be `0x` + image ID hex (64 chars). Cannot be empty. | `malformed predicate data` |
| `requirements.imageId` | 64-char hex **without** `0x` prefix | Image ID mismatch |
| `input.data` | Hex-encoded input **with** `0x` prefix | Invalid input / empty journal |

Generate `rampUpStart`:

```bash
echo $(( $(date +%s) + 30 ))
```

### Submit

```bash
AWS_EC2_METADATA_DISABLED=true RUST_LOG=info boundless requestor submit-file /tmp/boundless-request.yaml --no-preflight
```

**Do NOT use `--wait` initially.** Submit without it and poll status manually so you can see what's happening.

---

## Phase 6: Monitor & Retrieve Results

### Poll for Fulfillment

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor status <REQUEST_ID>
```

Run every 30–60 seconds. Typical fulfillment takes 1–5 minutes after lock.

| Status | Meaning |
|--------|---------|
| **Submitted** | Request broadcast, auction running |
| **Locked** | A prover has committed to fulfilling |
| **Fulfilled** | Proof delivered and verified on-chain ✅ |
| **Expired** | Request timed out before fulfillment |

### View on Explorer

```
https://explorer.boundless.network/orders/<REQUEST_ID>
```

### Show Timeline

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor status <REQUEST_ID> --timeline
```

Present a summary:

```
🎉 Your proof is complete!

  🔗 Explorer:   https://explorer.boundless.network/orders/<REQUEST_ID>

  ⏱️  Time to lock:     8s
  ⏱️  Proving time:     34s
  ⏱️  Total time:       42s

  💰 Price paid:        ~0.0001 ETH
```

### Retrieve the Proof

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor get-proof <REQUEST_ID>
```

- **Journal** — the public output committed by the guest program
- **Seal** — the cryptographic proof (used by smart contracts for on-chain verification)

### Verify the Proof

```bash
AWS_EC2_METADATA_DISABLED=true boundless requestor verify-proof <REQUEST_ID>
```

## Troubleshooting

See `references/troubleshooting.md` for a full list. Common issues:

| Problem | Fix |
|---------|-----|
| `submit` hangs with no output | Use `submit-file --no-preflight` instead |
| `Malformed ProgramBinary` | Use the `.bin` file (R0BF format), not the raw `.elf` |
| AWS IMDS timeout warnings | Set `AWS_EC2_METADATA_DISABLED=true` |
| Cloudflare `429 Too Many Requests` | Wait 10–15 min. Don't create/destroy tunnels rapidly. |
| Tunnel URL not reachable | Check firewall. Wait a few seconds for DNS. Retry. |
| Request expired | Increase `--max-price`. Ensure tunnel stayed alive (Path A). |
| `rampUpStart must be greater than 0` | Use a future Unix timestamp: `echo $(( $(date +%s) + 30 ))` |

## SDK Equivalent (Reference)

```rust
use boundless_market::Client;

let client = Client::builder()
    .with_rpc_url(rpc_url)
    .with_private_key(private_key)
    .with_skip_preflight(true)
    .build()
    .await?;

// Path A: self-hosted via tunnel
let request = client.new_request()
    .with_program_url("https://xyz.trycloudflare.com/program.bin")?
    .with_input_url("https://xyz.trycloudflare.com/input.bin")?;

// Path B: replay from IPFS
let request = client.new_request()
    .with_program_url(ipfs_url)?
    .with_stdin(&input_bytes);

let (request_id, expires_at) = client.submit(request).await?;

let fulfillment = client
    .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
    .await?;
```

## Additional Resources

- `references/guest-program-explainer.md` — deep dive on guest programs, ZK concepts, building an ELF
- `references/cli-reference.md` — full requestor command reference
- `references/troubleshooting.md` — common errors and fixes
- `examples/request.yaml` — YAML request template
- [Boundless Docs](https://docs.boundless.network)
- [Boundless GitHub](https://github.com/boundless-xyz/boundless)
- [RISC Zero Developer Docs](https://dev.risczero.com)
