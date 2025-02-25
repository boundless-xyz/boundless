default:
    @just --list

ci: check test

test: foundry-test cargo-test

check: link-check format-check license-check cargo-clippy

foundry-test:
    forge clean # Required by OpenZeppelin upgrades plugin
    forge test -vvv --isolate

cargo-test: cargo-test-root cargo-test-example-counter

cargo-test-root:
    RISC0_DEV_MODE=1 cargo test --workspace --exclude order-stream

cargo-test-bento:
    cd bento && RISC0_DEV_MODE=1 cargo test --workspace --exclude taskdb --exclude order-stream

cargo-test-example-counter:
    cd examples/counter && \
    forge build && \
    RISC0_DEV_MODE=1 cargo test

cargo-test-db $DATABASE_URL=DEFAULT_DATABASE_URL: setup-db
    sqlx migrate run --source ./bento/crates/taskdb/migrations/
    cd bento && RISC0_DEV_MODE=1 cargo test -p taskdb
    RISC0_DEV_MODE=1 cargo test -p order-stream
    just clean-db

cargo-clippy:
    RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNEL=1 \
    cargo clippy --workspace --all-targets
    RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNEL=1 \
    cd examples/counter cargo clippy --workspace --all-targets
    RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNEL=1 \
    cd bento cargo clippy --workspace --all-targets

cargo-update:
    cargo update
    cd examples/counter cargo update

link-check:
    git ls-files '*.md' ':!:documentation/*' | xargs lychee --base . --cache --

license-check:
    python license-check.py

format:
    cargo sort --workspace
    cargo fmt --all
    cd examples/counter && cargo sort --workspace
    cd examples/counter && cargo fmt --all
    cd bento && cargo sort --workspace
    cd bento && cargo fmt --all
    cd documentation && bun run format-markdown
    dprint fmt
    forge fmt

format-check:
    cargo sort --workspace --check
    cargo fmt --all --check
    cd examples/counter && cargo sort --workspace --check
    cd examples/counter && cargo fmt --all --check
    cd bento && cargo sort --workspace --check
    cd bento && cargo fmt --all --check
    cd documentation && bun run check
    dprint check
    forge fmt --check

docker:
    docker compose --profile broker --env-file ./.env-compose config
    docker compose --profile broker --env-file ./.env-compose -f compose.yml -f ./dockerfiles/compose.ci.yml build

# Set up local Postgres database for testing
setup-db:
    docker inspect postgres-test > /dev/null || \
    docker run -d \
        --name postgres-test \
        -e POSTGRES_PASSWORD=password \
        -p 5432:5432 \
        postgres:latest
    # Wait for PostgreSQL to be ready
    sleep 3

# Clean up local Postgres database
clean-db:
    docker stop postgres-test
    docker rm postgres-test

DEFAULT_DATABASE_URL := "postgres://postgres:password@localhost:5432/postgres"
