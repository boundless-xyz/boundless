# Running Indexer and API Locally

This guide explains how to run the rewards-indexer and API server locally using SQLite for testing and development.

## Prerequisites

- Rust toolchain installed
- Access to an Ethereum RPC endpoint (e.g., Alchemy, Infura)

## Setup

1. **Set up your environment variables**

   Copy the `.env` file and add your RPC URL:
   ```bash
   # Edit .env and replace YOUR_API_KEY with your actual key
   export ETH_RPC_URL=https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY
   ```

2. **Build the binaries**

   ```bash
   cargo build -p boundless-indexer --bin rewards-indexer --release
   cargo build -p indexer-api --bin local-server --release
   ```

## Running the Services

### Option 1: Using the test script

The easiest way to test both services:

```bash
# Make sure ETH_RPC_URL is set
export ETH_RPC_URL=https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY

# Run the test script
./test_local_indexer.sh
```

This script will:
1. Build both services
2. Run the rewards-indexer for 60 seconds to populate data
3. Start the API server on port 3000

### Option 2: Running services manually

1. **Run the rewards-indexer to populate data:**

   ```bash
   DATABASE_URL=sqlite:local_indexer.db \
   VEZKC_ADDRESS=0xE8Ae8eE8ffa57F6a79B6Cbe06BAFc0b05F3ffbf4 \
   ZKC_ADDRESS=0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555 \
   POVW_ACCOUNTING_ADDRESS=0x319bd4050b2170a7aE3Ead3E6d5AB8a5c7cFBDF8 \
   ./target/release/rewards-indexer \
       --rpc-url "$ETH_RPC_URL" \
       --vezkc-address 0xE8Ae8eE8ffa57F6a79B6Cbe06BAFc0b05F3ffbf4 \
       --zkc-address 0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555 \
       --povw-accounting-address 0x319bd4050b2170a7aE3Ead3E6d5AB8a5c7cFBDF8 \
       --db sqlite:local_indexer.db \
       --interval 600
   ```

   Let it run for a minute or until you see "Sleeping for 600 seconds", then stop it with Ctrl+C.

2. **Start the API server:**

   ```bash
   DB_URL=sqlite:local_indexer.db \
   PORT=3000 \
   ./target/release/local-server
   ```

## Testing the API

Once the API server is running, you can test it:

```bash
# Health check
curl http://localhost:3000/health

# Get PoVW aggregate data
curl http://localhost:3000/v1/povw

# Get staking aggregate data
curl http://localhost:3000/v1/staking

# Get data for a specific epoch
curl http://localhost:3000/v1/povw/epochs/4

# Get staking leaderboard
curl http://localhost:3000/v1/staking/addresses
```

## Database

The SQLite database file `local_indexer.db` will be created in the current directory. You can inspect it with any SQLite client:

```bash
sqlite3 local_indexer.db
.tables  # Show all tables
.schema povw_rewards  # Show schema for a table
SELECT * FROM povw_summary_stats;  # Query data
```

## Notes

- The indexer will fetch real blockchain data from Ethereum mainnet
- Initial data population may take a few minutes
- The database persists between runs unless you delete the `.db` file
- Both services use the same SQLite database file
- The API server runs on port 3000 by default (configurable via PORT env var)