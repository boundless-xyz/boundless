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
- `BENTO_API_URL` (required) - Bento API endpoint URL for cycle count computation
- `BENTO_API_KEY` (required) - API key for the Bento API

Optional environment variables:

- `LOGS_RPC_URL` - Separate RPC endpoint for `eth_getLogs` calls. If not provided, uses `MARKET_RPC_URL` for all operations
- `ORDER_STREAM_URL` - Optional URL of the order stream API for off-chain order indexing
- `ORDER_STREAM_API_KEY` - Optional API key for authenticating with the order stream API
- `TX_FETCH_STRATEGY` - Transaction fetching strategy: `"block-receipts"` or `"tx-by-hash"` (default: `"tx-by-hash"`)
- `RUST_LOG` - Tracing log level (default: info)
- `CACHE_URI` - Optional cache storage URI (e.g., `file:///path/to/cache` or `s3://bucket-name`)

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

### Setup

The `manage_local` script provides convenient commands for running the indexer and API locally.

### Available Commands

Use the `manage_local` CLI tool to run the indexer and API:

```bash
./manage_local --help
```

**Commands:**

- `run-market-indexer <database> [duration] [start_block] [end_block] [batch_size] [cache_dir]` - Run market-indexer to populate database with market data
- `run-market-backfill <database> <mode> <start_block> [end_block] [cache_dir] [tx_fetch_strategy]` - Run market-indexer-backfill to recompute statuses/aggregates
- `run-rewards-indexer <database> [duration] [end_epoch] [end_block] [batch_size]` - Run rewards-indexer to populate database with staking/delegation/PoVW data
- `run-api <port> <database>` - Run API server

### Example: Running Market Indexer and API

#### 1. Run the Market Indexer

```bash
# Set required environment variables
export MARKET_RPC_URL="https://your-rpc-endpoint.com/v2/YOUR_API_KEY" # recommend quicknode
export BOUNDLESS_MARKET_ADDRESS="0xfd152dadc5183870710fe54f939eae3ab9f0fe82"
export LOGS_RPC_URL="https://your-logs-rpc-endpoint.com/v2/YOUR_API_KEY"  # recommend alchemy
export ORDER_STREAM_URL="https://order-stream.example.com"  # Optional
export ORDER_STREAM_API_KEY="your-api-key"  # Optional
export TX_FETCH_STRATEGY="tx-by-hash"  # Optional: "block-receipts" or "tx-by-hash"
export RUST_LOG="info"  # Optional
export BENTO_API_URL="https://bento-api.url"
export BENTO_API_KEY="api-key"

# Run indexer for max of 7200 seconds (2 hours), processing blocks 35060420 to 38958539
# Using PostgreSQL database with cache directory
./manage_local run-market-indexer \
  postgres://postgres:password@localhost:5432/postgres \
  7200 \
  35060420 \
  38958539 \
  9999 \
  ./cache_market_x2
```

**Parameters:**

- `database`: PostgreSQL URL
- `duration`: Seconds to run (default: 120)
- `start_block`: Block to start from (optional)
- `end_block`: Block to stop at (optional)
- `batch_size`: Blocks per batch (default: 500)
- `cache_dir`: Directory for cache file storage (optional)

#### 2. Start the API Server

```bash
# Start API server on port 3000
./manage_local run-api 3000 postgres://postgres:password@localhost:5432/postgres
```

Once the API server is running, you can test it:

```bash
curl http://localhost:3000/docs
```

### Running Market Backfill

The `run-market-backfill` command recomputes request statuses and market aggregates from existing event data. This is useful for fixing inconsistencies or rebuilding computed tables after schema changes.

**Modes:**

- `statuses_and_aggregates`: Recompute both request statuses and all market summaries
- `aggregates`: Recompute only market summaries (faster, skips status updates)

```bash
# Set required environment variables
export MARKET_RPC_URL="https://your-rpc-endpoint.com"
export BOUNDLESS_MARKET_ADDRESS="0x..."
export TX_FETCH_STRATEGY="tx-by-hash"  # Optional

# Recompute statuses and aggregates from block 35060420 to 38958539
./manage_local run-market-backfill \
  postgres://postgres:password@localhost:5432/postgres \
  statuses_and_aggregates \
  35060420 \
  38958539 \
  ./cache_market_x2 \
  tx-by-hash

# Or recompute only aggregates
./manage_local run-market-backfill \
  postgres://postgres:password@localhost:5432/postgres \
  aggregates \
  35060420 \
  38958539
```

**Parameters:**

- `database`: PostgreSQL URL
- `mode`: `"statuses_and_aggregates"` or `"aggregates"`
- `start_block`: Block to start backfill from (required)
- `end_block`: Block to end backfill at (optional, default: latest indexed)
- `cache_dir`: Directory for cache file storage (optional)
- `tx_fetch_strategy`: `"block-receipts"` or `"tx-by-hash"` (optional, default: `"block-receipts"`)

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
  postgres://postgres:password@localhost:5432/postgres \
  120
```

=

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
