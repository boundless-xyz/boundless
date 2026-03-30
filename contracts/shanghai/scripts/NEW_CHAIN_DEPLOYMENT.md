# Deploying Boundless to a New Chain

End-to-end guide for deploying the full Boundless stack (verifiers + market) to a new EVM chain. This guide assumes a Shanghai-compatible L2 (no PUSH0); for Cancun chains, use the default profile and scripts under `contracts/scripts/` instead of `contracts/shanghai/scripts/`.

> [!NOTE]
> All commands assume your working directory is the repo root.

## Prerequisites

- [Foundry](https://book.getfoundry.sh/getting-started/installation)
- [yq v4+](https://github.com/mikefarah/yq)
- `python3` with `tomlkit` (`pip install tomlkit`)
- A funded deployer wallet on the target chain
- A funded wallet on Ethereum mainnet (for bridging ZKC, if applicable)

## Overview

| Step | What                                              | Script / Tool                                                                                    |
| ---- | ------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| 1    | Add chain config                                  | `deployment_verifier.toml`, `deployment_secrets.toml`, `deployment.toml`                         |
| 2    | Deploy RISC Zero timelocked router                | `manage-verifier DeployRisc0TimelockRouter`                                                      |
| 3    | Deploy RISC Zero verifiers (Groth16, SetVerifier) | `manage-verifier DeployEstop*Verifier`                                                           |
| 4    | Route RISC Zero verifiers                         | `manage-verifier ScheduleAddVerifierToRisc0Router` / `FinishAddVerifierToRisc0Router`            |
| 5    | Deploy Boundless layered router                   | `manage-verifier DeployTimelockRouter` (with `parent-router` = RISC Zero router)                 |
| 6    | Deploy Blake3Groth16 verifier and route it        | `manage-verifier DeployEstopBlake3Groth16Verifier` + `ScheduleAddVerifier` / `FinishAddVerifier` |
| 7    | Bridge ZKC collateral token                       | Chain's native bridge (e.g. `scripts/bridge-zkc-to-taiko.sh`)                                    |
| 8    | Deploy BoundlessMarket                            | `manage DeployBoundlessMarket`                                                                   |
| 9    | Verify contracts on block explorer                | `verify-*.sh` scripts                                                                            |
| 10   | Update Rust deployment constants                  | `crates/boundless-market/src/deployments.rs`, `crates/boundless-cli/src/config.rs`               |

---

## Step 1: Add Chain Configuration

### `contracts/deployment_verifier.toml`

Add a new `[chains.<chain-key>]` section:

```toml
[chains.mychain-mainnet]
name = "MyChain Mainnet"
id = 12345
etherscan-url = "https://explorer.mychain.xyz/"
foundry-profile = "shanghai"  # if Shanghai EVM; omit for Cancun

# Accounts
admin = "0x..."  # Safe or EOA that will admin the timelock + market
```

### `contracts/deployment_secrets.toml`

```toml
[chains.mychain-mainnet]
rpc-url = "https://rpc.mychain.xyz"
etherscan-api-key = "..."
```

### `contracts/deployment.toml`

Add a `[deployment.mychain-mainnet]` section. Start with placeholder addresses (zeroes); the deploy scripts will fill them in:

```toml
[deployment.mychain-mainnet]
name = "MyChain Mainnet"
id = 12345
etherscan-url = "https://explorer.mychain.xyz/"
version = "v1.0.0"

# Boundless Market admins
admin = "0x..."

# Contracts (filled by deploy scripts)
verifier = "0x0000000000000000000000000000000000000000"
application-verifier = "0x0000000000000000000000000000000000000000"
set-verifier = "0x0000000000000000000000000000000000000000"
boundless-market = "0x0000000000000000000000000000000000000000"
boundless-market-impl = "0x0000000000000000000000000000000000000000"
boundless-market-old-impl = "0x0000000000000000000000000000000000000000"
collateral-token = "0x0000000000000000000000000000000000000000"

# Guests info
deployment-commit = "bda3b118"
assessor-image-id = "0x6c5a03c0785e91bc0ad0db486004116010680a03af4e712bcca3188e56694100"
assessor-guest-url = "https://gateway.beboundless.cloud/ipfs/bafybeiauvbhinz2yqm2vbgpl2njgoyaxhuwa2vbv6gts2ajcjfkw5m4ejq"
deprecated-assessor-duration = 0
```

> [!TIP]
> Copy `assessor-image-id` and `assessor-guest-url` from the latest production deployment (e.g. `base-mainnet`).

---

## Step 2: Deploy RISC Zero Timelocked Router

The verifier infrastructure has two tiers: a **RISC Zero router** (manages core RISC Zero verifiers) and a **Boundless layered router** (wraps the RISC Zero router and adds Boundless-specific verifiers like Blake3Groth16). The RISC Zero router must be deployed first.

```bash
export CHAIN_KEY=mychain-mainnet
export DEPLOYER_PRIVATE_KEY=0x...

# Dry run first
contracts/shanghai/scripts/manage-verifier DeployRisc0TimelockRouter

# If looks good, broadcast
contracts/shanghai/scripts/manage-verifier DeployRisc0TimelockRouter --broadcast
```

This deploys a `TimelockController` and `RiscZeroVerifierRouter`. Update `deployment_verifier.toml` with the deployed addresses:

```toml
risc0-router = "0x..."
risc0-timelock-controller = "0x..."
risc0-timelock-delay = 0
```

> [!IMPORTANT]
> Set `risc0-timelock-delay` appropriately: `0` for testnet, `259200` (3 days) for mainnet.

---

## Step 3: Deploy RISC Zero Verifiers

Deploy the core RISC Zero verifiers: `RiscZeroGroth16Verifier` and `RiscZeroSetVerifier`.

```bash
# RiscZero Groth16 verifier
contracts/shanghai/scripts/manage-verifier DeployEstopGroth16Verifier --broadcast

# RiscZero Set verifier
contracts/shanghai/scripts/manage-verifier DeployEstopSetVerifier --broadcast
```

After each deployment, add the verifier entry to `deployment_verifier.toml` under `risc0-verifiers`:

```toml
[[chains.mychain-mainnet.risc0-verifiers]]
name = "RiscZeroGroth16Verifier"
version = "3.0.0"
selector = "0x..."
verifier = "0x..."
estop = "0x..."

[[chains.mychain-mainnet.risc0-verifiers]]
name = "RiscZeroSetVerifier"
version = "0.9.0"
selector = "0x..."
verifier = "0x..."
estop = "0x..."
```

---

## Step 4: Route RISC Zero Verifiers

Route each RISC Zero verifier through the RISC Zero router:

```bash
# For each verifier, schedule and finish the add operation
VERIFIER_SELECTOR="0x..." contracts/shanghai/scripts/manage-verifier ScheduleAddVerifierToRisc0Router --broadcast

# After timelock delay passes, finish
VERIFIER_SELECTOR="0x..." contracts/shanghai/scripts/manage-verifier FinishAddVerifierToRisc0Router --broadcast
```

Repeat for both `RiscZeroGroth16Verifier` and `RiscZeroSetVerifier`.

> [!TIP]
> If timelock delay is 0 (testnet), you can schedule and finish immediately.

---

## Step 5: Deploy Boundless Layered Router

Now deploy the Boundless layered router, which wraps the RISC Zero router as its parent:

```bash
contracts/shanghai/scripts/manage-verifier DeployTimelockRouter --broadcast
```

Ensure `parent-router` in `deployment_verifier.toml` points to the RISC Zero router address before deploying. Update the TOML with the deployed addresses:

```toml
timelock-controller = "0x..."
timelock-delay = 0
router = "0x..."
parent-router = "<risc0-router-address>"
```

---

## Step 6: Deploy and Route Blake3Groth16 Verifier

Deploy the Blake3Groth16 verifier and route it through the Boundless layered router:

```bash
# Deploy
contracts/shanghai/scripts/manage-verifier DeployEstopBlake3Groth16Verifier --broadcast
```

Add to `deployment_verifier.toml` under `verifiers` (not `risc0-verifiers`):

```toml
[[chains.mychain-mainnet.verifiers]]
name = "Blake3Groth16Verifier"
version = "0.0.1"
selector = "0x..."
verifier = "0x..."
estop = "0x..."
```

Route it through the layered router:

```bash
VERIFIER_SELECTOR="0x..." contracts/shanghai/scripts/manage-verifier ScheduleAddVerifier --broadcast
VERIFIER_SELECTOR="0x..." contracts/shanghai/scripts/manage-verifier FinishAddVerifier --broadcast
```

Verify the full verifier stack:

```bash
FOUNDRY_PROFILE=deployment-test forge test --match-contract=VerifierDeploymentTest -vv --fork-url=$RPC_URL
```

Update `deployment.toml` with the final router and set-verifier addresses:

```toml
verifier = "<layered-router-address>"
application-verifier = "<layered-router-address>"  # often the same as verifier
set-verifier = "<set-verifier-address>"
```

---

## Step 7: Bridge ZKC Collateral Token

If the target chain is an L2, you need to bridge ZKC from Ethereum mainnet. The bridged token becomes the market's collateral token.

### Using the bridge script (Taiko example)

```bash
PRIVATE_KEY=0x... ./scripts/bridge-zkc-to-taiko.sh
```

### General approach for other chains

1. **Approve** the chain's L1 bridge/vault contract to spend ZKC (`0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555`)
2. **Bridge** a small amount (1 ZKC) via the chain's native bridge — this triggers auto-deployment of a bridged ERC20 on L2
3. **Claim** the bridge message on L2 (via bridge UI or contract call)
4. **Record** the bridged token address and update `collateral-token` in `deployment.toml`

> [!NOTE]
> Most L2 bridged tokens do NOT support ERC20Permit. The `collateral_token_supports_permit()` function in `deployments.rs` already returns `false` for all chains except Ethereum mainnet, Sepolia, and Anvil — so new L2s are handled correctly by default.

---

## Step 8: Deploy BoundlessMarket

Ensure `deployment.toml` has the correct `verifier`, `application-verifier`, `set-verifier`, `collateral-token`, `assessor-image-id`, and `assessor-guest-url` for your chain before deploying.

```bash
export CHAIN_KEY=mychain-mainnet
export DEPLOYER_PRIVATE_KEY=0x...

# Dry run
contracts/shanghai/scripts/manage DeployBoundlessMarket

# Broadcast
contracts/shanghai/scripts/manage DeployBoundlessMarket --broadcast
```

The script auto-updates `deployment.toml` with `boundless-market`, `boundless-market-impl`, and `boundless-market-deployment-commit`.

---

## Step 9: Verify Contracts

### Implementation

```bash
export CHAIN_KEY=mychain-mainnet
export RPC_URL=https://rpc.mychain.xyz
contracts/shanghai/scripts/verify-boundless-market.sh
```

### Proxy (ERC1967Proxy)

```bash
export FOUNDRY_PROFILE=shanghai

IMPL="<boundless-market-impl>"
ADMIN="<admin-address>"
GUEST_URL="<assessor-guest-url>"

INIT_CALLDATA=$(cast calldata "initialize(address,string)" "$ADMIN" "$GUEST_URL")
CONSTRUCTOR_ARGS=$(cast abi-encode "constructor(address,bytes)" "$IMPL" "$INIT_CALLDATA")

forge verify-contract --watch \
    --chain-id=<chain-id> \
    --constructor-args="$CONSTRUCTOR_ARGS" \
    --verifier-url "https://api.etherscan.io/v2/api?chainid=<chain-id>" \
    --etherscan-api-key="$ETHERSCAN_API_KEY" \
    <boundless-market-proxy-address> \
    lib/openzeppelin-contracts/contracts/proxy/ERC1967/ERC1967Proxy.sol:ERC1967Proxy
```

> [!TIP]
> For blockscout-based explorers, use `--verifier blockscout --verifier-url https://api.explorer.xyz/api/`. For Etherscan V2-compatible explorers, use `--verifier-url "https://api.etherscan.io/v2/api?chainid=<chain-id>"`.

### Verifier contracts

```bash
contracts/shanghai/scripts/verify-router.sh
contracts/shanghai/scripts/verify-blake3-groth16-verifier.sh
contracts/shanghai/scripts/verify-risc0-groth16-verifier.sh
contracts/shanghai/scripts/verify-risc0-set-verifier.sh
```

---

## Step 10: Update Rust Code

### `crates/boundless-market/src/deployments.rs`

Add a deployment constant and wire it into `from_chain()`:

```rust
pub const MY_CHAIN: Deployment = Deployment {
    market_chain_id: Some(NamedChain::MyChain as u64),
    boundless_market_address: address!("0x..."),
    verifier_router_address: Some(address!("0x...")),
    set_verifier_address: address!("0x..."),
    collateral_token_address: Some(address!("0x...")),
    order_stream_url: None,
    indexer_url: None,
    deployment_block: Some(12345),
};
```

Add to `from_chain()`:

```rust
NamedChain::MyChain => Some(MY_CHAIN),
```

> [!NOTE]
> Check that `NamedChain::MyChain` exists in `alloy-chains`. If not, users must pass deployment info explicitly via CLI flags or config.

### `crates/boundless-cli/src/config.rs`

Add `"mychain-mainnet"` to both match blocks (requestor and prover):

```rust
"mychain-mainnet" => Some(boundless_market::deployments::MY_CHAIN),
```

### `crates/boundless-cli/src/display.rs`

Add to `network_name_from_chain_id()`:

```rust
Some(12345) => "MyChain Mainnet",
```

### Rebuild and verify

```bash
cargo check -p boundless-market -p boundless-cli
RUSTFLAGS=-Dwarnings RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNELS=1 cargo clippy -p boundless-market -p boundless-cli --all-targets
```

---

## Post-Deployment Checklist

- [ ] All verifiers deployed and routed
- [ ] Deployment tests pass on fork (`FOUNDRY_PROFILE=deployment-test forge test ...`)
- [ ] ZKC bridged and collateral token address recorded
- [ ] BoundlessMarket deployed, initialized, and admin set
- [ ] All contracts verified on block explorer
- [ ] `deployment.toml` and `deployment_verifier.toml` updated with all addresses
- [ ] Rust deployment constants added and compile clean
- [ ] CLI config supports the new chain name
- [ ] `~/.boundless/config.toml` can be pointed at the new chain for testing
