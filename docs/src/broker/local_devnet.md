# Playing with a Local Development Environment

Ensure the following software is installed on your machine before proceeding:

- **[Rust](https://www.rust-lang.org/tools/install) version 1.79 or higher**
- **[Foundry](https://book.getfoundry.sh/getting-started/installation) version 0.2 or higher**
- **[Docker](https://docs.docker.com/engine/install/)**
- **Python version 3.10 or higher**
- **jq**

Before starting, ensure you have cloned with recursive submodules, or pull them with:

```console
git submodule update --init
```

1. Build the contracts

   ```console
   forge build
   ```

2. Build the project

   ```console
   cargo build
   ```

3. Start anvil

   ```console
   anvil -b 2
   ```

4. Deploy market contracts

   This will deploy the market contracts.
   Configuration environment variables are read from the [.env](../../../.env) file.
   Optionally, by setting the environment variable `RISC0_DEV_MODE`, a mock verifier will be deployed.

   ```console
   source .env
   RISC0_DEV_MODE=1 forge script contracts/scripts/Deploy.s.sol --rpc-url $RPC_URL --broadcast -vv
   ```

   > NOTE: Starting from a fresh Anvil instance, the deployed contract addresses will match the values in `.env`.
   > If you need to deploy again, restart Anvil first or change the `.env` file to match your newly deployed contract addresses.

5. Deposit Prover funds and start the Broker

   Optionally, by setting the environment variable `RISC0_DEV_MODE`, a mock prover will be used by the broker.

   ```console
   RUST_LOG=info cargo run --bin cli -- --wallet-private-key ${PROVER_PRIVATE_KEY:?} deposit 10
   ```

   Here we will use a mock prover by setting `RISC0_DEV_MODE`.
   The Broker can use either Bonsai or Bento as backend.
   To use Bonsai, export the `BONSAI_API_URL` and `BONSAI_API_KEY` and run without `RISC0_DEV_MODE`.
   To use Bento, refer to the [Running Bento](../bento/running_bento.md) guide.

   ```console
   RISC0_DEV_MODE=1 RUST_LOG=info cargo run --bin broker -- --priv-key ${PROVER_PRIVATE_KEY:?} --proof-market-addr ${PROOF_MARKET_ADDRESS:?} --set-verifier-addr ${SET_VERIFIER_ADDRESS:?}
   ```

6. Test your deployment with the client cli.
   You can read more about the client on the [proving request](../market/proving_request.md) page.

   ```console
   RISC0_DEV_MODE=1 RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-request request.yaml
   ```

Congratulations! You now have a local devnet running and a prover that will respond to proving requests.

Check out the [counter example](../../../examples/counter/README.md) for an example of how to run and application using the prover market.
