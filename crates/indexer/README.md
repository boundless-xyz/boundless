# Indexer

The `boundless-indexer` crate contains logic for two separate blockchain indexers that populate a shared PostgreSQL database:

1. **Market Indexer** - Indexes Boundless Market protocol events (proof requests, fulfillments, etc.) on any chain with Boundless Market deployed
2. **Rewards Indexer** - Indexes ZKC token staking, PoVW rewards, and delegation data on Ethereum mainnet

Both indexers share the same database schema and can run independently or together, depending on your needs.

## Market Indexer

The market indexer monitors the Boundless Market protocol contract and indexes all market-related events and data.

### Market Indexer Chain Support

The market indexer can run on any EVM-compatible chain where the Boundless Market contract is deployed.

### Market Indexer Data

The market indexer tracks:

- **Proof Request Submissions**: Both onchain submissions (via contract events) and offchain submissions (via the order stream API)
- **Request Locking Events**: When provers lock requests for fulfillment
- **Proof Deliveries and Fulfillments**: When proofs are delivered and requests are fulfilled
- **Request Statuses**: Tracks request lifecycle states (submitted, locked, fulfilled, expired, etc.)
- **Cycle Counts**: Computes cycle counts for fulfilled requests based on requestor address
- **Market Aggregates**: Precomputed aggregations generated for the market as a whole, as well as for each individual requestor and prover. Includes hourly, daily, weekly, monthly, and all-time summaries with:
  - Total requests and fulfillments
  - Total fees and rewards
  - Per-requestor and per-prover statistics
  - Cycle counts and computational work

### Market Indexer Binary

The market indexer binary is located at `src/bin/market-indexer.rs`.

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

## Architecture

This section covers the design and architecture of the Market Indexer in detail.

### High-Level Design Philosophy

The indexer follows these design principles:

- **Denormalized table structure**: The `request_status` table is a flattened view that consolidates data from `proof_requests`, all event tables, `proofs`, and `cycle_counts`. This avoids expensive joins at query time. This table is the primary source for our frontend applications when displaying request information.

- **Precomputed aggregations**: All summary statistics (market-wide, per-requestor, and per-prover) are precomputed and stored in dedicated tables rather than computed on-demand. This enables fast dashboard and analytics queries.

- **Event sourcing pattern**: Raw blockchain events are stored in separate `*_events` tables. The `request_status` table is a materialized view derived from these events, allowing state to be rebuilt if needed.

### Task Architecture

The Market Indexer runs three concurrent tasks orchestrated by `IndexerService::run()` in [`src/market/service/core.rs`](src/market/service/core.rs):

```
                     +------------------+
                     |  IndexerService  |
                     +--------+---------+
                              |
        +---------------------+---------------------+
        |                     |                     |
        v                     v                     v
+----------------+   +------------------+   +------------------+
|   Core Loop    |   |  Aggregation     |   |  Execution       |
|   (blocking)   |   |  Supervisor      |   |  Supervisor      |
+----------------+   +------------------+   +------------------+
        |                     |                     |
  process_blocks()     process_aggregations()   execute_requests()
```

1. **Core Loop** (main thread): Reads blockchain data, stores all raw event data to event tables, and tracks which requests have been "touched" in the current block range. Touched requests (those affected by new events, expirations, or cycle count updates) are then recomputed and their statuses updated in the `request_status` table. Writes progress to `last_block` table (id=0).

2. **Aggregation Supervisor** (spawned task): Runs on a configurable interval, lagging behind the core loop. Computes and populates hourly, daily, weekly, monthly, and all-time aggregations for the market as a whole, each requestor, and each prover. Reads core loop progress from `last_block` (id=0) and writes its own progress to `last_aggregation_block` (id=1).

3. **Execution Supervisor** (spawned task, optional): Tracks which requests need cycle counts computed. For requests that can't be precomputed, it calls out to an external Bento API to submit execution requests, then polls for responses until completion. Runs on a configurable interval.

The supervisor pattern (see `run_aggregation_task_supervisor()` and `run_execution_task_supervisor()`) automatically restarts tasks on failure with a 5-second delay.

#### Core Loop

The core loop processes blocks in batches via `process_blocks()` in [`src/market/service/core.rs`](src/market/service/core.rs).

**Data Flow**:

1. **Fetch logs** from blockchain by event signatures (see [`fetch_logs()`](src/market/service/chain_data.rs#L36))
2. **Fetch transaction metadata** and cache it
3. **Process events** in parallel (see [`src/market/service/log_processors.rs`](src/market/service/log_processors.rs)):
   - `RequestSubmitted` -> `request_submitted_events` + `proof_requests`
   - `RequestLocked` -> `request_locked_events`
   - `ProofDelivered` -> `proof_delivered_events` + `proofs`
   - `RequestFulfilled` -> `request_fulfilled_events`
   - `ProverSlashed` -> `prover_slashed_events`
   - `CallbackFailed` -> `callback_failed_events`
   - Deposit/Withdrawal events -> respective tables
4. **Process offchain requests** from Order Stream API (if configured)
5. **Track touched requests**: Collect all request digests affected by events, expirations, or cycle count updates
6. **Compute statuses** (see [`src/market/service/status.rs`](src/market/service/status.rs)): Query comprehensive data, compute status fields, upsert to `request_status`
7. **Request cycle counts** (see [`src/market/service/cycle_counts.rs`](src/market/service/cycle_counts.rs)): Trigger cycle count computation for locked/fulfilled requests

**Tables Used**:

| Table                      | Purpose                                                                                 |
| -------------------------- | --------------------------------------------------------------------------------------- |
| `transactions`             | Transaction metadata (hash, block, timestamp, from/to)                                  |
| `proof_requests`           | Raw request data from onchain/offchain sources                                          |
| `request_submitted_events` | RequestSubmitted event data                                                             |
| `request_locked_events`    | RequestLocked event data                                                                |
| `proof_delivered_events`   | ProofDelivered event data                                                               |
| `request_fulfilled_events` | RequestFulfilled event data                                                             |
| `prover_slashed_events`    | ProverSlashed event data                                                                |
| `callback_failed_events`   | CallbackFailed event data                                                               |
| `proofs`                   | Proof seal and journal data                                                             |
| `cycle_counts`             | Execution cycle tracking                                                                |
| `request_status`           | Denormalized view with computed status and metrics (primary table for frontend queries) |
| `last_block`               | Tracks last processed block (id=0)                                                      |

#### Aggregation Loop

The aggregation loop runs concurrently via `process_aggregations()` in [`src/market/service/core.rs`](src/market/service/core.rs), with aggregation logic in [`src/market/service/aggregation/`](src/market/service/aggregation/).

**Design**:

- **Lags behind core loop**: Reads `last_block` (id=0), writes to `last_aggregation_block` (id=1)
- **Runs on interval**: Configurable via `aggregation_interval` (default: 10s)
- **Three aggregation types**: Market, Requestor, Prover
- **Five time periods each**: Hourly, Daily, Weekly, Monthly, All-Time
- **15 total summary tables**: 5 periods x 3 types

**Recompute Windows** (defined in [`src/market/service/aggregation/mod.rs`](src/market/service/aggregation/mod.rs)):

Recent periods are recomputed to capture delayed data:

- Hourly: last 6 hours
- Daily: last 2 days
- Weekly: last 2 weeks
- Monthly: last 2 months

**Aggregation Modules**:

- [`src/market/service/aggregation/market.rs`](src/market/service/aggregation/market.rs): Market-wide aggregations
- [`src/market/service/aggregation/requestors.rs`](src/market/service/aggregation/requestors.rs): Per-requestor aggregations
- [`src/market/service/aggregation/provers.rs`](src/market/service/aggregation/provers.rs): Per-prover aggregations

**Tables Used**:

| Table                        | Purpose                              |
| ---------------------------- | ------------------------------------ |
| `request_status`             | Source data for aggregation queries  |
| `hourly_market_summary`      | Market stats per hour                |
| `daily_market_summary`       | Market stats per day                 |
| `weekly_market_summary`      | Market stats per week                |
| `monthly_market_summary`     | Market stats per month               |
| `all_time_market_summary`    | Cumulative market stats              |
| `hourly_requestor_summary`   | Per-requestor stats per hour         |
| `daily_requestor_summary`    | Per-requestor stats per day          |
| `weekly_requestor_summary`   | Per-requestor stats per week         |
| `monthly_requestor_summary`  | Per-requestor stats per month        |
| `all_time_requestor_summary` | Cumulative per-requestor stats       |
| `hourly_prover_summary`      | Per-prover stats per hour            |
| `daily_prover_summary`       | Per-prover stats per day             |
| `weekly_prover_summary`      | Per-prover stats per week            |
| `monthly_prover_summary`     | Per-prover stats per month           |
| `all_time_prover_summary`    | Cumulative per-prover stats          |
| `last_block`                 | Tracks last aggregation block (id=1) |

#### Cycle Counts Executor

The cycle counts executor computes execution cycles via `execute_requests()` in [`src/market/service/execution.rs`](src/market/service/execution.rs), with cycle count logic in [`src/market/service/cycle_counts.rs`](src/market/service/cycle_counts.rs).

**Precomputation Optimization**:

Known requestors use precomputed or extracted values to avoid Bento execution:

- **Signal Requestor**: Generates random cycles (50B-54B range)
- **Known Requestors**: Extracts cycles from inline input data (GuestEnv-encoded)
- **Unknown Requestors**: Marked as `PENDING` for Bento execution

**Bento Cluster Integration**:

For unknown requestors, the executor:

1. Downloads input data (handles `Url` and `Inline` types)
2. Uploads input to Bento
3. Checks if image exists, uploads if missing
4. Creates execution session with price limit
5. Polls session status until completion
6. Extracts `program_cycles` and `total_cycles` from results

**State Machine**:

```
PENDING -> EXECUTING -> COMPLETED
                    \-> FAILED
```

**Concurrency Control**:

- `max_concurrent_executing`: Limits concurrent Bento executions (default: 20)
- `max_status_queries`: Max status checks per iteration (default: 30)

**Tables Used**:

| Table            | Purpose                               |
| ---------------- | ------------------------------------- |
| `cycle_counts`   | Tracks cycle count status and results |
| `proof_requests` | Source for input data and image info  |

### RPC Configuration

The indexer supports multiple RPC providers and configurable lag behind chain head.

**Multiple RPC Endpoints** (configured in [`src/market/service/mod.rs`](src/market/service/mod.rs)):

- `provider`: Main RPC for general operations such as `get_block_number` (checking chain head), `get_transaction_by_hash` (fetching transaction metadata), and contract calls to the Boundless Market contract.

- `logs_provider`: Dedicated RPC for `get_logs` calls. Can use a different URL optimized for log queries, as some RPC providers have specialized endpoints or rate limits for historical log queries.

- `any_network_provider`: Used for `get_block_receipts` calls with network flexibility. This is needed because some chains (e.g., Optimism) may require querying L1 data for deposit transactions, and this provider is configured to support any network rather than being locked to a specific chain ID.

**Block Delay** (see `current_block()` in [`src/market/service/core.rs`](src/market/service/core.rs)):

The indexer can lag behind chain head by a configurable number of blocks (`--block-delay`) to reduce reorg risk:

```rust
let chain_head = provider.get_block_number().await?;
Ok(chain_head.saturating_sub(self.config.block_delay))
```

**Retry Logic**:

- Recoverable errors (RPC errors, event query errors) trigger exponential backoff: `2^(attempt-1)` seconds, capped at 120 seconds
- Max retries configurable via `--retries` (default: 10)
- Rate limiting protection: 1-second sleep between block receipt queries

**CLI Options**:

- `--rpc-url`: Main RPC endpoint (required)
- `--logs-rpc-url`: Optional separate endpoint for log queries
- `--block-delay`: Blocks to lag behind chain head (default: 0)
- `--retries`: Max retries for recoverable errors (default: 10)
- `--batch-size`: Blocks per batch (default: 9999)
- `--tx-fetch-strategy`: "block-receipts" or "tx-by-hash" (default: "tx-by-hash")

### Backfill Tool

The backfill binary (`market-indexer-backfill`) at [`src/bin/market-indexer-backfill.rs`](src/bin/market-indexer-backfill.rs) can reprocess historical data without affecting the main indexer.

**Backfill Modes** (defined in [`src/market/backfill/service.rs`](src/market/backfill/service.rs)):

1. **`chain_data`**: Re-fetches and processes blockchain events from a block range
   - Fetches logs, transaction metadata, and processes all event types
   - Processes in batches (default: 750 blocks per batch)
   - Configurable delay between batches to avoid RPC rate limits

2. **`statuses_and_aggregates`**: Recomputes request statuses then aggregations
   - Iterates through all request digests using cursor-based pagination
   - Recomputes status for each request using `compute_request_status()`
   - Then runs full aggregation recompute

3. **`aggregates`**: Recomputes only aggregations
   - Regenerates all market, requestor, and prover summary tables
   - Chunks time ranges to avoid overwhelming the database:
     - Hourly: 48 hours per chunk
     - Daily: 7 days per chunk
     - Weekly: 2 weeks per chunk
     - Monthly: 2 months per chunk

**CLI Options**:

- `--mode`: Required - "chain_data", "statuses_and_aggregates", or "aggregates"
- `--start-block` or `--lookback-blocks`: Start block specification
- `--end-block`: Optional end block (defaults to last indexed block)
- `--batch-size`: Blocks per batch for chain_data mode (default: 750)
- `--chain-data-batch-delay-ms`: Delay between batches (default: 1000ms)

**Example Usage**:

```bash
# Backfill chain data for last 10000 blocks
market-indexer-backfill --mode chain_data --lookback-blocks 10000 --rpc-url $RPC_URL --db $DATABASE_URL

# Recompute all statuses and aggregations
market-indexer-backfill --mode statuses_and_aggregates --db $DATABASE_URL

# Recompute only aggregations
market-indexer-backfill --mode aggregates --db $DATABASE_URL
```

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

Both indexers share the same database schema. Database migrations are located in the `migrations/` directory and are numbered sequentially.

### Market Tables

| Table                        | Purpose                                                                                  |
| ---------------------------- | ---------------------------------------------------------------------------------------- |
| `proof_requests`             | Raw request data from onchain/offchain sources                                           |
| `request_status`             | Denormalized view with computed status and metrics (primary table used by frontend apps) |
| `transactions`               | Transaction metadata                                                                     |
| `proofs`                     | Proof seal and journal data                                                              |
| `cycle_counts`               | Execution cycle tracking                                                                 |
| `*_events` tables            | Raw event data (7 event types)                                                           |
| `*_market_summary` tables    | Precomputed market aggregations (5 time periods)                                         |
| `*_requestor_summary` tables | Precomputed per-requestor aggregations (5 time periods)                                  |
| `*_prover_summary` tables    | Precomputed per-prover aggregations (5 time periods)                                     |
| `last_block`                 | Tracks indexer progress (id=0: core loop, id=1: aggregation)                             |

### Rewards Tables

| Table                         | Purpose                        |
| ----------------------------- | ------------------------------ |
| `staking_positions`           | Staking position data          |
| `staking_positions_aggregate` | Current staking state          |
| `staking_rewards`             | Staking reward distributions   |
| `povw_rewards_by_epoch`       | PoVW rewards per epoch         |
| `povw_rewards_aggregate`      | Aggregated PoVW rewards        |
| `delegation_powers`           | Voting delegation data         |
| `epoch_*_summary`             | Epoch-based summary statistics |
