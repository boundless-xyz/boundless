---
name: boundless-cli
description: >-
  How to use the Boundless CLI — the primary interface for the Boundless ZK
  proof marketplace. Covers requestor, prover, and rewards commands, config,
  env vars, and the monorepo structure. Use when working with the boundless
  CLI, submitting proofs, managing provers, staking rewards, or navigating
  the codebase.
---

# Boundless CLI

The `boundless` CLI is the primary interface for the Boundless protocol — a decentralized marketplace for zero-knowledge proofs built on RISC Zero's zkVM. It connects **requestors** (who need ZK proofs) with **provers** (who generate proofs for rewards). The protocol runs on Base (L2) with staking/rewards on Ethereum (L1).

## Command Quick Reference

### Requestor

```
boundless requestor setup              # Interactive setup wizard
boundless requestor config             # Show configuration status
boundless requestor submit             # Submit a proof request (offer + input + image)
boundless requestor submit-file        # Submit a proof request from a YAML file
boundless requestor status             # Check request status
boundless requestor get-proof          # Download proof journal and seal
boundless requestor verify-proof       # Verify a completed proof
boundless requestor deposit            # Deposit funds into the market
boundless requestor withdraw           # Withdraw funds from the market
boundless requestor balance            # Check deposited balance
```

### Prover

```
boundless prover setup                 # Interactive setup wizard
boundless prover config                # Show configuration status
boundless prover deposit-collateral    # Deposit collateral
boundless prover withdraw-collateral   # Withdraw collateral
boundless prover balance-collateral    # Check collateral balance
boundless prover lock                  # Lock a request
boundless prover fulfill               # Fulfill proof requests
boundless prover execute               # Execute proof via zkVM
boundless prover benchmark             # Run benchmark suite
boundless prover slash                 # Slash a prover for failed request
boundless prover generate-config       # Generate broker/compose configs
```

### Rewards

```
boundless rewards setup                # Interactive setup wizard
boundless rewards config               # Show configuration status
boundless rewards stake-zkc            # Stake ZKC tokens
boundless rewards balance-zkc          # Check ZKC balance
boundless rewards staked-balance-zkc   # Check staked ZKC balance
boundless rewards epoch                # Get current epoch info
boundless rewards power                # Check reward power
boundless rewards prepare-mining       # Prepare mining work log
boundless rewards submit-mining        # Submit mining work updates
boundless rewards claim-mining-rewards # Claim mining rewards
boundless rewards claim-staking-rewards # Claim staking rewards
boundless rewards delegate             # Delegate rewards
boundless rewards get-delegate         # Get current delegate
boundless rewards list-staking-rewards # List staking rewards by epoch
boundless rewards list-mining-rewards  # List mining rewards by epoch
boundless rewards inspect-mining-state # Inspect mining state file
```

## Configuration

Config is stored in `~/.boundless/`:

- `config.toml` — public config (selected networks, custom deployments)
- `secrets.toml` — private config (RPC URLs, private keys), 0600 permissions

**Precedence** (highest to lowest):
1. CLI flags
2. Environment variables (e.g. `REQUESTOR_RPC_URL`)
3. Config files (`~/.boundless/secrets.toml`, `config.toml`)
4. Network defaults (built-in contract addresses)

**Setup wizards** configure everything interactively:

```bash
boundless requestor setup
boundless prover setup
boundless rewards setup
```

**Tip:** Set `AWS_EC2_METADATA_DISABLED=true` on all `boundless` commands to avoid 2–4s AWS IMDS timeout warnings on non-EC2 machines.

## Key Environment Variables

| Variable | Module | Description |
|----------|--------|-------------|
| `REQUESTOR_RPC_URL` | Requestor | RPC endpoint for requestor commands |
| `REQUESTOR_PRIVATE_KEY` | Requestor | Private key for requestor transactions |
| `PROVER_RPC_URL` | Prover | RPC endpoint for prover commands |
| `PROVER_PRIVATE_KEY` | Prover | Private key for prover transactions |
| `BENTO_API_URL` | Prover | Bento/Bonsai API URL |
| `BENTO_API_KEY` | Prover | Bento/Bonsai API key |
| `REWARD_RPC_URL` | Rewards | RPC endpoint for rewards (L1) |
| `REWARD_PRIVATE_KEY` | Rewards | Private key for reward transactions |
| `STAKING_PRIVATE_KEY` | Rewards | Private key for staking (can differ from reward key) |
| `STAKING_ADDRESS` | Rewards | Staking address (read-only) |
| `MINING_STATE_FILE` | Rewards | Path to mining state file |
| `ZKC_ADDRESS` | Rewards | ZKC token contract (has default per network) |
| `VEZKC_ADDRESS` | Rewards | Staked ZKC NFT contract (has default per network) |
| `STAKING_REWARDS_ADDRESS` | Rewards | Rewards distribution contract (has default per network) |
| `BEACON_API_URL` | Rewards | Beacon API URL |
| `BOUNDLESS_MARKET_ADDRESS` | Global | Market contract address (has default per network) |
| `SET_VERIFIER_ADDRESS` | Global | Verifier contract address (has default per network) |
| `TX_TIMEOUT` | Global | Transaction timeout in seconds (default: 300) |
| `LOG_LEVEL` | Global | Log verbosity: error, warn, info, debug, trace |

## Networks

| Network | ID | Description |
|---------|-----|-------------|
| Base Mainnet | `base-mainnet` | Production market |
| Base Sepolia | `base-sepolia` | Market testnet |
| Ethereum Mainnet | `eth-mainnet` | Rewards/staking (L1) |
| Ethereum Sepolia | `eth-sepolia` | Rewards testnet |

## Related Skills

For guided workflows that build on the CLI:

- **requesting** — Walk through submitting a proof request end-to-end (self-hosted storage, no Pinata needed)
- **setup-prover** — Deploy a prover to a GPU server using Ansible

## Reference Files

- [CLI Reference](references/cli-reference.md) — Detailed command arguments, flags, and env vars
- [Architecture](references/architecture.md) — Monorepo structure, key crates, build toolchain
- [Conventions](references/conventions.md) — Code patterns for contributing to the CLI (Rust patterns, error handling, display output, testing)
