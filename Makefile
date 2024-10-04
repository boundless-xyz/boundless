.PHONY: devnet-up devnet-down check-deps

# Check that required dependencies are installed
check-deps:
	@command -v forge >/dev/null 2>&1 || { echo "Error: forge is not installed. Please install it from https://book.getfoundry.sh/getting-started/installation"; exit 1; }
	@command -v cargo >/dev/null 2>&1 || { echo "Error: cargo is not installed. Please install it from https://rustup.rs"; exit 1; }
	@command -v anvil >/dev/null 2>&1 || { echo "Error: anvil is not installed. Please install it from https://book.getfoundry.sh/getting-started/installation"; exit 1; }
	@command -v jq >/dev/null 2>&1 || { echo "Error: jq is not installed. Please install it from https://stedolan.github.io/jq/"; exit 1; }

devnet-up: check-deps
	@mkdir -p logs
	@echo "Building contracts..."
	@forge build || { echo "Failed to build contracts"; $(MAKE) devnet-down; exit 1; }
	@echo "Building Rust project..."
	@cargo build --bin broker || { echo "Failed to build broker binary"; $(MAKE) devnet-down; exit 1; }
	@echo "Starting Anvil..."
	@-anvil -b 2 &> logs/anvil.txt & \
	PID_ANVIL=$$!; \
	@sleep 5 # Give Anvil some time to start
	@echo "Deploying contracts..."
	@RISC0_DEV_MODE=1 forge script contracts/scripts/Deploy.s.sol --rpc-url http://localhost:8545 --broadcast -vv || { echo "Failed to deploy contracts"; $(MAKE) devnet-down; exit 1; }
	@echo "Fetching contract addresses..."
	@{ \
	  SET_VERIFIER_ADDRESS=$$(jq -re '.transactions[] | select(.contractName == "RiscZeroSetVerifier") | .contractAddress' ./broadcast/Deploy.s.sol/31337/run-latest.json); \
	  PROOF_MARKET_ADDRESS=$$(jq -re '.transactions[] | select(.contractName == "ProofMarket") | .contractAddress' ./broadcast/Deploy.s.sol/31337/run-latest.json); \
	  echo "Contract deployed at addresses:"; \
	  echo "SET_VERIFIER_ADDRESS=$$SET_VERIFIER_ADDRESS"; \
	  echo "PROOF_MARKET_ADDRESS=$$PROOF_MARKET_ADDRESS"; \
	  echo "Updating .env file..."; \
	  sed -i "" "s/^SET_VERIFIER_ADDRESS=.*/SET_VERIFIER_ADDRESS=$$SET_VERIFIER_ADDRESS/" .env; \
	  sed -i "" "s/^PROOF_MARKET_ADDRESS=.*/PROOF_MARKET_ADDRESS=$$PROOF_MARKET_ADDRESS/" .env; \
	  echo ".env file updated successfully."; \
	  echo "Starting Broker service..."; \
	  RISC0_DEV_MODE=1 RUST_LOG=info,broker=debug,boundless_market=debug ./target/debug/broker \
	  --priv-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
	  --proof-market-addr $$PROOF_MARKET_ADDRESS \
	  --set-verifier-addr $$SET_VERIFIER_ADDRESS \
	  --deposit-amount 10 > logs/broker.txt & \
	  echo $$! &> logs/broker.pid; \
	} || { echo "Failed to fetch addresses or start broker"; $(MAKE) devnet-down; exit 1; }
	@echo "Devnet is up and running!"
	@echo "Make sure to run 'source .env' to load the environment variables."

devnet-down:
	@echo "Bringing down all services..."
	@if [ -f logs/broker.pid ]; then kill $$(cat logs/broker.pid) && rm logs/broker.pid; fi
	-pkill anvil || true
	@echo "Devnet stopped."
