# Variables
DEFAULT_DATABASE_URL := "postgres://postgres:password@localhost:5433/postgres"
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
test-cargo: test-cargo-root test-cargo-example test-cargo-db

# Run Cargo tests for root workspace
test-cargo-root:
    RISC0_DEV_MODE=1 cargo nextest run --workspace --exclude order-stream --exclude boundless-cli --exclude indexer-api --exclude indexer-monitor --exclude boundless-indexer --exclude boundless-slasher --exclude boundless-bench --features test-r0vm

# Run Cargo tests for counter example
test-cargo-example:
    cd examples/counter && \
    forge build && \
    RISC0_DEV_MODE=1 cargo nextest run

# Run database tests
test-cargo-db:
    just test-db setup
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p boundless-bench --features test-r0vm
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p order-stream
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p boundless-indexer --features test-r0vm
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p boundless-cli --features test-r0vm
    just test-db clean

# Run slasher tests (requires database)
test-slasher:
    just test-db setup
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p boundless-slasher --features test-r0vm
    just test-db clean

# Run order-stream tests (requires database)
test-order-stream:
    #!/usr/bin/env bash
    set -e
    just test-db clean || true
    just test-db setup
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p order-stream

# Run all indexer tests (requires both RPC URLs)
test-indexer:
    #!/usr/bin/env bash
    set -e
    if [ -z "$ETH_MAINNET_RPC_URL" ]; then
        echo "Error: ETH_MAINNET_RPC_URL environment variable must be set to a mainnet archive node that supports event querying"
        exit 1
    fi
    if [ -z "$BASE_MAINNET_RPC_URL" ]; then
        echo "Error: BASE_MAINNET_RPC_URL environment variable must be set to a mainnet archive node that supports event querying"
        exit 1
    fi
    just test-db clean || true
    just test-db setup
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p boundless-indexer --features test-r0vm,test-rpc

# Run indexer market integration tests (requires BASE_MAINNET_RPC_URL)
test-indexer-market:
    #!/usr/bin/env bash
    set -e
    if [ -z "$BASE_MAINNET_RPC_URL" ]; then
        echo "Error: BASE_MAINNET_RPC_URL environment variable must be set to a mainnet archive node that supports event querying"
        exit 1
    fi
    just test-db clean || true
    just test-db setup
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p boundless-indexer --test market --features test-r0vm,test-rpc

# Run indexer rewards integration tests (requires ETH_MAINNET_RPC_URL)
test-indexer-rewards:
    #!/usr/bin/env bash
    set -e
    if [ -z "$ETH_MAINNET_RPC_URL" ]; then
        echo "Error: ETH_MAINNET_RPC_URL environment variable must be set to a mainnet archive node that supports event querying"
        exit 1
    fi
    just test-db clean || true
    just test-db setup
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p boundless-indexer --test rewards --features test-r0vm,test-rpc

# Run indexer-api integration tests (requires both RPC URLs)
test-indexer-api:
    #!/usr/bin/env bash
    set -e
    if [ -z "$ETH_MAINNET_RPC_URL" ]; then
        echo "Error: ETH_MAINNET_RPC_URL environment variable must be set to a mainnet archive node that supports event querying"
        exit 1
    fi
    if [ -z "$BASE_MAINNET_RPC_URL" ]; then
        echo "Error: BASE_MAINNET_RPC_URL environment variable must be set to a mainnet archive node that supports event querying"
        exit 1
    fi
    just test-db clean || true
    just test-db setup
    # Ensure indexer binaries are built with latest changes, API tests depend on them.
    RISC0_DEV_MODE=1 cargo build -p boundless-indexer --bin rewards-indexer --bin market-indexer
    DATABASE_URL={{DATABASE_URL}} RISC0_DEV_MODE=1 cargo nextest run -p indexer-api --features test-rpc

# Manage test postgres instance (setup or clean, defaults to setup)
test-db action="setup":
    #!/usr/bin/env bash
    if [ "{{action}}" = "setup" ]; then
        docker inspect postgres-test > /dev/null 2>&1 || \
        docker run -d \
            --name postgres-test \
            -e POSTGRES_PASSWORD=password --shm-size=2gb \
            -p 5433:5432 \
            postgres:latest -c max_connections=500
        # Wait for PostgreSQL to be ready
        sleep 3
        docker exec -u postgres postgres-test psql -U postgres -c "CREATE DATABASE test_db;"
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

check-main: check-format-main check-clippy-main check-license check-links

# Check links in markdown files
check-links:
    @echo "Checking links in markdown files..."
    git ls-files '*.md' | xargs lychee --base . --cache --

# Check licenses
check-license:
    @echo "Checking licenses..."
    @python license-check.py

# Check code formatting
check-format: check-format-main
    cd examples/counter && cargo sort --workspace --check
    cd examples/counter && cargo fmt --all --check
    cd examples/smart-contract-requestor && cargo sort --workspace --check
    cd examples/smart-contract-requestor && cargo fmt --all --check
    cd examples/composition && cargo sort --workspace --check
    cd examples/composition && cargo fmt --all --check
    cd examples/counter-with-callback && cargo sort --workspace --check
    cd examples/counter-with-callback && cargo fmt --all --check
    cd crates/guest/assessor && cargo sort --workspace --check
    cd crates/guest/assessor && cargo fmt --all --check
    cd crates/guest/util && cargo sort --workspace --check
    cd crates/guest/util && cargo fmt --all --check
    dprint check
    forge fmt --check

check-format-main:
    cargo sort --workspace --check .
    cargo fmt --all --check

# Run Cargo clippy for main workspace and all examples
check-clippy: check-clippy-main
    cd examples/counter && forge build && \
    RUSTFLAGS=-Dwarnings RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNELS=1 \
    cargo clippy --workspace --all-targets

    cd examples/composition && forge build && \
    RUSTFLAGS=-Dwarnings RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNELS=1 \
    cargo clippy --workspace --all-targets

    cd examples/counter-with-callback && \
    forge build && \
    RUSTFLAGS=-Dwarnings RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNELS=1 \
    cargo clippy --workspace --all-targets

    cd examples/smart-contract-requestor && \
    forge build && \
    RUSTFLAGS=-Dwarnings RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNELS=1 \
    cargo clippy --workspace --all-targets

    cd examples/blake3-groth16 && \
    RUSTFLAGS=-Dwarnings RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNELS=1 \
    cargo clippy --workspace --all-targets

# Check that the main workspace compiles (same RISC0 skip flags as check-clippy-main)
check-cargo-main:
    RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNELS=1 \
    cargo check --workspace --all-targets

# Check Cargo clippy for the main workspace
check-clippy-main:
    RUSTFLAGS=-Dwarnings RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNELS=1 \
    cargo clippy --workspace --all-targets

# Format all code
format:
    cargo sort --workspace .
    cargo fmt --all
    cd examples/counter && cargo sort --workspace
    cd examples/counter && cargo fmt --all
    cd examples/smart-contract-requestor && cargo sort --workspace
    cd examples/smart-contract-requestor && cargo fmt --all
    cd examples/composition && cargo sort --workspace
    cd examples/composition && cargo fmt --all
    cd examples/counter-with-callback && cargo sort --workspace
    cd examples/counter-with-callback && cargo fmt --all
    cd crates/guest/assessor && cargo sort --workspace
    cd crates/guest/assessor && cargo fmt --all
    cd crates/guest/util && cargo sort --workspace
    cd crates/guest/util && cargo fmt --all
    dprint fmt
    forge fmt

# Clean up all build artifacts
clean:
    @just localnet down || true
    @echo "Cleaning up..."
    @rm -rf {{LOGS_DIR}} ./broadcast
    cargo clean
    forge clean
    cd bento && cargo clean
    cd examples/counter && cargo clean
    cd examples/composition && cargo clean
    cd examples/counter-with-callback && cargo clean
    cd examples/smart-contract-requestor && cargo clean
    cd crates/guest/assessor && cargo clean
    cd crates/guest/assessor/assessor-guest && cargo clean
    cd crates/guest/util && cargo clean
    @echo "Cleanup complete."

# Manage the development network (up, down, clean, or logs)
localnet action="up":
    #!/usr/bin/env bash
    set -e
    COMPOSE="docker compose -f dockerfiles/compose.localnet.yml"
    DEV_MODE="${RISC0_DEV_MODE:-1}"

    # Ensure broker.toml exists with localnet-compatible price oracle config
    if [ ! -f broker.toml ]; then
        cp broker-template.toml broker.toml
    fi
    if ! grep -q '\[price_oracle\]' broker.toml 2>/dev/null; then
        printf '\n# Localnet price oracle: static prices, no on-chain feeds (anvil has no chainlink).\n[price_oracle]\neth_usd = "2500.0"\nzkc_usd = "1.0"\n\n[price_oracle.onchain.chainlink]\nenabled = false\n' >> broker.toml
    fi

    # In dev mode, include the broker container
    if [ "$DEV_MODE" = "1" ] || [ "$DEV_MODE" = "true" ]; then
        COMPOSE="$COMPOSE --profile dev-broker"
    fi

    case "{{action}}" in
        up)
            [ -f .env.localnet ] || cp .env.localnet-template .env.localnet
            $COMPOSE up -d --build --wait
            ;;
        down)  $COMPOSE --profile dev-broker down ;;
        clean) $COMPOSE --profile dev-broker down -v && rm -f .env.localnet ;;
        logs)  $COMPOSE logs -f ;;
        *)     echo "Available actions: up, down, clean, logs"; exit 1 ;;
    esac

# Update cargo dependencies
cargo-update:
    cargo update
    cd examples/counter && cargo update

# Regenerate lockfiles (without bumping versions)
update-lockfiles:
    cargo fetch
    cd bento && cargo fetch
    cd examples/counter && cargo fetch
    cd examples/composition && cargo fetch
    cd examples/counter-with-callback && cargo fetch
    cd examples/smart-contract-requestor && cargo fetch
    cd examples/blake3-groth16 && cargo fetch
    cd examples/request-stream && cargo fetch

# Start the bento service
# Set BOUNDLESS_BUILD to control image building:
#   BOUNDLESS_BUILD=all                - build all images from source
#   BOUNDLESS_BUILD=broker             - build only the broker image
#   BOUNDLESS_BUILD="broker rest_api"  - build specific services
bento action="up" env_file="" compose_flags="" detached="true" services="":
    #!/usr/bin/env bash
    if [ -n "{{env_file}}" ]; then
        ENV_FILE_ARG="--env-file {{env_file}}"
    else
        ENV_FILE_ARG=""
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
        if [ -n "{{env_file}}" ] && [ ! -f "{{env_file}}" ]; then
            echo "Error: Environment file {{env_file}} does not exist."
            exit 1
        fi

        echo "Starting Docker Compose services"
        if [ -n "{{env_file}}" ]; then
            echo "Using environment file: {{env_file}}"
        else
            echo "Using default values from compose.yml"
        fi

        if [ "{{detached}}" = "true" ]; then
            DETACHED_FLAG="-d"
        else
            DETACHED_FLAG=""
        fi
        
        BOUNDLESS_BUILD="${BOUNDLESS_BUILD:-}"

        # When building from source, clear the prebuilt image tags and set
        # dockerfiles to the source-build variants so Compose builds from source
        # instead of pulling pre-built images.
        if [ "$BOUNDLESS_BUILD" = "all" ]; then
            export AGENT_IMAGE="" CPU_AGENT_IMAGE="" BROKER_IMAGE="" REST_API_IMAGE="" BENTO_CLI_IMAGE=""
            export CPU_AGENT_DOCKERFILE="dockerfiles/agent.cpu.dockerfile"
            export GPU_AGENT_DOCKERFILE="dockerfiles/agent.dockerfile"
            export BROKER_DOCKERFILE="dockerfiles/broker.dockerfile"
            export REST_API_DOCKERFILE="dockerfiles/rest_api.dockerfile"
            export BENTO_CLI_DOCKERFILE="dockerfiles/bento_cli.dockerfile"
            docker compose {{compose_flags}} $ENV_FILE_ARG up --build $DETACHED_FLAG {{services}}
        elif [ -n "$BOUNDLESS_BUILD" ]; then
            for svc in $BOUNDLESS_BUILD; do
                case "$svc" in
                    exec_agent|aux_agent) export CPU_AGENT_IMAGE="" CPU_AGENT_DOCKERFILE="${CPU_AGENT_DOCKERFILE:-dockerfiles/agent.cpu.dockerfile}" ;;
                    gpu_prove_agent) export AGENT_IMAGE="" GPU_AGENT_DOCKERFILE="${GPU_AGENT_DOCKERFILE:-dockerfiles/agent.dockerfile}" ;;
                    miner) export BENTO_CLI_IMAGE="" BENTO_CLI_DOCKERFILE="${BENTO_CLI_DOCKERFILE:-dockerfiles/bento_cli.dockerfile}" ;;
                    broker)        export BROKER_IMAGE="" BROKER_DOCKERFILE="${BROKER_DOCKERFILE:-dockerfiles/broker.dockerfile}" ;;
                    rest_api)      export REST_API_IMAGE="" REST_API_DOCKERFILE="${REST_API_DOCKERFILE:-dockerfiles/rest_api.dockerfile}" ;;
                    *) echo "WARNING: unrecognized BOUNDLESS_BUILD service '$svc' — will not clear its image tag" ;;
                esac
            done
            docker compose {{compose_flags}} $ENV_FILE_ARG build $BOUNDLESS_BUILD
            docker compose {{compose_flags}} $ENV_FILE_ARG up --pull always $DETACHED_FLAG {{services}}
        else
            docker compose {{compose_flags}} $ENV_FILE_ARG up --pull always $DETACHED_FLAG {{services}}
        fi
        if [ "{{detached}}" != "true" ]; then
            echo "Docker Compose services have been started."
        fi
    elif [ "{{action}}" = "down" ]; then
        echo "Stopping Docker Compose services"
        if docker compose {{compose_flags}} --profile miner $ENV_FILE_ARG down; then
            echo "Docker Compose services have been stopped and removed."
        else
            echo "Error: Failed to stop Docker Compose services."
            exit 1
        fi
    elif [ "{{action}}" = "clean" ]; then
        echo "WARNING: This will stop Docker Compose services and remove all volumes (including data)."
        echo "If you have not run boundless povw prepare, you will lose the work you have done."
        read -p "Are you sure you want to continue? (y/N): " -r
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            echo "Clean operation cancelled."
            exit 0
        fi
        echo "Stopping and cleaning Docker Compose services"
        if docker compose {{compose_flags}} --profile miner $ENV_FILE_ARG down -v; then
            echo "Docker Compose services have been stopped and volumes have been removed."
        else
            echo "Error: Failed to clean Docker Compose services."
            exit 1
        fi
    elif [ "{{action}}" = "logs" ]; then
        echo "Docker logs"
        docker compose {{compose_flags}} $ENV_FILE_ARG logs -f
    else
        echo "Unknown action: {{action}}"
        echo "Available actions: up, down, clean, logs"
        exit 1
    fi

# Run all components of a boundless prover (bento, broker, miner)
# Set BOUNDLESS_MINING=false to disable mining (e.g., BOUNDLESS_MINING=false just prover)
prover action="up" env_file="" detached="true":
    #!/usr/bin/env bash
    BOUNDLESS_MINING="${BOUNDLESS_MINING:-true}"

    # Check if broker.toml exists, if not create it from template
    if [ ! -f broker.toml ]; then
        echo "Creating broker.toml from template..."
        cp broker-template.toml broker.toml || { echo "Error: broker-template.toml not found"; exit 1; }
        echo "broker.toml created successfully."
    fi

    if [ "{{action}}" = "logs" ]; then
        # Ignore mining process logs by default
        PROFILE_FLAGS="--profile broker"
    elif [ "{{action}}" = "down" ] || [ "{{action}}" = "clean" ]; then
        # Always include miner profile when shutting down to ensure no lingering mining process
        PROFILE_FLAGS="--profile broker --profile miner"
    elif [ "$BOUNDLESS_MINING" = "false" ]; then
        PROFILE_FLAGS="--profile broker"
    else
        PROFILE_FLAGS="--profile broker --profile miner"
    fi

    just bento "{{action}}" "{{env_file}}" "$PROFILE_FLAGS" "{{detached}}"

# Deprecated: Use 'just prover' instead
broker action="up" env_file="" detached="true":
    #!/usr/bin/env bash
    echo "Warning: 'just broker' is deprecated. Use 'just prover' instead." >&2
    just prover "{{action}}" "{{env_file}}" "{{detached}}"
    echo "Warning: 'just broker' is deprecated. Use 'just prover' instead." >&2

# Run the setup script
bento-setup:
    #!/usr/bin/env bash
    ./scripts/setup.sh

# Check job status in Postgres
job-status job_id:
    #!/usr/bin/env bash
    ./scripts/job_status.sh {{job_id}}
