# Boundless Architecture

## Monorepo Structure

The Boundless repository is a Cargo workspace with Solidity smart contracts:

```
boundless/
├── contracts/              # Solidity smart contracts (Foundry)
│   ├── src/               # Contract sources (BoundlessMarket, SetVerifier, etc.)
│   └── test/              # Solidity tests
├── crates/                # Rust workspace crates
│   ├── boundless-cli/     # Main CLI binary and library
│   ├── boundless-market/  # Market client library (contract bindings, request types)
│   ├── broker/            # Broker service (proof fulfillment orchestrator)
│   ├── assessor/          # Proof assessment logic
│   ├── indexer/           # Blockchain event indexer
│   ├── rewards/           # Rewards and staking logic
│   ├── povw/              # Proof of Verifiable Work
│   ├── distributor/       # Work distribution service
│   ├── order-stream/      # Order streaming service
│   ├── slasher/           # Slashing mechanism
│   ├── guest/             # zkVM guest programs
│   │   ├── assessor/     # Assessment guest program
│   │   └── util/         # Guest utilities
│   ├── bench/             # Benchmarking tools
│   ├── test-utils/        # Shared testing utilities
│   └── zkc/               # ZKC token utilities
├── examples/              # Example applications
├── bento/                 # Bento proving cluster configuration
└── xtask/                 # Cargo xtask build tasks
```

## Key Crates

### `boundless-cli` (this crate)

The main CLI binary. Organized into command modules under `src/commands/`:

- `requestor/` - Requestor commands (submit, status, balance, etc.)
- `prover/` - Prover commands (fulfill, benchmark, generate-config, etc.)
- `rewards/` - Rewards commands (stake, mine, claim, etc.)
- `setup/` - Interactive setup wizards for all modules
- `skill/` - AI tool skill file installation

### `boundless-market`

Client library for interacting with the Boundless market contracts. Contains:

- Contract bindings (generated from Solidity ABIs)
- `ProofRequest` and `Offer` types
- Market client for submitting and querying requests
- Network deployment configurations

### `broker`

The broker service orchestrates proof fulfillment. It watches for proof requests, manages a proving queue, and submits fulfilled proofs on-chain.

### `rewards` / `povw`

Proof of Verifiable Work (PoVW) mining and staking rewards system. The `povw` crate handles work claim generation and verification; `rewards` handles on-chain staking and reward distribution.

## Build Toolchain

- **Rust**: Cargo workspace, edition 2021
- **RISC Zero zkVM**: Used for zero-knowledge proof generation (`risc0-zkvm`)
- **Foundry**: Solidity smart contract development (`forge build`, `forge test`)
- **Docker Compose**: For running the broker, bento cluster, and supporting services
- **Just**: Task runner (`justfile` at repo root)

## Configuration

Configuration is stored in `~/.boundless/`:

- `config.toml` - Public config (selected networks, custom deployments)
- `secrets.toml` - Private config (RPC URLs, private keys) with 0600 permissions

Each module (requestor, prover, rewards) has its own network selection and credentials. Configuration can be overridden via environment variables or CLI flags.

## Networks

Pre-built network deployments:

- **Base Mainnet** (`base-mainnet`) - Production market
- **Base Sepolia** (`base-sepolia`) - Market testnet
- **Ethereum Mainnet** (`eth-mainnet`) - Rewards/staking (L1)
- **Ethereum Sepolia** (`eth-sepolia`) - Rewards testnet

Custom network deployments can be added via the setup wizard or `config.toml`.
