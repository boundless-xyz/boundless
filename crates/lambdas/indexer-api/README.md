# Indexer API Lambda

AWS Lambda function providing REST API access to Boundless protocol staking, delegation, and PoVW rewards data.

## Environment Variables

- `DB_URL` (required) - PostgreSQL connection string to the indexer database (or SQLite for local testing)
- `RUST_LOG` (optional) - Tracing log level (default: info)

Additional environment variables for local testing:
- `ETH_RPC_URL` - **Required for indexer**: Ethereum RPC endpoint URL
- `VEZKC_ADDRESS` - Optional: veZKC contract address (defaults to mainnet address)
- `ZKC_ADDRESS` - Optional: ZKC token address (defaults to mainnet address)
- `POVW_ACCOUNTING_ADDRESS` - Optional: PoVW accounting address (defaults to mainnet address)

## API Documentation

The complete API documentation is available through the OpenAPI specification:

- **`GET /docs`** - Interactive Swagger UI documentation. Open this in a browser to explore and test all API endpoints.
- **`GET /openapi.yaml`** - OpenAPI 3.0 specification (YAML format)
- **`GET /openapi.json`** - OpenAPI 3.0 specification (JSON format)

For detailed request/response schemas, query parameters, and data models, please refer to the OpenAPI specification through Swagger UI (`/docs`) or directly via `/openapi.yaml`.

## Running API Locally

### Setup

Export `ETH_RPC_URL` to be an archive node endpoint with support for querying events.

### Running the Services Locally

Use the `manage_local` CLI tool to run the indexer and API:

```bash
# Run the indexer to populate a SQLite database
# Arguments: <db_file> [duration_seconds]
./manage_local run-indexer test.db 30

# Run the API server with the populated database
# Arguments: <port> <db_file>
./manage_local run-api 3000 test.db
```

#### Example workflow

```bash
# 1. Create and populate a test database (runs for 30 seconds)
./manage_local run-indexer my_test.db 30

# 2. Start the API server on port 3000
./manage_local run-api 3000 my_test.db
```

Once the API server is running, you can test it:

```bash
# Health check
curl http://localhost:3000/health

# Get PoVW aggregate data
curl http://localhost:3000/v1/povw
```

You can also access the swagger UI at http://localhost:3000/docs

### Database

The SQLite database file will be created in the current directory. You can inspect it with any SQLite client:

```bash
sqlite3 test.db
.tables  # Show all tables
.schema povw_rewards  # Show schema for a table
SELECT * FROM povw_summary_stats;  # Query data
```

## Deployment

This Lambda function is designed to be deployed with AWS Lambda and API Gateway.
Build for Lambda deployment using cargo-lambda or similar tools.

## Testing

The crate includes integration tests that spin up the indexer and API server to test endpoints. These tests require an Ethereum RPC URL to be set, as they fetch real data from mainnet.

### Running the Tests
Each test module:
1. Spawns a rewards-indexer process to populate a temporary SQLite database
2. Starts the API server on a random port
3. Makes HTTP requests to test various endpoints
4. Cleans up processes and temporary files after completion

```bash
# Set your RPC URL (or add to .env file)
export ETH_RPC_URL="https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY"

# Run all integration tests (tests are ignored by default since they require RPC)
cargo test --test local_integration -- --ignored

# Run specific test modules
cargo test --test local_integration povw_tests -- --ignored
cargo test --test local_integration staking_tests -- --ignored
cargo test --test local_integration delegations_tests -- --ignored
cargo test --test local_integration docs_tests -- --ignored

# Run with verbose output and single thread (recommended for debugging)
RUST_LOG=debug cargo test --test local_integration -- --ignored --nocapture --test-threads=1

# Note: Tests will automatically build the required binaries (rewards-indexer and local-server)
```