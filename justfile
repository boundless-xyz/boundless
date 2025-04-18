# Variables
DEFAULT_DATABASE_URL := "postgres://postgres:password@localhost:5432/postgres"
DATABASE_URL := env_var_or_default("DATABASE_URL", DEFAULT_DATABASE_URL)

LOGS_DIR := "logs"
PID_FILE := LOGS_DIR + "/localnet.pid"

# Show available commands
default:
    @just --list

# Check that required dependencies are installed
check-deps:
    #!/usr/bin/env bash
    for cmd in forge cargo anvil jq; do
        command -v $cmd >/dev/null 2>&1 || { echo "Error: $cmd is not installed."; exit 1; }
    done

# Run all CI checks
ci: check test

# Run all tests
test: test-foundry test-cargo

# Run Foundry tests
test-foundry:
    forge test -vvv --isolate

# Run all Cargo tests
test-cargo: test-cargo-root test-cargo-bento test-cargo-example test-cargo-db

# Run Cargo tests for root workspace
test-cargo-root:
    RISC0_DEV_MODE=1 cargo test --workspace --exclude order-stream --exclude boundless-cli -- --include-ignored

# Run Cargo tests for bento
test-cargo-bento:
    cd bento && RISC0_DEV_MODE=1 cargo test --workspace --exclude taskdb -- --include-ignored

# Run Cargo tests for counter example
test-cargo-example:
    cd examples/counter && \
    forge build && \
    RISC0_DEV_MODE=1 cargo test

# Run database tests
test-cargo-db: 
    just test-db setup
    DATABASE_URL={{DATABASE_URL}} sqlx migrate run --source ./bento/crates/taskdb/migrations/
    cd bento && DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo test -p taskdb
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo test -p order-stream -- --include-ignored
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo test -p boundless-cli -- --include-ignored
    just test-db clean

# Manage test postgres instance (setup or clean, defaults to setup)
test-db action="setup":
    #!/usr/bin/env bash
    if [ "{{action}}" = "setup" ]; then
        docker inspect postgres-test > /dev/null 2>&1 || \
        docker run -d \
            --name postgres-test \
            -e POSTGRES_PASSWORD=password \
            -p 5432:5432 \
            postgres:latest
        # Wait for PostgreSQL to be ready
        sleep 3
    elif [ "{{action}}" = "clean" ]; then
        docker stop postgres-test
        docker rm postgres-test
    else
        echo "Unknown action: {{action}}"
        echo "Available actions: setup, clean"
        exit 1
    fi

# Run all formatting and linting checks
check: check-links check-license check-format check-clippy

# Check links in markdown files
check-links:
    @echo "Checking links in markdown files..."
    git ls-files '*.md' ':!:documentation/*' | xargs lychee --base . --cache --

# Check licenses
check-license:
    @python license-check.py

# Check code formatting
check-format:
    cargo sort --workspace --check
    cargo fmt --all --check
    cd examples/counter && cargo sort --workspace --check
    cd examples/counter && cargo fmt --all --check
    cd examples/smart-contract-requestor && cargo sort --workspace --check
    cd examples/smart-contract-requestor && cargo fmt --all --check
    cd bento && cargo sort --workspace --check
    cd bento && cargo fmt --all --check
    cd documentation && bun run check
    dprint check
    forge fmt --check

# Run Cargo clippy
check-clippy:
    RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNEL=1 \
    cargo clippy --workspace --all-targets -F boundless-order-generator/zeth
    cd examples/counter && RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNEL=1 \
    cargo clippy --workspace --all-targets
    cd bento && RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNEL=1 \
    cargo clippy --workspace --all-targets

# Format all code
format:
    cargo sort --workspace
    cargo fmt --all
    cd examples/counter && cargo sort --workspace
    cd examples/counter && cargo fmt --all
    cd examples/smart-contract-requestor && cargo sort --workspace
    cd examples/smart-contract-requestor && cargo fmt --all
    cd bento && cargo sort --workspace
    cd bento && cargo fmt --all
    cd documentation && bun run format-markdown
    dprint fmt
    forge fmt

# Clean up all build artifacts
clean: 
    @just localnet down
    @echo "Cleaning up..."
    @rm -rf {{LOGS_DIR}} ./broadcast
    cargo clean
    forge clean
    @echo "Cleanup complete."

# Manage the development network (up or down, defaults to up)
localnet action="up": check-deps
    #!/usr/bin/env bash
    # Localnet-specific variables
    ANVIL_PORT="8545"
    ANVIL_BLOCK_TIME="2"
    RISC0_DEV_MODE="1"
    CHAIN_KEY="anvil"
    RUST_LOG="info,broker=debug,boundless_market=debug,order_stream=debug"
    # This key is a prefunded address for the anvil test configuration (index 0)
    DEPLOYER_PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
    PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
    ADMIN_ADDRESS="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
    DEPOSIT_AMOUNT="100000000000000000000"
    
    if [ "{{action}}" = "up" ]; then
        mkdir -p {{LOGS_DIR}}
        
        # Create .env.localnet from template if it doesn't exist
        if [ ! -f .env.localnet ]; then
            echo "Creating .env.localnet from template..."
            cp .env.localnet-template .env.localnet || { echo "Error: .env.localnet-template not found"; exit 1; }
        fi
        
        echo "Building contracts..."
        forge build || { echo "Failed to build contracts"; just localnet down; exit 1; }
        echo "Building Rust project..."
        cargo build --bin broker || { echo "Failed to build broker binary"; just localnet down; exit 1; }
        cargo build --bin order_stream || { echo "Failed to build order-stream binary"; just localnet down; exit 1; }
        # Check if Anvil is already running
        if nc -z localhost $ANVIL_PORT; then
            echo "Anvil is already running on port $ANVIL_PORT. Reusing existing instance."
        else
            echo "Starting Anvil..."
            anvil -b $ANVIL_BLOCK_TIME > {{LOGS_DIR}}/anvil.txt 2>&1 & echo $! >> {{PID_FILE}}
            sleep 5
        fi
        echo "Deploying contracts..."
        DEPLOYER_PRIVATE_KEY=$DEPLOYER_PRIVATE_KEY CHAIN_KEY=$CHAIN_KEY RISC0_DEV_MODE=$RISC0_DEV_MODE BOUNDLESS_MARKET_OWNER=$ADMIN_ADDRESS forge script contracts/scripts/Deploy.s.sol --rpc-url http://localhost:$ANVIL_PORT --broadcast -vv || { echo "Failed to deploy contracts"; just localnet down; exit 1; }
        echo "Fetching contract addresses..."
        SET_VERIFIER_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "RiscZeroSetVerifier") | .contractAddress' ./broadcast/Deploy.s.sol/31337/run-latest.json)
        BOUNDLESS_MARKET_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "ERC1967Proxy") | .contractAddress' ./broadcast/Deploy.s.sol/31337/run-latest.json)
        HIT_POINTS_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "HitPoints") | .contractAddress' ./broadcast/Deploy.s.sol/31337/run-latest.json | head -n 1)
        echo "Contract deployed at addresses:"
        echo "SET_VERIFIER_ADDRESS=$SET_VERIFIER_ADDRESS"
        echo "BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS"
        echo "HIT_POINTS_ADDRESS=$HIT_POINTS_ADDRESS"
        echo "Updating .env.localnet file..."
        # Update the environment variables in .env.localnet
        sed -i.bak "s/^SET_VERIFIER_ADDRESS=.*/SET_VERIFIER_ADDRESS=$SET_VERIFIER_ADDRESS/" .env.localnet
        sed -i.bak "s/^BOUNDLESS_MARKET_ADDRESS=.*/BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS/" .env.localnet
        # Add HIT_POINTS_ADDRESS to .env.localnet
        grep -q "^HIT_POINTS_ADDRESS=" .env.localnet && \
            sed -i.bak "s/^HIT_POINTS_ADDRESS=.*/HIT_POINTS_ADDRESS=$HIT_POINTS_ADDRESS/" .env.localnet || \
            echo "HIT_POINTS_ADDRESS=$HIT_POINTS_ADDRESS" >> .env.localnet
        rm .env.localnet.bak
        echo ".env.localnet file updated successfully."
        echo "Minting HP for prover address."
        cast send --private-key $DEPLOYER_PRIVATE_KEY \
            --rpc-url http://localhost:$ANVIL_PORT \
            $HIT_POINTS_ADDRESS "mint(address, uint256)" $ADMIN_ADDRESS $DEPOSIT_AMOUNT

        # Start order stream server
        just test-db setup
        DATABASE_URL={{DATABASE_URL}} RUST_LOG=$RUST_LOG ./target/debug/order_stream \
            --min-balance 0 \
            --bypass-addrs="0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f" \
            --boundless-market-address $BOUNDLESS_MARKET_ADDRESS > {{LOGS_DIR}}/order_stream.txt 2>&1 & echo $! >> {{PID_FILE}}
        # Start a broker
        RISC0_DEV_MODE=$RISC0_DEV_MODE RUST_LOG=$RUST_LOG ./target/debug/broker \
            --private-key $PRIVATE_KEY \
            --boundless-market-address $BOUNDLESS_MARKET_ADDRESS \
            --set-verifier-address $SET_VERIFIER_ADDRESS \
            --rpc-url http://localhost:$ANVIL_PORT \
            --order-stream-url http://localhost:8585 \
            --deposit-amount $DEPOSIT_AMOUNT > {{LOGS_DIR}}/broker.txt 2>&1 & echo $! >> {{PID_FILE}}
        echo "Localnet is running!"
        echo "Make sure to run 'source .env.localnet' to load the environment variables before interacting with the network."
    elif [ "{{action}}" = "down" ]; then
        if [ -f {{PID_FILE}} ]; then
            while read pid; do
                kill $pid 2>/dev/null || true
            done < {{PID_FILE}}
            rm {{PID_FILE}}
        fi
        just test-db clean
    else
        echo "Unknown action: {{action}}"
        echo "Available actions: up, down"
        exit 1
    fi

# Update cargo dependencies
cargo-update:
    cargo update
    cd examples/counter && cargo update

# Start the bento service
bento action="up" env_file="./.env.broker" compose_flags="":
    #!/usr/bin/env bash
    if [ -n "{{env_file}}" ]; then
        ENV_FILE="{{env_file}}"
    else
        ENV_FILE="./.env.broker"
    fi

    if ! command -v docker &> /dev/null; then
        echo "Error: Docker command is not available. Please make sure you have docker in your PATH."
        exit 1
    fi

    if ! docker compose version &> /dev/null; then
        echo "Error: Docker compose command is not available. Please make sure you have docker in your PATH."
        exit 1
    fi

    if [ "{{action}}" = "up" ]; then
        if [ "$ENV_FILE" = "./.env.broker" ] && [ ! -f "$ENV_FILE" ]; then
            echo "Creating $ENV_FILE from template..."
            if [ -f ".env.broker-template" ]; then
                cp .env.broker-template "$ENV_FILE"
                echo "Please review and update values in $ENV_FILE before running the service again."
            else
                echo "Error: .env.broker-template not found."
                exit 1
            fi
        fi

        if [ ! -f "$ENV_FILE" ]; then
            echo "Error: Environment file $ENV_FILE does not exist."
            exit 1
        fi

        echo "Starting Docker Compose services using environment file: $ENV_FILE"
        docker compose {{compose_flags}} --env-file "$ENV_FILE" up --build -d
        echo "Docker Compose services have been started."
    elif [ "{{action}}" = "down" ]; then
        echo "Stopping Docker Compose services using environment file: $ENV_FILE"
        if docker compose {{compose_flags}} --env-file "$ENV_FILE" down; then
            echo "Docker Compose services have been stopped and removed."
        else
            echo "Error: Failed to stop Docker Compose services."
            exit 1
        fi
    elif [ "{{action}}" = "clean" ]; then
        echo "Stopping and cleaning Docker Compose services using environment file: $ENV_FILE"
        if docker compose {{compose_flags}} --env-file "$ENV_FILE" down -v; then
            echo "Docker Compose services have been stopped and volumes have been removed."
        else
            echo "Error: Failed to clean Docker Compose services."
            exit 1
        fi
    elif [ "{{action}}" = "logs" ]; then
        echo "Docker logs using environment file: $ENV_FILE"
        docker compose {{compose_flags}} --env-file "$ENV_FILE" logs -f
    else
        echo "Unknown action: {{action}}"
        echo "Available actions: up, down, clean, logs"
        exit 1
    fi

env NETWORK:
	#!/usr/bin/env bash
	FILE=".env.{{NETWORK}}"
	if [ -f "$FILE" ]; then
		grep -v '^#' "$FILE" | tr -d '"' | xargs -I {} echo export {}
	else
		echo "Error: $FILE file not found." >&2
		exit 1
	fi

# Run the broker service with a bento cluster for proving.
broker action="up" env_file="./.env.broker":
    just bento "{{action}}" "{{env_file}}" "--profile broker"

# Run the setup script
bento-setup:
    #!/usr/bin/env bash
    ./scripts/setup.sh

# Run the set_nvcc_flags script
bento-set-nvcc-flags:
    #!/usr/bin/env bash
    ./scripts/set_nvcc_flags.sh

# Check job status in Postgres
job-status job_id:
    #!/usr/bin/env bash
    ./scripts/job_status.sh {{job_id}}
