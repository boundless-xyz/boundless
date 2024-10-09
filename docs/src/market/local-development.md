# Local Development Guide

Ensure the following software is installed on your machine before proceeding:

- **[Rust](https://www.rust-lang.org/tools/install) version 1.79 or higher**
- **[Foundry](https://book.getfoundry.sh/getting-started/installation) version 0.2 or higher**

Before starting, ensure you have cloned with recursive submodules, or pull them with:

```console
git submodule update --init
```

1. Start a local devnet
   ```console
   make devnet-up
   source .env
   ```

2. Test your deployment with the client CLI.
   You can read more about the client on the [proving request](../market/proving_request.md) page.

   ```console
   RISC0_DEV_MODE=1 RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-request request.yaml --wait
   ```

   > If you see "Error: Market error: Failed to check fulfillment status",
   > check the deployment logs from running `forge script` and ensure it matches the addresses listed in `.env`
   > If they don't match, adjust the `.env` file or try restarting anvil and deploying again.

Congratulations! You now have a local devnet running and a prover that will respond to proving requests.

3. To tear down the local devnet run:

   ```console
   make devnet-down
   ```

Check out the is-even example in the [Boundless Foundry template][boundless-foundry-template] for an example of how to run and application using the prover market.

You can also try editing `request.yaml` to send a request with different values.
Check `cargo run --bin cli -- --help` for a full list of commands available through the CLI.

If instead you prefer setting up a local devnet step by step, you can run the following commands as an alternative to the Makefile:

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
   Configuration environment variables are read from the `.env` file.
   By setting the environment variable `RISC0_DEV_MODE`, a mock verifier will be deployed.

   ```console
   source .env
   RISC0_DEV_MODE=1 forge script contracts/scripts/Deploy.s.sol --rpc-url $RPC_URL --broadcast -vv
   ```

   > NOTE: Starting from a fresh Anvil instance, the deployed contract addresses will match the values in `.env`.
   > If you need to deploy again, restart Anvil first or change the `.env` file to match your newly deployed contract addresses.

5. Deposit Prover funds and start the Broker

   The Broker is the service that watches the chain for proving requests, evaluates them, and orchestrates proving the jobs with the proving backend.

   Here we will use a mock prover by setting `RISC0_DEV_MODE`.
   The Broker can use either Bonsai or Bento as backend, remove `RISC0_DEV_MODE` and:

   - To use Bonsai, export the `BONSAI_API_URL` and `BONSAI_API_KEY` env vars, or the the associated CLI flags.
   - To use Bento, export the `BENTO_API_URL` env var or use the `--bento-api-url` CLI flag.
     Also, refer to the [Running Bento][page-bento-running] guide.

   The Broker needs to have funds deposited on the Boundless market contract to cover [lockin-stake][id-rfc-order-matching] on requests.
   Setting the `--deposit-amount` flag below has the Broker deposit 10 ETH to the market upon startup.

   ```console
   RISC0_DEV_MODE=1 RUST_LOG=info cargo run --bin broker -- --priv-key ${PROVER_PRIVATE_KEY:?} --proof-market-addr ${PROOF_MARKET_ADDRESS:?} --set-verifier-addr ${SET_VERIFIER_ADDRESS:?} --deposit-amount 10
   ```

6. Test your deployment with the boundless CLI.
   You can read more about on the [proving request][page-requestor-request] page.

   ```console
   RISC0_DEV_MODE=1 RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-request request.yaml --wait
   ```

   > If you see "Error: Market error: Failed to check fulfillment status",
   > check the deployment logs from running `forge script` and ensure it matches the addresses listed in `.env`
   > If they don't match, adjust the `.env` file or try restarting anvil and deploying again.

Congratulations! You now have a local devnet running and a prover that will respond to proving requests.

Check out the is-even example in the [Boundless Foundry template](https://github.com/boundless-xyz/boundless-foundry-template/)for an example of how to run and application using the prover market.

You can also try editing `request.yaml` to send a request with different values.
Check `cargo run --bin cli -- --help` for a full list of commands available through the CLI.

[page-bento-running]: ../prover-manual/bento/running_bento.md
[page-requestor-request]: ../requestor-manual/broadcasting.md
