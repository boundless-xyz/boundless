# Indexer

The `boundless-indexer` crate contains logic for two separate blockchain indexers that populate a shared PostgreSQL database:

1. **Market Indexer** - Indexes Boundless Market protocol events (proof requests, fulfillments, etc.) on any chain with Boundless Market deployed
2. **Rewards Indexer** - Indexes ZKC token staking, PoVW rewards, and delegation data on Ethereum mainnet

Both indexers share the same database schema and can run independently or together, depending on your needs.

## Market Indexer

The market indexer monitors the Boundless Market protocol contract and indexes all market-related events and data.

### Market Indexer Chain Support

The market indexer can run on any EVM-compatible chain. It's typically used on Base mainnet, but can be configured for any chain where the Boundless Market contract is deployed.

### Market Indexer Data

The market indexer tracks:

- **Proof Request Submissions**: Both onchain submissions (via contract events) and offchain submissions (via the order stream API)
- **Request Locking Events**: When provers lock requests for fulfillment
- **Proof Deliveries and Fulfillments**: When proofs are delivered and requests are fulfilled
- **Request Statuses**: Tracks request lifecycle states (submitted, locked, fulfilled, expired, etc.)
- **Cycle Counts**: Computes cycle counts for fulfilled requests based on requestor address
- **Market Aggregates**: Generates hourly, daily, weekly, and monthly market summaries including:
  - Total requests and fulfillments
  - Total fees and rewards
  - Requestor and prover statistics
  - Cycle counts and computational work

### Market Indexer Binary

The market indexer binary is located at `src/bin/market-indexer.rs`.

### Backfill Tool

The crate includes a backfill binary (`market-indexer-backfill`) that can recompute request statuses and market aggregates from existing event data. This is useful for fixing inconsistencies or rebuilding computed tables after schema changes.

## Rewards Indexer

The rewards indexer monitors ZKC token staking, PoVW (Proof of Verifiable Work) rewards, and delegation contracts on Ethereum mainnet.

### Rewards Indexer Chain Support

The rewards indexer is designed for Ethereum mainnet (chain ID 1) or Sepolia testnet (chain ID 11155111). It automatically determines contract addresses based on the chain ID.

### Rewards Indexer Data

The rewards indexer tracks:

- **ZKC Token Staking**: Staking positions, staking power, and staking emissions by epoch
- **PoVW Rewards**: Proof of Verifiable Work rewards distributed by epoch, including aggregate statistics
- **Delegation Powers**: Voting delegation powers and weights by epoch
- **Epoch Summaries**: Comprehensive epoch-based summaries and aggregates for staking, PoVW, and delegations

### Rewards Indexer Binary

The rewards indexer binary is located at `src/bin/rewards-indexer.rs`.

## Testing

The indexer crate includes both library tests and integration tests. Integration tests are marked with `#[ignore]` because they require RPC endpoints to fetch real blockchain data.

### Test Structure

- **Library Tests** (`--lib`): Unit tests for individual modules and functions
- **Integration Tests**: End-to-end tests that require RPC URLs and a database
  - Market integration tests: `tests/market/`
  - Rewards integration tests: `tests/rewards/`

### Required Environment Variables

- `DATABASE_URL`: PostgreSQL connection string (automatically set by justfile commands - defaults to `postgres://postgres:password@localhost:5433/postgres`, but can be overridden)
- `BASE_MAINNET_RPC_URL`: Required for market indexer tests - must be an archive node endpoint with support for querying events
- `ETH_MAINNET_RPC_URL`: Required for rewards indexer tests - must be an archive node endpoint with support for querying events
- `RISC0_DEV_MODE=1`: Automatically set by justfile commands - enables faster dev mode proofs (insecure, for testing only)

### Justfile Commands

The repository includes convenient justfile commands for running tests:

- **`just test-indexer`**: Run all indexer tests (library tests + all integration tests)
  - Requires: `BASE_MAINNET_RPC_URL` and `ETH_MAINNET_RPC_URL`
  - Automatically sets up and cleans up test database

- **`just test-indexer-market`**: Run market indexer tests only (library tests + market integration tests)
  - Requires: `BASE_MAINNET_RPC_URL`
  - Automatically sets up and cleans up test database

- **`just test-indexer-rewards`**: Run rewards indexer tests only (library tests + rewards integration tests)
  - Requires: `ETH_MAINNET_RPC_URL`
  - Automatically sets up and cleans up test database

- **`just test-db setup`**: Sets up a test PostgreSQL database in Docker (creates `postgres-test` container on port 5433)

- **`just test-db clean`**: Cleans up the test database (stops and removes `postgres-test` container)

### Example Test Workflow

```bash
# Set required environment variables
export BASE_MAINNET_RPC_URL="https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY"
export ETH_MAINNET_RPC_URL="https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY"

# Run all indexer tests
just test-indexer

# Or run tests for a specific indexer
just test-indexer-market
just test-indexer-rewards
```

### End to End testing

For E2E manual testing, we recommend using the tools in the `indexer-api` crate, which will index the chain and spin up an API for exploring.

**For end-to-end setup instructions**, including how to run the indexer and API together locally, see the [Indexer API README](../lambdas/indexer-api/README.md). The API can be run locally using the `manage_local` tool, which provides a convenient way to spin up both the indexer and API server.

## Database Schema

Both indexers share the same database schema. Database migrations are located in the `migrations/` directory and are numbered sequentially. The schema includes:

- Market tables: `proof_requests`, `request_status`, `fulfillments`, `transactions`, event tables, and market summary tables
- Rewards tables: `staking_positions`, `staking_rewards`, `povw_rewards_by_epoch`, `povw_rewards_aggregate`, `delegation_powers`, and summary statistics
-
