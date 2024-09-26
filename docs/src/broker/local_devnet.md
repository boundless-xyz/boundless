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
   forge script contracts/scripts/Deploy.s.sol --rpc-url $RPC_URL --broadcast -vv
   ```

5. Deposit funds and start the broker

   Optionally, by setting the environment variable `RISC0_DEV_MODE`, a mock prover will be used by the broker.

   ```console
   RUST_LOG=info cargo run --bin cli -- --wallet-private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 deposit 10
   ```

   The broker can use both Bonsai and Bento as backend. You can export the BONSAI_API_URL and BONSAI_API_KEY in the first case or refer to
   [Running Bento](../bento/running_bento.md) to use that.

   ```console
   export BONSAI_API_URL=<BONSAI_URL> BONSAI_API_KEY=<BONSAI_KEY>
   RUST_LOG=info cargo run --bin broker -- --priv-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 --proof-market-addr 0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0 --set-verifier-addr 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512
   ```

6. Test your deployment with the client cli.
   You can read more about the client on the [proving request](../market/proving_request.md) page.

   ```console
   RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-request request.yaml
   ```

Congratulations! You now have a local devnet running and a prover that will respond to proving requests.

Check out the [counter example](../../../examples/counter/README.md) for an example of how to run and application using the prover market.
