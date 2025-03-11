.POSIX:
.SILENT:

# Variables
ANVIL_PORT = 8545
ANVIL_BLOCK_TIME = 2
RISC0_DEV_MODE = 1
CHAIN_KEY = "anvil"
RUST_LOG = info,broker=debug,boundless_market=debug
DEPLOYER_PRIVATE_KEY = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
PRIVATE_KEY = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
ADMIN_ADDRESS = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
DEPOSIT_AMOUNT = 100000000000000000000
DEFAULT_DATABASE_URL = postgres://postgres:password@localhost:5432/postgres
DATABASE_URL ?= $(DEFAULT_DATABASE_URL)

LOGS_DIR = logs
PID_FILE = $(LOGS_DIR)/devnet.pid

help:
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[.a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

# Check that required dependencies are installed
check-deps:
	for cmd in forge cargo anvil jq; do \
		command -v $$cmd >/dev/null 2>&1 || { echo "Error: $$cmd is not installed."; exit 1; }; \
	done

##@ Development
.PHONY: ci test check format format-check cargo-clippy license-check link-check broker-docker clean

ci: check test ## Run all CI checks

test: foundry-test cargo-test ## Run all tests

check: link-check format-check license-check cargo-clippy ## Run all code quality checks

format: ## Format all code
	echo "Formatting code..."
	cargo sort --workspace
	cargo fmt --all
	cd examples/counter && cargo sort --workspace
	cd examples/counter && cargo fmt --all
	cd bento && cargo sort --workspace
	cd bento && cargo fmt --all
	cd documentation && bun run format-markdown
	dprint fmt
	forge fmt

format-check: ## Check code formatting
	echo "Checking code formatting..."
	cargo sort --workspace --check
	cargo fmt --all --check
	cd examples/counter && cargo sort --workspace --check
	cd examples/counter && cargo fmt --all --check
	cd bento && cargo sort --workspace --check
	cd bento && cargo fmt --all --check
	cd documentation && bun run check
	dprint check
	forge fmt --check

cargo-clippy: ## Run Cargo clippy
	echo "Running Cargo clippy..."
	RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNEL=1 \
	cargo clippy --workspace --all-targets
	RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNEL=1 \
	cd examples/counter && cargo clippy --workspace --all-targets
	RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNEL=1 \
	cd bento && cargo clippy --workspace --all-targets

link-check: ## Check links in markdown files
	echo "Checking links in markdown files..."
	git ls-files '*.md' ':!:documentation/*' | xargs lychee --base . --cache --

license-check: ## Check licenses
	echo "Checking licenses..."
	python license-check.py

broker-docker: ## Build Broker Docker containers
	echo "Building Docker containers..."
	docker compose --profile broker --env-file ./.env-compose config
	docker compose --profile broker --env-file ./.env-compose -f compose.yml -f ./dockerfiles/compose.ci.yml build

clean: devnet-down ## Clean up all build artifacts
	echo "Cleaning up..."
	rm -rf $(LOGS_DIR) ./broadcast
	cargo clean
	forge clean
	echo "Cleanup complete."

##@ Devnet
.PHONY: devnet-up devnet-down

devnet-up: check-deps ## Start the development network
	mkdir -p $(LOGS_DIR)
	echo "Building contracts..."
	forge build || { echo "Failed to build contracts"; $(MAKE) devnet-down; exit 1; }
	echo "Building Rust project..."
	cargo build --bin broker || { echo "Failed to build broker binary"; $(MAKE) devnet-down; exit 1; }
	# Check if Anvil is already running
	if nc -z localhost $(ANVIL_PORT); then \
		echo "Anvil is already running on port $(ANVIL_PORT). Reusing existing instance."; \
	else \
		echo "Starting Anvil..."; \
		anvil -b $(ANVIL_BLOCK_TIME) > $(LOGS_DIR)/anvil.txt 2>&1 & echo $$! >> $(PID_FILE); \
		sleep 5; \
	fi
	echo "Deploying contracts..."
	DEPLOYER_PRIVATE_KEY=$(DEPLOYER_PRIVATE_KEY) CHAIN_KEY=${CHAIN_KEY} RISC0_DEV_MODE=$(RISC0_DEV_MODE) BOUNDLESS_MARKET_OWNER=$(ADMIN_ADDRESS) forge script contracts/scripts/Deploy.s.sol --rpc-url http://localhost:$(ANVIL_PORT) --broadcast -vv || { echo "Failed to deploy contracts"; $(MAKE) devnet-down; exit 1; }
	echo "Fetching contract addresses..."
	{ \
		SET_VERIFIER_ADDRESS=$$(jq -re '.transactions[] | select(.contractName == "RiscZeroSetVerifier") | .contractAddress' ./broadcast/Deploy.s.sol/31337/run-latest.json); \
		BOUNDLESS_MARKET_ADDRESS=$$(jq -re '.transactions[] | select(.contractName == "ERC1967Proxy") | .contractAddress' ./broadcast/Deploy.s.sol/31337/run-latest.json); \
		HIT_POINTS_ADDRESS=$$(jq -re '.transactions[] | select(.contractName == "HitPoints") | .contractAddress' ./broadcast/Deploy.s.sol/31337/run-latest.json | head -n 1); \
		echo "Contract deployed at addresses:"; \
		echo "SET_VERIFIER_ADDRESS=$$SET_VERIFIER_ADDRESS"; \
		echo "BOUNDLESS_MARKET_ADDRESS=$$BOUNDLESS_MARKET_ADDRESS"; \
		echo "HIT_POINTS_ADDRESS=$$HIT_POINTS_ADDRESS"; \
		echo "Updating .env file..."; \
		sed -i.bak "s/^SET_VERIFIER_ADDRESS=.*/SET_VERIFIER_ADDRESS=$$SET_VERIFIER_ADDRESS/" .env && \
		sed -i.bak "s/^BOUNDLESS_MARKET_ADDRESS=.*/BOUNDLESS_MARKET_ADDRESS=$$BOUNDLESS_MARKET_ADDRESS/" .env && \
		rm .env.bak; \
		echo ".env file updated successfully."; \
		echo "Minting HP for prover address."; \
        cast send --private-key $(DEPLOYER_PRIVATE_KEY) \
            --rpc-url http://localhost:$(ANVIL_PORT) \
            $$HIT_POINTS_ADDRESS "mint(address, uint256)" $(ADMIN_ADDRESS) $(DEPOSIT_AMOUNT); \
		echo "Starting Broker service..."; \
		RISC0_DEV_MODE=$(RISC0_DEV_MODE) RUST_LOG=$(RUST_LOG) ./target/debug/broker \
			--private-key $(PRIVATE_KEY) \
			--boundless-market-addr $$BOUNDLESS_MARKET_ADDRESS \
			--set-verifier-addr $$SET_VERIFIER_ADDRESS \
			--deposit-amount $(DEPOSIT_AMOUNT) > $(LOGS_DIR)/broker.txt 2>&1 & echo $$! >> $(PID_FILE); \
	} || { echo "Failed to fetch addresses or start broker"; $(MAKE) devnet-down; exit 1; }
	echo "Devnet is up and running!"
	echo "Make sure to run 'source .env' to load the environment variables."

devnet-down: ## Stop the development network
	echo "Bringing down all services..."
	if [ -f $(PID_FILE) ]; then \
		while read pid; do \
			kill $$pid 2>/dev/null || true; \
		done < $(PID_FILE); \
		rm $(PID_FILE); \
	fi
	echo "Devnet stopped."

##@ Testing
.PHONY: foundry-test cargo-test cargo-test-root cargo-test-example-counter cargo-test-bento cargo-test-db setup-db clean-db

foundry-test: ## Run Foundry tests
	echo "Running Foundry tests..."
	forge clean # Required by OpenZeppelin upgrades plugin
	forge test -vvv --isolate

cargo-test: cargo-test-root cargo-test-example-counter cargo-test-bento cargo-test-db ## Run all Cargo tests

cargo-test-root: ## Run Cargo tests for root workspace
	echo "Running Cargo tests for root workspace..."
	RISC0_DEV_MODE=1 cargo test --workspace --exclude order-stream

cargo-test-bento: ## Run Cargo tests for bento
	echo "Running Cargo tests for bento..."
	cd bento && RISC0_DEV_MODE=1 cargo test --workspace --exclude taskdb --exclude order-stream

cargo-test-example-counter: ## Run Cargo tests for counter example
	echo "Running Cargo tests for counter example..."
	cd examples/counter && \
	forge build && \
	RISC0_DEV_MODE=1 cargo test

cargo-test-db: setup-db ## Run database tests
	echo "Running database tests..."
	DATABASE_URL=$(DATABASE_URL) sqlx migrate run --source ./bento/crates/taskdb/migrations/
	cd bento && DATABASE_URL=$(DATABASE_URL) RISC0_DEV_MODE=1 cargo test -p taskdb
	DATABASE_URL=$(DATABASE_URL) RISC0_DEV_MODE=1 cargo test -p order-stream
	$(MAKE) clean-db

setup-db: ## Set up test database
	echo "Setting up test database..."
	docker inspect postgres-test > /dev/null || \
	docker run -d \
		--name postgres-test \
		-e POSTGRES_PASSWORD=password \
		-p 5432:5432 \
		postgres:latest
	# Wait for PostgreSQL to be ready
	sleep 3

clean-db: ## Clean up test database
	echo "Cleaning up test database..."
	docker stop postgres-test
	docker rm postgres-test
