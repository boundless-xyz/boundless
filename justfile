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

# Run broker + boundless-market tests
test-broker:
    forge build
    RISC0_DEV_MODE=1 cargo nextest run -p broker --lib --bin broker
    RISC0_DEV_MODE=1 RISC0_SKIP_BUILD=1 cargo nextest run -p boundless-market --lib -E 'test(prover_utils::config::tests)'

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
    COMPOSE="docker compose -f dockerfiles/compose.localnet.yml --profile order-stream"
    DEV_MODE="${RISC0_DEV_MODE:-1}"

    # The dev-broker container mounts broker.localnet.toml (committed), which
    # is tuned for fast localnet runs and independent of the user's broker.toml.

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
    cd crates/guest/assessor/assessor-guest && cargo fetch
    cd crates/guest/util/echo && cargo fetch
    cd crates/guest/util/identity && cargo fetch
    cd crates/guest/util/loop && cargo fetch
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
# Set PROVER_STACK=experimental to use the new prover stack (prover-compose.yml)
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

    # Select compose file based on PROVER_STACK
    COMPOSE_FILE_FLAG=""
    if [ "${PROVER_STACK:-}" = "experimental" ]; then
        COMPOSE_FILE_FLAG="-f prover-compose.yml"
    fi
    # Bind-mount host-provided BLAKE3 Groth16 setup artifacts only when the user
    # explicitly opts in via BLAKE3_GROTH16_SETUP_DIR. Default uses the artifacts
    # baked into the prebuilt agent image at /.blake3_groth16_artifacts/.
    if [ -n "${BLAKE3_GROTH16_SETUP_DIR:-}" ]; then
        COMPOSE_FILE_FLAG="$COMPOSE_FILE_FLAG -f compose.blake3-local.yml"
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
            if [ "${PROVER_STACK:-}" = "experimental" ]; then
                export PROVER_AGENT_IMAGE="" PROVER_CPU_AGENT_IMAGE="" BROKER_IMAGE="" PROVER_REST_API_IMAGE="" PROVER_CLI_IMAGE=""
                export PROVER_CPU_AGENT_DOCKERFILE="dockerfiles/prover/agent.cpu.dockerfile"
                export PROVER_AGENT_DOCKERFILE="dockerfiles/prover/agent.dockerfile"
                export BROKER_DOCKERFILE="dockerfiles/broker.dockerfile"
                export PROVER_REST_API_DOCKERFILE="dockerfiles/prover/rest_api.dockerfile"
                export PROVER_CLI_DOCKERFILE="dockerfiles/prover/prover_cli.dockerfile"
            else
                export AGENT_IMAGE="" CPU_AGENT_IMAGE="" BROKER_IMAGE="" REST_API_IMAGE="" BENTO_CLI_IMAGE=""
                export CPU_AGENT_DOCKERFILE="dockerfiles/agent.cpu.dockerfile"
                export GPU_AGENT_DOCKERFILE="dockerfiles/agent.dockerfile"
                export BROKER_DOCKERFILE="dockerfiles/broker.dockerfile"
                export REST_API_DOCKERFILE="dockerfiles/rest_api.dockerfile"
                export BENTO_CLI_DOCKERFILE="dockerfiles/bento_cli.dockerfile"
            fi
            docker compose $COMPOSE_FILE_FLAG {{compose_flags}} $ENV_FILE_ARG up --build $DETACHED_FLAG {{services}}
        elif [ -n "$BOUNDLESS_BUILD" ]; then
            for svc in $BOUNDLESS_BUILD; do
                if [ "${PROVER_STACK:-}" = "experimental" ]; then
                    case "$svc" in
                        exec_agent|aux_agent) export PROVER_CPU_AGENT_IMAGE="" PROVER_CPU_AGENT_DOCKERFILE="${PROVER_CPU_AGENT_DOCKERFILE:-dockerfiles/prover/agent.cpu.dockerfile}" ;;
                        gpu_prove_agent) export PROVER_AGENT_IMAGE="" PROVER_AGENT_DOCKERFILE="${PROVER_AGENT_DOCKERFILE:-dockerfiles/prover/agent.dockerfile}" ;;
                        miner) export PROVER_CLI_IMAGE="" PROVER_CLI_DOCKERFILE="${PROVER_CLI_DOCKERFILE:-dockerfiles/prover/prover_cli.dockerfile}" ;;
                        broker)        export BROKER_IMAGE="" BROKER_DOCKERFILE="${BROKER_DOCKERFILE:-dockerfiles/broker.dockerfile}" ;;
                        rest_api)      export PROVER_REST_API_IMAGE="" PROVER_REST_API_DOCKERFILE="${PROVER_REST_API_DOCKERFILE:-dockerfiles/prover/rest_api.dockerfile}" ;;
                        *) echo "WARNING: unrecognized BOUNDLESS_BUILD service '$svc' — will not clear its image tag" ;;
                    esac
                else
                    case "$svc" in
                        exec_agent|aux_agent) export CPU_AGENT_IMAGE="" CPU_AGENT_DOCKERFILE="${CPU_AGENT_DOCKERFILE:-dockerfiles/agent.cpu.dockerfile}" ;;
                        gpu_prove_agent) export AGENT_IMAGE="" GPU_AGENT_DOCKERFILE="${GPU_AGENT_DOCKERFILE:-dockerfiles/agent.dockerfile}" ;;
                        miner) export BENTO_CLI_IMAGE="" BENTO_CLI_DOCKERFILE="${BENTO_CLI_DOCKERFILE:-dockerfiles/bento_cli.dockerfile}" ;;
                        broker)        export BROKER_IMAGE="" BROKER_DOCKERFILE="${BROKER_DOCKERFILE:-dockerfiles/broker.dockerfile}" ;;
                        rest_api)      export REST_API_IMAGE="" REST_API_DOCKERFILE="${REST_API_DOCKERFILE:-dockerfiles/rest_api.dockerfile}" ;;
                        *) echo "WARNING: unrecognized BOUNDLESS_BUILD service '$svc' — will not clear its image tag" ;;
                    esac
                fi
            done
            docker compose $COMPOSE_FILE_FLAG {{compose_flags}} $ENV_FILE_ARG build $BOUNDLESS_BUILD
            docker compose $COMPOSE_FILE_FLAG {{compose_flags}} $ENV_FILE_ARG up --pull always $DETACHED_FLAG {{services}}
        else
            docker compose $COMPOSE_FILE_FLAG {{compose_flags}} $ENV_FILE_ARG up --pull always $DETACHED_FLAG {{services}}
        fi
        if [ "{{detached}}" != "true" ]; then
            echo "Docker Compose services have been started."
        fi
    elif [ "{{action}}" = "down" ]; then
        echo "Stopping Docker Compose services"
        if docker compose $COMPOSE_FILE_FLAG {{compose_flags}} --profile miner $ENV_FILE_ARG down; then
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
        if docker compose $COMPOSE_FILE_FLAG {{compose_flags}} --profile miner $ENV_FILE_ARG down -v; then
            echo "Docker Compose services have been stopped and volumes have been removed."
        else
            echo "Error: Failed to clean Docker Compose services."
            exit 1
        fi
    elif [ "{{action}}" = "logs" ]; then
        echo "Docker logs"
        docker compose $COMPOSE_FILE_FLAG {{compose_flags}} $ENV_FILE_ARG logs -f
    else
        echo "Unknown action: {{action}}"
        echo "Available actions: up, down, clean, logs"
        exit 1
    fi

# Run all components of a boundless prover (bento, broker, miner)
# Set BOUNDLESS_MINING=false to disable mining (e.g., BOUNDLESS_MINING=false just prover)
# Set PROVER_STACK=experimental to use the new prover stack (redis-only, no postgres/minio)
prover action="up" env_file="" detached="true":
    #!/usr/bin/env bash
    BOUNDLESS_MINING="${BOUNDLESS_MINING:-true}"

    # Check if broker.toml exists, if not create it from template
    if [ ! -f broker.toml ]; then
        echo "Creating broker.toml from template..."
        cp broker-template.toml broker.toml || { echo "Error: broker-template.toml not found"; exit 1; }
        echo "broker.toml created successfully."
    fi
    # Ensure chain-overrides/ exists for compose bind mount
    mkdir -p chain-overrides

    if [ "{{action}}" = "logs" ]; then
        PROFILE_FLAGS="--profile broker"
    elif [ "{{action}}" = "down" ] || [ "{{action}}" = "clean" ]; then
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

# Run the end-to-end release smoke test: localnet up + submit_echo (on-chain and off-chain) + teardown.
# Intended to be run before cutting a release, and from CI via .github/workflows/release-e2e.yml.
test-e2e-release:
    #!/usr/bin/env bash
    set -uo pipefail
    mkdir -p target/e2e-logs
    rm -f target/e2e-logs/*.log
    # Guarantee teardown on any exit path.
    trap 'just _e2e-teardown "$?" || true' EXIT
    # Ensure a clean slate so each run gets a fresh anvil + redeployed contracts.
    just localnet down || true
    just localnet up
    # Source the deployer-written contract addresses (and RISC0_DEV_MODE,
    # which localnet-deploy.sh patches in before writing .env.localnet).
    set -a
    source .env.localnet
    set +a

    # Non-dev mode: bring up our own bento+broker cluster, but refuse if one is
    # already running so we don't clobber the user's stack.
    if [ "${RISC0_DEV_MODE:-1}" != "1" ]; then
        n=$(docker compose ps --services --filter status=running 2>/dev/null | grep -cE '^(rest_api|broker)$')
        [ "$n" = 0 ] || { echo "[test-e2e-release] bento/broker already running; stop them first" >&2; exit 1; }
        trap 'rc=$?; just prover down || true; just _e2e-teardown "$rc" || true' EXIT
        BOUNDLESS_MINING=false just prover
    fi

    # Launch all examples in parallel. Each runs in a separate bash job with its own
    # private key and log file. pids maps PID -> example name.
    declare -A pids=()

    just _e2e-run-submit-echo onchain target/e2e-logs/submit-echo-onchain.log &
    pids[$!]=submit-echo-onchain
    just _e2e-run-submit-echo offchain target/e2e-logs/submit-echo-offchain.log &
    pids[$!]=submit-echo-offchain
    just _e2e-run-counter {{E2E_KEY_COUNTER}} target/e2e-logs/counter.log &
    pids[$!]=counter
    just _e2e-run-counter-with-callback {{E2E_KEY_COUNTER_CALLBACK}} target/e2e-logs/counter-with-callback.log &
    pids[$!]=counter-with-callback
    just _e2e-run-composition {{E2E_KEY_COMPOSITION}} target/e2e-logs/composition.log &
    pids[$!]=composition
    just _e2e-run-smart-contract-requestor {{E2E_KEY_SCR}} target/e2e-logs/smart-contract-requestor.log &
    pids[$!]=smart-contract-requestor
    just _e2e-run-blake3-groth16 {{E2E_KEY_BLAKE3}} target/e2e-logs/blake3-groth16.log &
    pids[$!]=blake3-groth16
    just _e2e-run-echo-unpinned {{E2E_KEY_UNPINNED}} target/e2e-logs/echo-unpinned.log &
    pids[$!]=echo-unpinned

    echo "[test-e2e-release] launched ${#pids[@]} parallel examples"

    failures=()
    for pid in "${!pids[@]}"; do
        name="${pids[$pid]}"
        if wait "$pid"; then
            echo "[test-e2e-release] PASS: $name"
        else
            rc=$?
            echo "[test-e2e-release] FAIL ($rc): $name"
            failures+=("$name")
        fi
    done

    if [ "${#failures[@]}" -gt 0 ]; then
        for name in "${failures[@]}"; do
            echo "::group::${name} log"
            cat "target/e2e-logs/${name}.log" 2>&1 || true
            echo "::endgroup::"
        done
        echo "[test-e2e-release] ${#failures[@]} example(s) failed: ${failures[*]}"
        exit 1
    fi
    echo "[test-e2e-release] all examples passed"

# Private: on failure, dump docker logs to stdout (visible inline in CI),
# then tear down localnet regardless of state. Safe to call even if no containers
# are running.
_e2e-teardown status:
    #!/usr/bin/env bash
    set +e
    if [ "{{status}}" != "0" ]; then
        for svc in anvil deployer order-stream broker; do
            echo "${svc} logs:"
            docker compose -f dockerfiles/compose.localnet.yml \
                --profile order-stream --profile dev-broker logs "$svc" \
                2>&1 || true
            echo "::end::"
        done
    fi
    just localnet down || true

# Private: run the submit_echo example in either on-chain or off-chain mode.
# Arg `mode` must be "onchain" or "offchain".
# Per-example wall-clock timeouts (seconds). Dev mode uses fake proofs; real
# proving via Bento is dramatically slower (Groth16 + recursion), so we scale up.
E2E_SUBMIT_ECHO_TIMEOUT := if env_var_or_default("RISC0_DEV_MODE", "1") == "1" { "180" } else { "1800" }
E2E_EXAMPLE_TIMEOUT := if env_var_or_default("RISC0_DEV_MODE", "1") == "1" { "600" } else { "3600" }
# Anvil default-mnemonic keys. Slot 0 = deployer, slot 3 = prover — both reserved.
# See docs/superpowers/plans/2026-04-23-release-e2e-phase-2-3.md for the full allocation.
E2E_KEY_SUBMIT_ECHO_ONCHAIN := "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
E2E_KEY_SUBMIT_ECHO_OFFCHAIN := "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
E2E_KEY_COUNTER := "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a"
E2E_KEY_COUNTER_CALLBACK := "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"
E2E_KEY_COMPOSITION := "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e"
E2E_KEY_SCR := "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356"
E2E_KEY_BLAKE3 := "0xdbda1821b80551c9d65939329250298aa3472ba22feea921c0cf5d620ea67b97"
E2E_KEY_UNPINNED := "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6"
_e2e-run-submit-echo mode log_path:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "[test-e2e-release] submit_echo ({{mode}}) -> {{log_path}}"
    case "{{mode}}" in
        onchain)
            timeout --foreground {{E2E_SUBMIT_ECHO_TIMEOUT}} \
                env -u ORDER_STREAM_URL REQUESTOR_KEY={{E2E_KEY_SUBMIT_ECHO_ONCHAIN}} \
                cargo run --quiet --example submit_echo -p boundless-market \
                > "{{log_path}}" 2>&1
            rc=$?
            ;;
        offchain)
            timeout --foreground {{E2E_SUBMIT_ECHO_TIMEOUT}} \
                env REQUESTOR_KEY={{E2E_KEY_SUBMIT_ECHO_OFFCHAIN}} \
                cargo run --quiet --example submit_echo -p boundless-market \
                > "{{log_path}}" 2>&1
            rc=$?
            ;;
        *)
            echo "unknown mode: {{mode}}" >&2
            exit 2
            ;;
    esac
    if [ "$rc" = "124" ]; then
        echo "[test-e2e-release] submit_echo ({{mode}}) timed out after {{E2E_SUBMIT_ECHO_TIMEOUT}}s" >&2
    fi
    exit "$rc"

# Private: run the blake3-groth16 example. Takes a private key (for request submission)
# and a log path. Redirects all cargo stdout/stderr to the log.
_e2e-run-blake3-groth16 private_key log_path:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "[test-e2e-release] blake3-groth16 -> {{log_path}}"
    timeout --foreground {{E2E_EXAMPLE_TIMEOUT}} \
        env PRIVATE_KEY={{private_key}} \
        cargo run --quiet --release \
            --manifest-path examples/blake3-groth16/Cargo.toml \
            -p example-blake3-groth16 \
        > "{{log_path}}" 2>&1
    rc=$?
    if [ "$rc" = "124" ]; then
        echo "[test-e2e-release] blake3-groth16 timed out after {{E2E_EXAMPLE_TIMEOUT}}s" >&2
    fi
    exit "$rc"

_e2e-deploy-via-forge example_dir contract_name log_path private_key extra_env="":
    #!/usr/bin/env bash
    set -euo pipefail
    {
        echo "=== deploy {{contract_name}} ({{example_dir}}) ==="
        (cd {{example_dir}} && \
            env PRIVATE_KEY="{{private_key}}" {{extra_env}} \
            forge script contracts/scripts/Deploy.s.sol --rpc-url "$RPC_URL" --broadcast -vv) \
            || { echo "forge deploy failed"; exit 1; }
    } >> "{{log_path}}" 2>&1
    jq -re \
        '.transactions[] | select(.contractName == "{{contract_name}}") | .contractAddress' \
        "{{example_dir}}/broadcast/Deploy.s.sol/31337/run-latest.json"

# Private: run the counter example. Deploys its own Counter.sol instance using the passed key.
_e2e-run-counter private_key log_path:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "[test-e2e-release] counter -> {{log_path}}"
    log="{{log_path}}"
    key="{{private_key}}"
    : > "$log"
    counter_addr=$(just _e2e-deploy-via-forge examples/counter Counter "$log" "$key" "VERIFIER_ADDRESS=$VERIFIER_ADDRESS") \
        || { echo "[test-e2e-release] counter: deploy failed; see $log" >&2; exit 1; }
    {
        echo "Counter deployed at: $counter_addr"
        echo "=== run counter example ==="
        timeout --foreground {{E2E_EXAMPLE_TIMEOUT}} \
            env PRIVATE_KEY="$key" COUNTER_ADDRESS="$counter_addr" \
            cargo run --quiet --release \
                --manifest-path examples/counter/Cargo.toml \
                -p example-counter
    } >> "$log" 2>&1
    rc=$?
    if [ "$rc" = "124" ]; then
        echo "[test-e2e-release] counter timed out after {{E2E_EXAMPLE_TIMEOUT}}s" >&2
    fi
    exit "$rc"

# Private: run the composition example. Deploys its own Counter.sol instance.
_e2e-run-composition private_key log_path:
    #!/usr/bin/env bash
    set -uo pipefail
    # The IDENTITY guest enables risc0-zkvm/disable-dev-mode (see
    # crates/guest/util/identity/Cargo.toml), so it rejects FakeReceipts even
    # when RISC0_DEV_MODE=1. Composition is exercised in nightly-examples.yml
    # with real Groth16 proving.
    if [ "${RISC0_DEV_MODE:-}" = "1" ]; then
        echo "[test-e2e-release] composition: SKIPPED (incompatible with RISC0_DEV_MODE=1)"
        echo "SKIPPED: composition requires real proofs (identity guest sets risc0-zkvm/disable-dev-mode)" > "{{log_path}}"
        exit 0
    fi
    echo "[test-e2e-release] composition -> {{log_path}}"
    log="{{log_path}}"
    key="{{private_key}}"
    : > "$log"
    counter_addr=$(just _e2e-deploy-via-forge examples/composition Counter "$log" "$key" "VERIFIER_ADDRESS=$VERIFIER_ADDRESS") \
        || { echo "[test-e2e-release] composition: deploy failed; see $log" >&2; exit 1; }
    {
        echo "Counter deployed at: $counter_addr"
        echo "=== run composition example ==="
        timeout --foreground {{E2E_EXAMPLE_TIMEOUT}} \
            env PRIVATE_KEY="$key" COUNTER_ADDRESS="$counter_addr" \
            cargo run --quiet --release \
                --manifest-path examples/composition/Cargo.toml \
                -p example-composition
    } >> "$log" 2>&1
    rc=$?
    if [ "$rc" = "124" ]; then
        echo "[test-e2e-release] composition timed out after {{E2E_EXAMPLE_TIMEOUT}}s" >&2
    fi
    exit "$rc"

# Private: run the counter-with-callback example. Deploys its own callback-variant Counter.sol.
_e2e-run-counter-with-callback private_key log_path:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "[test-e2e-release] counter-with-callback -> {{log_path}}"
    log="{{log_path}}"
    key="{{private_key}}"
    : > "$log"
    counter_addr=$(just _e2e-deploy-via-forge examples/counter-with-callback Counter "$log" "$key" "VERIFIER_ADDRESS=$VERIFIER_ADDRESS BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS") \
        || { echo "[test-e2e-release] counter-with-callback: deploy failed; see $log" >&2; exit 1; }
    {
        echo "Counter deployed at: $counter_addr"
        echo "=== run counter-with-callback example ==="
        timeout --foreground {{E2E_EXAMPLE_TIMEOUT}} \
            env PRIVATE_KEY="$key" COUNTER_ADDRESS="$counter_addr" \
            cargo run --quiet --release \
                --manifest-path examples/counter-with-callback/Cargo.toml \
                -p example-counter-with-callback
    } >> "$log" 2>&1
    rc=$?
    if [ "$rc" = "124" ]; then
        echo "[test-e2e-release] counter-with-callback timed out after {{E2E_EXAMPLE_TIMEOUT}}s" >&2
    fi
    exit "$rc"

# Private: run the smart-contract-requestor example. Deploys its own SCR contract.
_e2e-run-smart-contract-requestor private_key log_path:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "[test-e2e-release] smart-contract-requestor -> {{log_path}}"
    log="{{log_path}}"
    key="{{private_key}}"
    : > "$log"
    scr_addr=$(just _e2e-deploy-via-forge examples/smart-contract-requestor SmartContractRequestor "$log" "$key" "BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS") \
        || { echo "[test-e2e-release] smart-contract-requestor: deploy failed; see $log" >&2; exit 1; }
    {
        echo "SmartContractRequestor deployed at: $scr_addr"
        echo "=== fund SmartContractRequestor via execute(deposit) ==="
        # Send ETH to the SCR contract, then have it call BoundlessMarket.deposit() with that ETH.
        # deposit() selector is 0xd0e30db0; value 0.01 ETH is sufficient for a single ECHO request.
        cast send --private-key "$key" --rpc-url "$RPC_URL" \
            --value 0.01ether "$scr_addr" \
            || { echo "send ETH to SCR failed"; exit 1; }
        cast send --private-key "$key" --rpc-url "$RPC_URL" \
            "$scr_addr" "execute(address,bytes,uint256)" \
            "$BOUNDLESS_MARKET_ADDRESS" "0xd0e30db0" "10000000000000000" \
            || { echo "SCR deposit to market failed"; exit 1; }
        echo "=== run smart-contract-requestor example ==="
        timeout --foreground {{E2E_EXAMPLE_TIMEOUT}} \
            env PRIVATE_KEY="$key" SMART_CONTRACT_REQUESTOR_ADDRESS="$scr_addr" \
            cargo run --quiet --release \
                --manifest-path examples/smart-contract-requestor/Cargo.toml \
                -p example-smart-contract-requestor
    } >> "$log" 2>&1
    rc=$?
    if [ "$rc" = "124" ]; then
        echo "[test-e2e-release] smart-contract-requestor timed out after {{E2E_EXAMPLE_TIMEOUT}}s" >&2
    fi
    exit "$rc"

# Private: run the out-of-workspace echo-unpinned example. Regenerates Cargo.lock each run
# to exercise fresh transitive dependency resolution.
_e2e-run-echo-unpinned private_key log_path:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "[test-e2e-release] echo-unpinned -> {{log_path}}"
    log="{{log_path}}"
    key="{{private_key}}"
    : > "$log"
    {
        echo "=== cargo generate-lockfile (fresh resolution) ==="
        (cd examples/echo-unpinned && cargo generate-lockfile) \
            || { echo "generate-lockfile failed"; exit 1; }
        echo "=== cargo update (latest compatible deps) ==="
        (cd examples/echo-unpinned && cargo update) \
            || { echo "cargo update failed"; exit 1; }
        echo "=== run echo-unpinned ==="
        timeout --foreground {{E2E_EXAMPLE_TIMEOUT}} \
            env -u ORDER_STREAM_URL REQUESTOR_KEY="$key" \
            cargo run --quiet --release --manifest-path examples/echo-unpinned/Cargo.toml
    } >> "$log" 2>&1
    rc=$?
    if [ "$rc" = "124" ]; then
        echo "[test-e2e-release] echo-unpinned timed out after {{E2E_EXAMPLE_TIMEOUT}}s" >&2
    fi
    exit "$rc"
