# VersionRegistry Operations Runbook

An operations guide for the VersionRegistry contract — deployment, management, verification, and troubleshooting.

> [!NOTE]
> All commands assume your current working directory is the root of the repo.

## Overview

The VersionRegistry is a UUPS-upgradeable, Ownable2Step contract that stores:

- **Minimum broker version** — a packed semver (major.minor.patch). Brokers poll this every 10 minutes and shut down gracefully if their version is below the minimum.
- **Governance notice** — a free-text string displayed to brokers on each version check. Used for advance warnings (e.g. "Upgrade to v1.4.0 by March 28"). Empty string = no notice.

The broker reads the registry via `getVersionInfo()` which returns both values in a single RPC call.

### How the broker uses it

The broker's `VersionCheckTask` (`crates/broker/src/version_check.rs`):

1. Looks up the registry address from a **hardcoded table** keyed by chain ID (not configurable, to prevent bypass).
2. Calls `getVersionInfo()` on startup and every 10 minutes.
3. If the broker version < on-chain minimum, the supervisor faults and the broker shuts down with error `[B-VER-001]`.
4. If the RPC call fails (contract not deployed, network error), it logs a warning and continues.
5. If `RISC0_DEV_MODE=1`, the version check is skipped entirely.

### Version encoding

Semver components are packed into a `uint64`: `(major << 32) | (minor << 16) | patch`. This preserves natural ordering — comparing packed values gives correct semver ordering. Components are `uint16` (0–65535 each). The packed value must fit in 48 bits.

## Dependencies

- [Foundry](https://book.getfoundry.sh/getting-started/installation) (`forge`, `cast`)
- [yq v4+](https://github.com/mikefarah/yq) — TOML parsing in bash
- Python 3 with `tomlkit` (`pip install tomlkit`) — TOML updates

## Configuration

### `contracts/deployment-version-registry.toml`

The source of truth for VersionRegistry state per chain. Each section contains:

```toml
[deployment.ethereum-sepolia]
name = "Ethereum Sepolia [prod]"
id = 11155111

# Accounts
admin = "0x..."            # Owner / governance multisig

# Contracts
version-registry = "0x..."       # Proxy address
version-registry-impl = "0x..."  # Current implementation address

# Version policy
minimum-broker-version = "1.2.3"  # Desired minimum version (semver string)
notice = ""                        # Governance notice (empty = no notice)
```

### `contracts/deployment_secrets.toml`

RPC URLs and Etherscan API keys (not committed to git):

```toml
[chains.ethereum-sepolia]
rpc-url = "https://..."
etherscan-api-key = "..."
```

## Environment Setup

Set the target chain:

```sh
export CHAIN_KEY="ethereum-sepolia"
# Optional: for multi-deployment environments
export STACK_TAG="staging"
```

Set the deployer key (required for all broadcast operations):

```sh
export DEPLOYER_PRIVATE_KEY="0x..."
```

The `manage-version-registry` script automatically loads `RPC_URL`, `ETHERSCAN_API_KEY`, and `CHAIN_ID` from the TOML files. You can override any of them via environment variables.

> [!NOTE]
> Running commands without `--broadcast` executes in simulation mode (dry run). Add `--broadcast` to send transactions.

## Operations

### Deploy a new VersionRegistry

**Prerequisites:**

- Set the `admin` address in `deployment-version-registry.toml` for the target chain.

```sh
CHAIN_KEY=ethereum-sepolia \
DEPLOYER_PRIVATE_KEY=0x... \
  ./contracts/scripts/manage-version-registry DeployVersionRegistry --broadcast
```

This deploys both the implementation and an ERC1967Proxy, initializes with the admin from the TOML, and writes the proxy + impl addresses back to the TOML.

**After deployment**, update the broker's hardcoded registry table in `crates/broker/src/version_check.rs`:

```rust
const VERSION_REGISTRIES: &[(u64, Address)] = &[
    (11155111, address!("0x<deployed-proxy-address>")), // Sepolia
];
```

### Verify the contract on Etherscan

```sh
CHAIN_KEY=ethereum-sepolia \
  ./contracts/scripts/verify-version-registry.sh
```

This verifies both the implementation contract and the ERC1967Proxy. No `DEPLOYER_PRIVATE_KEY` required — verification is read-only.

### Set the minimum broker version

**Option A: Edit TOML first (recommended for planned rollouts)**

1. Edit `contracts/deployment-version-registry.toml`:
   ```toml
   minimum-broker-version = "1.3.0"
   ```
2. Run the script:
   ```sh
   CHAIN_KEY=ethereum-sepolia \
   DEPLOYER_PRIVATE_KEY=0x... \
     ./contracts/scripts/manage-version-registry SetMinimumBrokerVersion --broadcast
   ```

**Option B: Override via env vars (for quick changes)**

```sh
CHAIN_KEY=ethereum-sepolia \
DEPLOYER_PRIVATE_KEY=0x... \
MAJOR=1 MINOR=3 PATCH=0 \
  ./contracts/scripts/manage-version-registry SetMinimumBrokerVersion --broadcast
```

Both options write the version back to the TOML after execution.

**Via Gnosis Safe:**

```sh
CHAIN_KEY=ethereum-sepolia \
GNOSIS_EXECUTE=true \
MAJOR=1 MINOR=3 PATCH=0 \
  ./contracts/scripts/manage-version-registry SetMinimumBrokerVersion
```

Copy the printed calldata into the Safe web app. No `--broadcast` or `DEPLOYER_PRIVATE_KEY` needed — it only prints the calldata.

### Set or clear the governance notice

**Set a notice:**

```sh
# From TOML (edit deployment-version-registry.toml first)
CHAIN_KEY=ethereum-sepolia \
DEPLOYER_PRIVATE_KEY=0x... \
  ./contracts/scripts/manage-version-registry SetNotice --broadcast

# Or via env override
CHAIN_KEY=ethereum-sepolia \
DEPLOYER_PRIVATE_KEY=0x... \
NOTICE="Upgrade to v1.4.0 by March 30. See https://docs.boundless.network/" \
  ./contracts/scripts/manage-version-registry SetNotice --broadcast
```

**Clear the notice:**

```sh
CHAIN_KEY=ethereum-sepolia \
DEPLOYER_PRIVATE_KEY=0x... \
NOTICE="" \
  ./contracts/scripts/manage-version-registry SetNotice --broadcast
```

Notices appear in broker logs as `WARN` with the text `VersionRegistry notice: <message>`.

### Upgrade the contract

```sh
CHAIN_KEY=ethereum-sepolia \
DEPLOYER_PRIVATE_KEY=0x... \
SKIP_SAFETY_CHECKS=true \
  ./contracts/scripts/manage-version-registry UpgradeVersionRegistry --broadcast
```

**Via Gnosis Safe:**

```sh
CHAIN_KEY=ethereum-sepolia \
GNOSIS_EXECUTE=true \
SKIP_SAFETY_CHECKS=true \
  ./contracts/scripts/manage-version-registry UpgradeVersionRegistry --broadcast
```

This deploys the new implementation and prints `upgradeToAndCall` calldata for the Safe.

> [!WARNING]
> `SKIP_SAFETY_CHECKS=true` bypasses OpenZeppelin's upgrade safety validation. Without it, you need a reference build (see the PoVW runbook for the reference build workflow).

### Transfer ownership

Ownership uses Ownable2Step — a two-transaction process to prevent accidental transfers.

**Step 1: Initiate transfer (current owner)**

```sh
CHAIN_KEY=ethereum-sepolia \
DEPLOYER_PRIVATE_KEY=0x... \
NEW_OWNER=0x... \
  ./contracts/scripts/manage-version-registry TransferVersionRegistryOwnership --broadcast
```

**Step 2: Accept transfer (new owner)**

```sh
CHAIN_KEY=ethereum-sepolia \
DEPLOYER_PRIVATE_KEY=0x... \
  ./contracts/scripts/manage-version-registry AcceptVersionRegistryOwnership --broadcast
```

Both steps support `GNOSIS_EXECUTE=true` for Safe-based workflows.

## Reading On-Chain State

Query the current state without sending transactions:

```sh
# Current minimum version (human-readable)
cast call --rpc-url $RPC_URL $VERSION_REGISTRY 'minimumBrokerVersionString()(string)'

# Current minimum version (semver components)
cast call --rpc-url $RPC_URL $VERSION_REGISTRY 'minimumBrokerVersionSemver()(uint16,uint16,uint16)'

# Current notice
cast call --rpc-url $RPC_URL $VERSION_REGISTRY 'notice()(string)'

# Both at once
cast call --rpc-url $RPC_URL $VERSION_REGISTRY 'getVersionInfo()(uint64,string)'

# Current owner
cast call --rpc-url $RPC_URL $VERSION_REGISTRY 'owner()(address)'

# Pending owner (for in-progress transfers)
cast call --rpc-url $RPC_URL $VERSION_REGISTRY 'pendingOwner()(address)'

# Contract version (upgrade tracking)
cast call --rpc-url $RPC_URL $VERSION_REGISTRY 'VERSION()(uint64)'
```

Where `$VERSION_REGISTRY` is the proxy address from `deployment-version-registry.toml`.

## Playbooks

### Playbook: Force all brokers to upgrade

**Scenario:** A critical bug or security issue requires all brokers to be on v1.5.0+.

1. Communicate the deadline via notice (give brokers time to upgrade):
   ```sh
   NOTICE="Critical update: upgrade to v1.5.0 by 2026-04-01. See https://docs.boundless.network/upgrade" \
   CHAIN_KEY=ethereum-sepolia DEPLOYER_PRIVATE_KEY=0x... \
     ./contracts/scripts/manage-version-registry SetNotice --broadcast
   ```

2. After the deadline, enforce the minimum:
   ```sh
   MAJOR=1 MINOR=5 PATCH=0 \
   CHAIN_KEY=ethereum-sepolia DEPLOYER_PRIVATE_KEY=0x... \
     ./contracts/scripts/manage-version-registry SetMinimumBrokerVersion --broadcast
   ```

3. Clear the notice once enforcement is active:
   ```sh
   NOTICE="" \
   CHAIN_KEY=ethereum-sepolia DEPLOYER_PRIVATE_KEY=0x... \
     ./contracts/scripts/manage-version-registry SetNotice --broadcast
   ```

4. Repeat for each chain (base-mainnet, etc.).

### Playbook: Rollback the minimum version

**Scenario:** The new broker version has a regression and you need to let brokers run the older version.

Downgrades are allowed — just set the minimum to the older version:

```sh
MAJOR=1 MINOR=4 PATCH=0 \
CHAIN_KEY=ethereum-sepolia DEPLOYER_PRIVATE_KEY=0x... \
  ./contracts/scripts/manage-version-registry SetMinimumBrokerVersion --broadcast
```

Brokers that were shut down by the version check will need to be manually restarted.

### Playbook: Emergency — disable version enforcement

Set the minimum to `0.0.0` to effectively disable the check:

```sh
MAJOR=0 MINOR=0 PATCH=0 \
CHAIN_KEY=ethereum-sepolia DEPLOYER_PRIVATE_KEY=0x... \
  ./contracts/scripts/manage-version-registry SetMinimumBrokerVersion --broadcast
```

### Playbook: Multi-chain rollout

When updating the minimum version across multiple chains:

```sh
for chain in ethereum-sepolia base-sepolia ethereum-mainnet base-mainnet; do
  echo "=== Setting minimum version on $chain ==="
  CHAIN_KEY=$chain DEPLOYER_PRIVATE_KEY=0x... MAJOR=1 MINOR=5 PATCH=0 \
    ./contracts/scripts/manage-version-registry SetMinimumBrokerVersion --broadcast
done
```

## Troubleshooting

### Broker error: `[B-VER-001] Broker version X.Y.Z is below the on-chain minimum version A.B.C`

The broker is below the required minimum. Upgrade the broker binary to at least `A.B.C`. If the minimum was set too aggressively, roll it back (see playbook above).

### Broker log: `Failed to read VersionRegistry. Continuing.`

The broker couldn't reach the VersionRegistry contract. Possible causes:

- Contract not deployed on this chain yet
- RPC endpoint is down or rate-limited
- Chain ID not in the broker's hardcoded registry table

This is a **warning, not a failure** — the broker continues operating.

### Broker log: `VersionRegistry address not configured for chain. Skipping version check.`

The chain ID is not in the hardcoded `VERSION_REGISTRIES` table in `version_check.rs`. After deploying to a new chain, add the address to the table and release a new broker version.

### Script error: `required address value not set`

The `version-registry` or `admin` address is missing in `deployment-version-registry.toml` for the target `CHAIN_KEY`. Check the TOML file.

### `GNOSIS_EXECUTE` mode doesn't print calldata

Ensure you're not setting `DEPLOYER_PRIVATE_KEY` when using `GNOSIS_EXECUTE=true` — the script runs in simulation mode and only needs RPC access to read state.

## File Reference

| File                                                   | Purpose                                                                 |
| ------------------------------------------------------ | ----------------------------------------------------------------------- |
| `contracts/src/VersionRegistry.sol`                    | Contract implementation                                                 |
| `contracts/src/IVersionRegistry.sol`                   | Interface                                                               |
| `contracts/test/VersionRegistry.t.sol`                 | Forge tests                                                             |
| `contracts/scripts/Manage.VersionRegistry.s.sol`       | Forge scripts (deploy, upgrade, set version/notice, transfer ownership) |
| `contracts/scripts/manage-version-registry`            | Bash wrapper for the Forge scripts                                      |
| `contracts/scripts/verify-version-registry.sh`         | Etherscan verification script                                           |
| `contracts/deployment-version-registry.toml`           | Per-chain deployment config and version policy                          |
| `contracts/update_deployment_version_registry_toml.py` | Python helper for TOML updates (called via FFI)                         |
| `crates/broker/src/version_check.rs`                   | Broker-side version enforcement (Rust)                                  |
