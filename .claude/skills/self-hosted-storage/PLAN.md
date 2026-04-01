# Self-Hosted Request Skill — Plan

## Problem

Every Boundless example and the quick-start docs require a **Pinata account** (IPFS provider) to upload the guest program ELF and large inputs before submitting a proof request. This is friction for developers who just want to try Boundless — they have to sign up for a third-party service, generate a JWT, and configure it before they can submit anything.

## Idea

Use **Cloudflare Quick Tunnels** (`cloudflared tunnel --url`) to serve files from the developer's machine over a public HTTPS URL. Zero auth, zero signup — just install `cloudflared` and go.

The developer's machine becomes the file host:
```
[local files] → [python http.server] → [cloudflared tunnel] → https://xyz.trycloudflare.com
                                                                        ↑
                                                        provers download ELF + input from here
```

## Scope: Two Skill Modes

### Mode 1: "Replay" (zero code, zero build)

Like the existing `first-request` skill — discover a recently fulfilled request from the indexer, but instead of using the original IPFS URL, **download the ELF locally and re-host it through the tunnel**. This proves the tunnel approach works end-to-end with no custom guest program needed.

Good for: absolute first experience, proving the tunnel concept works.

### Mode 2: "Bring Your Own Program"

Developer has a guest program ELF (either built locally or downloaded). The script serves it through the tunnel and submits a request.

Good for: developers with their own guest programs who don't want to set up Pinata/S3.

---

## Merging with `first-request`?

The existing `first-request` skill does:
1. Check prerequisites
2. Wallet setup + funding
3. CLI install + config
4. Discover a program from the indexer
5. Build YAML + submit via `submit-file --no-preflight`
6. Poll for fulfillment

This new skill replaces step 5 with a **long-running serve-and-submit process**. The wallet/CLI/discovery phases are identical. Options:

- **Option A**: Keep `first-request` as-is, make `self-hosted-storage` a separate skill that assumes CLI is already set up. `first-request` = quickest path (uses existing IPFS URLs). `self-hosted-storage` = "I have my own program" or "I don't want Pinata".

- **Option B**: Merge into a single `my-first-request` skill with two tracks. The SKILL.md guides the agent to ask: "Do you have your own guest program, or want to try a pre-existing one?" Then branches. Shared wallet/CLI setup phases, different submission paths.

- **Option C**: `self-hosted-storage` is a standalone tool (the bash script), and `first-request` optionally invokes it when the developer doesn't have Pinata configured.

**Recommendation**: Option A for now. The skills have different triggers and audiences. We can always merge later. The `first-request` skill works fine for replaying — the IPFS URLs are already public. Self-hosting shines when you have **your own program** and no upload provider.

---

## The Script: `boundless-serve`

### Core Script: `scripts/self-host.sh`

A single long-running bash script that acts as a mini-service.

#### Arguments

```
boundless-serve.sh [OPTIONS] <ELF_PATH> [INPUT_PATH]

Arguments:
  ELF_PATH          Path to the guest program ELF binary
  INPUT_PATH        Path to the input file (optional; if omitted, uses empty input)

Options:
  --input-hex HEX   Provide input as hex string instead of file (inlined, no hosting needed)
  --image-id ID     Image ID (64-char hex). Auto-computed from ELF if omitted.
  --port PORT       Local HTTP server port (default: random available port)
  --submit          Automatically submit after tunnel is up (default: just serve)
  --wait            After submit, poll until fulfilled then exit
  --offer-min ETH   Min price in ETH (default: 0.0001)
  --offer-max ETH   Max price in ETH (default: 0.002)
  --timeout SECS    Request timeout in seconds (default: 3600)
```

#### Lifecycle

```
1. Validate prerequisites (cloudflared, python3, boundless CLI)
2. Create temp directory, copy/symlink ELF and input files
3. Start python3 HTTP server on PORT
4. Start cloudflared tunnel → capture public URL from stderr
5. Verify tunnel is live (curl the URL)
6. Print serving info:
   ┌──────────────────────────────────────────────────┐
   │ 🌐 Boundless Self-Hosted Request                 │
   │──────────────────────────────────────────────────│
   │ 📁 Program: ./my-guest.elf (234 KB)             │
   │ 📦 Input:   ./input.bin (1.2 KB)                │
   │ 🔗 Tunnel:  https://abc-xyz.trycloudflare.com   │
   │                                                  │
   │ Program URL: https://abc-xyz.trycloudflare.com/  │
   │              program.elf                          │
   │ Input URL:   https://abc-xyz.trycloudflare.com/  │
   │              input.bin                            │
   │                                                  │
   │ ⚠️  Keep this running until proof is fulfilled!   │
   └──────────────────────────────────────────────────┘

7. If --submit: build YAML and submit via CLI
8. If --wait: poll status every 15s, show live updates
9. On fulfillment: print results, exit cleanly
10. On expiry/error: print diagnostics, exit with error
```

#### SIGINT Handling

```bash
SIGINT_COUNT=0
trap 'handle_sigint' INT

handle_sigint() {
    ((SIGINT_COUNT++))
    remaining=$((3 - SIGINT_COUNT))
    if [[ $SIGINT_COUNT -lt 3 ]]; then
        echo ""
        echo "⚠️  Tunnel is still serving files to provers!"
        echo "   Killing now may cause your request to fail."
        echo "   Press Ctrl-C ${remaining} more time(s) to force quit."
        echo ""
    else
        echo ""
        echo "🛑 Force quitting. Cleaning up..."
        cleanup
        exit 1
    fi
}
```

#### Cleanup

```bash
cleanup() {
    # Kill python HTTP server
    [[ -n "$HTTP_PID" ]] && kill "$HTTP_PID" 2>/dev/null
    # Kill cloudflared
    [[ -n "$TUNNEL_PID" ]] && kill "$TUNNEL_PID" 2>/dev/null
    # Remove temp dir
    [[ -n "$SERVE_DIR" ]] && rm -rf "$SERVE_DIR"
}
trap cleanup EXIT
```

### Helper: `scripts/compute-image-id.sh`

If the developer doesn't provide `--image-id`, we need to compute it from the ELF. Options:
- Use `r0vm` to compute the image ID: `r0vm --elf <path> --image-id`
- Or use the boundless CLI if it has a subcommand for this
- Or require it as an argument (simplest, least magic)

**Decision**: Require `--image-id` for now. The agent can help compute it. We can add auto-detection later.

### Input Strategy

| Input Size | Strategy | Notes |
|---|---|---|
| < 10 KB | **Inline** in YAML as hex | No hosting needed, simpler |
| ≥ 10 KB | **URL** via tunnel | `inputType: Url`, data is the tunnel URL |
| None | **Empty** | Some programs don't need input |

When using inline, the tunnel only serves the ELF — input goes directly in the on-chain tx. This is more robust (one fewer thing to download) and cheaper for small inputs.

---

## Skill Folder Structure

```
.claude/skills/self-hosted-storage/
├── SKILL.md                          # Agent-facing skill doc
├── PLAN.md                           # This file
├── scripts/
│   ├── self-host.sh                  # Main long-running script
│   └── check-prerequisites.sh        # Prereq checker (cloudflared, python3, boundless)
├── references/
│   ├── troubleshooting.md            # Common issues (tunnel fails, firewall, etc.)
│   └── cloudflare-tunnels.md         # How quick tunnels work, limitations
└── examples/
    └── request-template.yaml         # YAML template with URL-based imageUrl
```

## SKILL.md Outline

```
---
name: self-hosted-storage
description: Submit a Boundless proof request using Cloudflare Tunnels
  to self-host the guest program — no Pinata, no S3, no third-party
  storage accounts needed. Triggers include "self-host", "no pinata",
  "skip pinata", "cloudflare tunnel", "serve my program",
  "host my own ELF", "submit without IPFS".
---

# Self-Hosted Proof Request (No Pinata Required)

Phase 1: Check prerequisites (cloudflared, python3, boundless CLI, funded wallet)
Phase 2: Prepare files (ELF binary + optional input)
Phase 3: Launch boundless-serve.sh (starts server + tunnel + submits + polls)
Phase 4: Wait for fulfillment (script handles this)
Phase 5: View results
```

## Prerequisites

- `cloudflared` — `brew install cloudflared` / `curl -L https://github.com/cloudflare/cloudflared/releases/latest/...`
- `python3` — for HTTP server
- `boundless` CLI — already installed and configured (wallet, RPC)
- `curl` — for tunnel health checks

## Open Questions

1. **Should `boundless-serve` be a bash script or a standalone Rust binary?**
   Bash is zero-compile, works immediately, and is easy for agents to modify. Rust would be more robust but adds a build step. **Leaning bash** for the skill, with notes on the SDK equivalent for production use.

2. **Should we also generate a Rust SDK example alongside the bash script?**
   A small Rust example using `Client::builder().with_program_url(tunnel_url)` would show the "production" path. Could live in `examples/` as a reference.

3. **Large input hosting — worth it for v1?**
   Most first requests have small inputs (< 1 KB). We could defer URL-type input hosting and just inline everything for v1. Simplifies the script significantly.

4. **Auto image-id computation?**
   `r0vm` can compute this, but it's another tool dependency. For v1, require it as an arg or have the agent compute it.

5. **Tunnel reliability — what if cloudflared tunnel drops?**
   Quick tunnels are ephemeral and generally stable for the ~5-15 minutes needed. If the tunnel drops mid-prove, the prover can't download the ELF and the request fails (but the developer doesn't lose funds — the request just expires). The script should detect tunnel health and warn.

## Implementation Order

1. Write `scripts/check-prerequisites.sh` — quick, validates environment
2. Write `scripts/self-host.sh` — the main script, core of the skill
3. Write `SKILL.md` — agent-facing documentation
4. Write `references/troubleshooting.md` — common failure modes
5. Write `examples/request-template.yaml` — YAML template
6. Test end-to-end with a real request on Base mainnet
