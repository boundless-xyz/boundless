# Indexer API Lambda

AWS Lambda function providing REST API access to Boundless protocol staking, delegation, and PoVW rewards data.

## Environment Variables

### API Server

- `DB_URL` (required) - PostgreSQL connection string to the indexer database
- `RUST_LOG` (optional) - Tracing log level (default: info)

### Market Indexer

The following environment variables are required when running the market indexer:

- `MARKET_RPC_URL` (required) - Ethereum RPC endpoint URL for the Boundless Market contract
- `BOUNDLESS_MARKET_ADDRESS` (required) - Boundless Market contract address

Optional environment variables:

- `LOGS_RPC_URL` - Separate RPC endpoint for `eth_getLogs` calls. If not provided, uses `MARKET_RPC_URL` for all operations
- `ORDER_STREAM_URL` - URL of the order stream API for off-chain order indexing
- `ORDER_STREAM_API_KEY` - API key for authenticating with the order stream API
- `TX_FETCH_STRATEGY` - Transaction fetching strategy: `"block-receipts"` or `"tx-by-hash"` (default: `"tx-by-hash"`)
- `RUST_LOG` - Tracing log level (default: info)
- `CACHE_URI` - Cache storage URI (e.g., `file:///path/to/cache` or `s3://bucket-name`)

Cycle count computation (optional - requires both to be set):

- `BENTO_API_URL` - Bento API endpoint URL for cycle count computation
- `BENTO_API_KEY` - API key for the Bento API

### Rewards Indexer

The rewards indexer tracks staking, delegation, and PoVW rewards data. It is a separate indexer from the market indexer and can be run independently.

The following environment variables are required when running the rewards indexer:

- `ZKC_RPC_URL` (required) - Ethereum RPC endpoint URL for ZKC/veZKC contracts

Optional environment variables:

- `VEZKC_ADDRESS` - veZKC contract address (defaults to mainnet address)
- `ZKC_ADDRESS` - ZKC token address (defaults to mainnet address)
- `POVW_ACCOUNTING_ADDRESS` - PoVW accounting address (defaults to mainnet address)
- `RUST_LOG` - Tracing log level (default: info)

## API Documentation

The complete API documentation is available through the OpenAPI specification:

- **`GET /docs`** - Interactive Swagger UI documentation. Open this in a browser to explore and test all API endpoints.
- **`GET /openapi.yaml`** - OpenAPI 3.0 specification (YAML format)
- **`GET /openapi.json`** - OpenAPI 3.0 specification (JSON format)

For detailed request/response schemas, query parameters, and data models, please refer to the OpenAPI specification through Swagger UI (`/docs`) or directly via `/openapi.yaml`.

## Running API Locally

### Prerequisites

#### Start a PostgreSQL Instance

```bash
docker run -d \
  --name postgres-indexer \
  -e POSTGRES_USER=postgres \
  -e POSTGRES_PASSWORD=password \
  -e POSTGRES_DB=postgres \
  -p 5490:5432 \
  postgres:latest
```

This creates a database accessible at `postgres://postgres:password@localhost:5490/postgres`.

### Setup

The `manage_local` script provides convenient commands for running the indexer and API locally.

### Available Commands

Use the `manage_local` CLI tool to run the indexer and API:

```bash
./manage_local --help
```

**Commands:**

- `run-market-indexer` - Run market-indexer to populate database with market data (Ctrl+C to stop)
- `run-market-backfill` - Run market-indexer-backfill to recompute statuses/aggregates/chain_data
- `run-market-efficiency-indexer` - Run market-efficiency-indexer to compute market efficiency metrics
- `run-rewards-indexer` - Run rewards-indexer to populate database with staking/delegation/PoVW data
- `run-api` - Run API server

See `./manage_local --help` for detailed usage and options for each command.

### Example: Running Market Indexer and API

#### 1. Run the Market Indexer

```bash
# Set required environment variables
export BOUNDLESS_MARKET_ADDRESS="0xfd152dadc5183870710fe54f939eae3ab9f0fe82" # Base mainnet
export ORDER_STREAM_URL="https://base-mainnet.boundless.network" # Base mainnet
export MARKET_RPC_URL="https://proud-shy-morning.base-mainnet.quiknode.pro/YOUR_API_KEY/"  # recommend quicknode
export LOGS_RPC_URL="https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY"  # recommend alchemy

# Optional environment variables
export ORDER_STREAM_API_KEY="your-api-key"
export RUST_LOG="boundless_indexer=debug"  # default

# Run indexer processing blocks 35060420 to 40768860 (Ctrl+C to stop)
./manage_local run-market-indexer \
  --database postgres://postgres:password@localhost:5490/postgres \
  --start-block 35060420 \
  --end-block 40768860 \
  --batch-size 9999 \
  --cache-dir ./cache_dir
```

#### With Cycle Count Computation

To enable cycle count computation, add Bento API arguments:

```bash
# Run indexer with cycle count computation enabled
./manage_local run-market-indexer \
  --database postgres://postgres:password@localhost:5490/postgres \
  --start-block 35060420 \
  --end-block 40768860 \
  --batch-size 9999 \
  --cache-dir ./cache_dir \
  --bento-api-url "https://your-bento-api" \
  --bento-api-key "your-bento-api-key" \
  --max-concurrent-executing 20
```

**Options:**

- `--database` (required): PostgreSQL URL
- `--start-block`: Block to start from (optional)
- `--end-block`: Block to stop at (optional, runs indefinitely if not set)
- `--batch-size`: Blocks per batch (default: 9999)
- `--cache-dir`: Directory for cache file storage (optional)
- `--bento-api-url`: URL to Bento API for cycle count execution (optional, requires `--bento-api-key`)
- `--bento-api-key`: API key for Bento API (optional, requires `--bento-api-url`)
- `--max-concurrent-executing`: Max concurrent execution requests (default: 20)

#### 2. Start the API Server

```bash
# Start API server on port 3000
./manage_local run-api \
  --port 3000 \
  --database postgres://postgres:password@localhost:5490/postgres
```

Once the API server is running, you can test it:

```bash
curl http://localhost:3000/docs
```

### Running Market Backfill

The `run-market-backfill` command recomputes request statuses, market aggregates, or chain data from existing event data. This is useful for fixing inconsistencies or rebuilding computed tables after schema changes.

**Modes:**

- `statuses_and_aggregates`: Recompute both request statuses and all market summaries
- `aggregates`: Recompute only market summaries (faster, skips status updates)
- `chain_data`: Recompute chain data (transaction hashes, block numbers, etc.) for requests

```bash
# Set required environment variables
export MARKET_RPC_URL="https://your-rpc-endpoint.com"
export BOUNDLESS_MARKET_ADDRESS="0x..."

# Recompute statuses and aggregates from block 35060420 to 38958539
./manage_local run-market-backfill \
  --database postgres://postgres:password@localhost:5490/postgres \
  --mode statuses_and_aggregates \
  --start-block 35060420 \
  --end-block 38958539 \
  --cache-dir ./cache_market_x2 \
  --tx-fetch-strategy tx-by-hash

# Or recompute only aggregates
./manage_local run-market-backfill \
  --database postgres://postgres:password@localhost:5490/postgres \
  --mode aggregates \
  --start-block 35060420 \
  --end-block 38958539

# Recompute chain data using lookback (backfill last 1000 blocks from current)
./manage_local run-market-backfill \
  --database postgres://postgres:password@localhost:5490/postgres \
  --mode chain_data \
  --lookback-blocks 1000

# Recompute chain data for a specific block range
./manage_local run-market-backfill \
  --database postgres://postgres:password@localhost:5490/postgres \
  --mode chain_data \
  --start-block 35060420 \
  --end-block 38958539 \
  --cache-dir ./cache_dir
```

**Options:**

- `--database` (required): PostgreSQL URL
- `--mode` (required): `"statuses_and_aggregates"`, `"aggregates"`, or `"chain_data"`
- `--start-block`: Block to start backfill from (required if `--lookback-blocks` not provided)
- `--lookback-blocks`: Number of blocks to look back from current block (required if `--start-block` not provided)
  - When specified, fetches current block from RPC and calculates `start_block = current_block - lookback_blocks`
  - Either `--start-block` or `--lookback-blocks` must be provided, but not both
- `--end-block`: Block to end backfill at (optional, default: latest indexed)
- `--cache-dir`: Directory for cache file storage (optional)
- `--tx-fetch-strategy`: `"block-receipts"` or `"tx-by-hash"` (optional, default: `"block-receipts"`)

### Running Market Efficiency Indexer

The `run-market-efficiency-indexer` command computes market efficiency metrics — whether provers are locking the most profitable available orders. It reads locked requests from the database (populated by the market indexer), ranks them by profitability, and writes per-order and aggregated efficiency data.

It requires the market indexer to have already populated the database with request data.

```bash
# Run once with a 7-day lookback window
./manage_local run-market-efficiency-indexer \
  --database postgres://postgres:password@localhost:5490/postgres \
  --lookback-days 7 \
  --once

# Run for a specific time range (one-shot, exits when done)
./manage_local run-market-efficiency-indexer \
  --database postgres://postgres:password@localhost:5490/postgres \
  --start-time 1700000000 \
  --end-time 1700100000

# Run in a loop, recomputing every hour
./manage_local run-market-efficiency-indexer \
  --database postgres://postgres:password@localhost:5490/postgres \
  --interval 3600 \
  --lookback-days 3
```

**Options:**

- `--database` (required): PostgreSQL URL
- `--interval`: Seconds between runs when looping (default: 3600)
- `--lookback-days`: Number of days to look back for requests (default: 3)
- `--start-time`: Unix timestamp to start from (requires `--end-time`, runs once and exits)
- `--end-time`: Unix timestamp to end at (requires `--start-time`, runs once and exits)
- `--retries`: Number of retries before quitting on error (default: 3)
- `--once`: Run once using the lookback window from now, then exit

### Running Rewards Indexer

The `run-rewards-indexer` command indexes staking, delegation, and PoVW rewards data from the blockchain.

```bash
# Set required environment variables
export ZKC_RPC_URL="https://your-rpc-endpoint.com/v2/YOUR_API_KEY"
export VEZKC_ADDRESS="0x..."  # Optional
export ZKC_ADDRESS="0x..."  # Optional
export POVW_ACCOUNTING_ADDRESS="0x..."  # Optional
export RUST_LOG="info"  # Optional

# Run rewards indexer for 120 seconds
./manage_local run-rewards-indexer \
  --database postgres://postgres:password@localhost:5490/postgres \
  --duration 120
```

## Deployment

This Lambda function is designed to be deployed with AWS Lambda and API Gateway.
Build for Lambda deployment using cargo-lambda or similar tools. See `infra/indexer` for how we deploy it.

## Testing

Tests are ignored by default as they require an Ethereum RPC URL to be set, as they fetch real data from mainnet.

### Running the Tests

```
just test-indexer-api
```

#### Debugging Tests

Sometimes test failures can be swallowed since the indexer is run as a separate process e.g.

```
thread 'staking_tests::test_staking_epochs_summary' panicked at crates/lambdas/indexer-api/tests/local_integration.rs:102:26:
Failed to initialize test environment: Indexer exited with error: ExitStatus(unix_wait_status(256))
```

Use following to see all the logs:

```
cargo test --package indexer-api --test local_integration -- --ignored --nocapture
```
