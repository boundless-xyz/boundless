---
name: boundless-overview
description: Context for working with the Boundless decentralized ZK proof marketplace. Triggers include "Boundless", "ZK proof marketplace", "boundless CLI", "proof request", "requestor", "prover", "zkVM".
---

# Boundless CLI Skill

## Overview

Boundless is a decentralized marketplace for zero-knowledge (ZK) proofs built on RISC Zero's zkVM. It connects **requestors** (who need ZK proofs computed) with **provers** (who generate proofs for rewards). The protocol runs on Ethereum L2 (Base) with staking/rewards on Ethereum L1.

## CLI Quick Reference

The `boundless` CLI is the primary interface for interacting with the Boundless protocol.

### Requestor Commands

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

### Prover Commands

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

### Rewards Commands

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

## Reference Files

- [Architecture](references/architecture.md) - Monorepo structure, key crates, build toolchain
- [CLI Reference](references/cli-reference.md) - Detailed command arguments, env vars, config precedence
- [Conventions](references/conventions.md) - Code patterns, error handling, display output
