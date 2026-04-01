---
name: self-hosted-storage
description: The fastest way to get a developer requesting proofs on Boundless. Uses Cloudflare Quick Tunnels to self-host the guest program and input files — no Pinata, no S3, no third-party storage accounts to set up. Ideal for first-time developers who want to go from zero to a fulfilled proof request with minimal friction. Triggers include "submit a request", "request a proof", "self-host", "no pinata", "skip pinata", "quick start", "serve my program", "submit without storage provider", "first proof request", "get started with boundless", "try boundless".
---

# Self-Hosted Storage for Boundless Requests

The fastest path to submitting a proof request on Boundless. Uses Cloudflare Quick Tunnels to self-host the guest program binary and input from the developer's machine — no Pinata, no S3, no third-party storage accounts to sign up for. One script handles serving, submitting, and waiting for fulfillment.

## Important: This Costs Real Money

Boundless runs on **Base Mainnet**. Every transaction uses real ETH on Base.

- **Gas fees**: ~$0.01–0.05 per transaction
- **Proof cost**: ~$0.10–0.50 depending on prover auction dynamics
- **Recommended starting deposit**: $1–5 worth of ETH on Base
- Use a **fresh wallet** — do not use a wallet holding significant funds

## How It Works

```
[guest.bin + input.bin] → [local HTTP server] → [cloudflared tunnel] → https://xyz.trycloudflare.com
                                                                                ↑
                                                                provers download files from here
```

1. A local HTTP server serves the guest program binary and input file
2. `cloudflared` creates a zero-auth quick tunnel exposing them at a public HTTPS URL
3. A proof request YAML is built with `imageUrl` pointing to the tunnel
4. The request is submitted via `boundless requestor submit-file --no-preflight`
5. Provers download the program and input through the tunnel, generate the proof
6. The tunnel stays alive until the request is fulfilled

No signup, no API keys, no JWT tokens. Just `cloudflared` (free, zero-auth).

## Quick Command Reference

| Step                 | Command                                                                                  |
| -------------------- | ---------------------------------------------------------------------------------------- |
| Check prerequisites  | `bash scripts/check-prerequisites.sh` (from this skill's directory)                       |
| Serve + submit + wait| `bash scripts/self-host.sh <PROGRAM.bin> <INPUT> --image-id <ID> --submit --wait`         |
| Serve only (no submit)| `bash scripts/self-host.sh <PROGRAM.bin> <INPUT> --image-id <ID>`                        |
| Check status         | `boundless requestor status <REQUEST_ID>`                                                 |
| Get proof            | `boundless requestor get-proof <REQUEST_ID>`                                              |

## CRITICAL: File Format

The guest program must be the **`.bin` file** (R0BF wrapped format), NOT the raw `.elf` file.

When you build a guest with `cargo build -p guests`, the build produces both:
- `is-even` — raw ELF (starts with `\x7fELF`) ❌ **Do NOT use this**
- `is-even.bin` — R0BF wrapped format (starts with `R0BF`) ✅ **Use this**

The `.bin` file lives at:
```
target/riscv-guest/guests/<name>/riscv32im-risc0-zkvm-elf/release/<name>.bin
```

If you use the raw ELF, you'll get: `Error: Malformed ProgramBinary`

## Phase 1: Check Prerequisites

Run the prerequisite checker:

```bash
bash /path/to/self-hosted-storage/scripts/check-prerequisites.sh
```

**Required tools:**

| Tool | Purpose | Install |
|------|---------|---------|
| `cloudflared` | Creates the public tunnel | `brew install cloudflared` |
| `python3` | Local HTTP server | Usually pre-installed |
| `curl` | Health checks | Usually pre-installed |
| `boundless` | CLI for submitting requests | See Phase 3 |
| `cast` | Wallet management | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |

### Installing cloudflared

**macOS:**
```bash
brew install cloudflared
```

**Linux:**
```bash
# Debian/Ubuntu
curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64.deb -o cloudflared.deb
sudo dpkg -i cloudflared.deb

# Or download the binary directly
curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -o /usr/local/bin/cloudflared
chmod +x /usr/local/bin/cloudflared
```

## Phase 2: Wallet Setup & Funding

The developer needs a funded wallet on Base Mainnet.

**Ask the developer:** Do you already have a wallet with ETH on Base, or do you need to create one?

### Create a New Wallet

```bash
cast wallet new
```

Save the **address** and **private key** securely.

### Fund the Wallet

Options:
1. **Bridge from Ethereum mainnet**: [bridge.base.org](https://bridge.base.org)
2. **CEX withdrawal**: Withdraw ETH directly to Base from Coinbase, Binance, etc.
3. **Transfer from another wallet**

Confirm the balance:
```bash
cast balance <ADDRESS> --rpc-url https://mainnet.base.org
```

## Phase 3: Install & Configure Boundless CLI

### Install

```bash
cargo install --locked --git https://github.com/boundless-xyz/boundless boundless-cli --branch release-1.2 --bin boundless
```

### Configure

Run the interactive setup wizard:

```bash
boundless requestor setup
```

The wizard prompts for:
1. **Network** — select Base Mainnet
2. **RPC URL** — `https://mainnet.base.org` (or an Alchemy/Infura endpoint for better reliability)
3. **Private key** — the wallet private key from Phase 2
4. **Storage provider** — **skip this** (that's the whole point of this skill)

Configuration is stored in `~/.boundless/`. Verify it worked:

```bash
boundless requestor config
```

You should see your network, RPC URL, and address.

Alternatively, for non-interactive setup (useful for agents):

```bash
boundless requestor setup --change-network base-mainnet
boundless requestor setup --set-rpc-url "https://mainnet.base.org"
boundless requestor setup --set-private-key "0x<PRIVATE_KEY>"
```

### Deposit Funds

Deposit ETH into the Boundless Market contract (this is what provers are paid from):

```bash
boundless requestor deposit 0.005
```

Check the deposited balance:
```bash
boundless requestor balance
```

## Phase 4: Build Your Guest Program

If the developer already has a compiled guest `.bin` file and knows the image ID, skip to Phase 5.

### Build from Source

```bash
cd /path/to/your/project
cargo build -p guests
```

### Locate the Binary

The `.bin` file (R0BF format) is at:
```
target/riscv-guest/guests/<guest-name>/riscv32im-risc0-zkvm-elf/release/<guest-name>.bin
```

### Get the Image ID

The build generates a Solidity file with the image ID:
```bash
cat contracts/src/ImageID.sol
```

Look for `bytes32 public constant <NAME>_ID = bytes32(0x...);` — the hex value (without `0x`) is the image ID.

If using the Boundless foundry template (`boundless-foundry-template`), the image ID is in `contracts/src/ImageID.sol` after building.

### Prepare Input

The input file should be the raw bytes your guest program expects on stdin. For example, for a program that takes a U256 ABI-encoded number:

```bash
# ABI-encode the number 42 as U256 (32 bytes, big-endian)
python3 -c "import sys; sys.stdout.buffer.write((42).to_bytes(32, 'big'))" > input.bin
```

Or use `cast`:
```bash
cast abi-encode "f(uint256)" 42 | xxd -r -p > input.bin
```

## Phase 5: Serve & Submit

This is the core step. The `self-host.sh` script handles everything:

### Full Auto (Serve + Submit + Wait)

```bash
bash /path/to/self-hosted-storage/scripts/self-host.sh \
    ./target/riscv-guest/guests/<name>/riscv32im-risc0-zkvm-elf/release/<name>.bin \
    ./input.bin \
    --image-id <64_CHAR_HEX_IMAGE_ID> \
    --submit --wait
```

This will:
1. Start a local HTTP server
2. Start a Cloudflare Tunnel
3. Verify the tunnel is reachable
4. Build a proof request YAML
5. Submit it via `boundless requestor submit-file --no-preflight`
6. Poll for fulfillment every 15 seconds
7. Print results when fulfilled

**The script must stay running until the proof is fulfilled.** It will display:
```
🚀 Boundless Self-Hosted Request
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

📁 Program:   is-even.bin (815.6 KB)
📦 Input:     input.bin (32 B)
🔑 Image ID:  1651fd0e972bff71...fd2111f3
🌐 Tunnel:    https://xyz.trycloudflare.com

📋 Submitting proof request...

✓ Request submitted!
  Request ID: 0x1a2b3c...
  Explorer:   https://explorer.boundless.network/orders/0x1a2b3c...

⏳ Waiting for fulfillment...
⚠️  Keep this running until fulfillment! (Ctrl-C x3 to force quit)

  🔒 Status: Locked — a prover is generating your proof!

🎉 Request fulfilled!
```

### Serve Only (No Submit)

```bash
bash /path/to/self-hosted-storage/scripts/self-host.sh \
    ./program.bin ./input.bin \
    --image-id <IMAGE_ID>
```

This starts the server and tunnel, prints the URLs, and keeps running. Use the URLs in your own YAML or SDK code.

### Pricing Options

| Flag | Default | Description |
|------|---------|-------------|
| `--min-price` | `100000000000000` (0.0001 ETH) | Starting auction price |
| `--max-price` | `2000000000000000` (0.002 ETH) | Maximum auction price |
| `--timeout` | `3600` (1 hour) | Request expiry |
| `--poll-interval` | `15` | Seconds between status checks |

### SIGINT Handling

The script catches Ctrl-C to prevent accidental tunnel death:
- **1st Ctrl-C**: Warning — "tunnel still needed, press 2 more times"
- **2nd Ctrl-C**: Warning — "press 1 more time to force quit"
- **3rd Ctrl-C**: Force quit, cleanup, exit

## Phase 6: View Results

Once fulfilled:

### Explorer Link
```
https://explorer.boundless.network/orders/<REQUEST_ID>
```

### Retrieve the Proof
```bash
boundless requestor get-proof <REQUEST_ID>
```

### Verify the Proof
```bash
boundless requestor verify-proof <REQUEST_ID>
```

## Troubleshooting

### `Malformed ProgramBinary`

You're serving the raw ELF file instead of the `.bin` (R0BF wrapped format). Use the `.bin` file:
```
target/riscv-guest/guests/<name>/riscv32im-risc0-zkvm-elf/release/<name>.bin  ✅
target/riscv-guest/guests/<name>/riscv32im-risc0-zkvm-elf/release/<name>      ❌
```

### `429 Too Many Requests` from Cloudflare

Cloudflare rate-limits quick tunnel creation to roughly **~20 tunnels per hour per IP**. If you hit this:
- Wait 10–15 minutes for the limit to reset
- Don't create/destroy tunnels in rapid succession
- If iterating, keep one tunnel alive and reuse it rather than restarting the script each time

### Tunnel URL not reachable

- Ensure `cloudflared` is not blocked by a firewall (e.g., Little Snitch, corporate firewall)
- The tunnel needs a few seconds after "Registered tunnel connection" before DNS propagates
- Try again — quick tunnels are occasionally slow to become routable

### Request expired without fulfillment

- **Price too low**: Increase `--max-price`. Check recent fulfilled requests on the [explorer](https://explorer.boundless.network) for typical pricing.
- **Tunnel died**: If the tunnel died before the prover downloaded the program, they can't fulfill. Ensure the script stays running.
- **Program too large/complex**: Very large programs may need higher pricing or longer timeouts.

### `offer rampUpStart must be greater than 0`

The YAML `rampUpStart` timestamp is in the past. The script auto-generates this 30 seconds in the future. If submission is slow, try again.



## SDK Equivalent (For Reference)

For developers who want to integrate self-hosting into their Rust application instead of using the bash script:

```rust
use boundless_market::Client;

let client = Client::builder()
    .with_rpc_url(rpc_url)
    .with_private_key(private_key)
    .with_skip_preflight(true)
    .build()
    .await?;

// Use with_program_url to point at your self-hosted tunnel URL
// No storage provider needed — provers download directly from your URL
let request = client
    .new_request()
    .with_program_url("https://xyz.trycloudflare.com/program.bin")?
    .with_input_url("https://xyz.trycloudflare.com/input.bin")?;

let (request_id, expires_at) = client.submit(request).await?;
```

This is the equivalent of what the bash script does under the hood.
