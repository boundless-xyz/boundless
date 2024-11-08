# Local Development Guide

To develop Boundless applications both as a requestor and prover, a running Market is required.
The workflow is generally:

::::steps
##### Step 1 

(One-time [Install](#install-boundless))

##### Step 2 

[Spin up a devnet](#run-a-market-devnet)

##### Step 3 

[Submit proof requests](#submit-proof-requests)

##### Step 4 

Tweak you app & submit more requests

##### Step 5 

[Tear down the devnet](#tear-down)
::::

To accelerate development, there are helpful utilities provided:

- Helpful `make` workflows:
  - `devnet-up` (default): spin up a service running in th background to fulfill proof requests
  - `devnet-down`: tear down a running devnet
- A CLI tool to interact with the Market:
  - `deposit`: Deposit funds
  - `withdraw`: Withdraw funds
  - `balance`: Check the balance of an account
  - `submit-offer`: Submit a proof request, constructed with the given offer, input, and image
  - `submit-request`: Submit a fully specified proof request
  - `slash`: Slash a prover for a given request
  - `get-proof`: Get the journal and seal for a given request
  - `verify-proof`: Verify the proof of the given request against the `SetVerifier` contract
  - `status`: Get the status of a given request

All utilities and more are provided in the [Boundless monorepo](https://github.com/boundless-xyz/boundless).

## Install Boundless

Ensure the following software is installed on your machine:

- **[Rust](https://www.rust-lang.org/tools/install) version 1.79 or higher**
- **[Foundry](https://book.getfoundry.sh/getting-started/installation) version 0.2 or higher**

##### 1. Clone Boundless (SSH or github login required)

```sh [Terminal]
# SSH or github login required
git clone git clone git@github.com:boundless-xyz/boundless.git
cd boundless
```

##### 2. Initialize recursive submodules (located in `lib`, required by Foundry)

```sh [Terminal]
git submodule update --init
```

## Run a Market devnet

For both requestors and provers, you will need to:

- Spin up a local `anvil` (or other EVM) devnet
- Deployed Boundless Market contracts to the devnet
- Run a [Broker][page-broker] instance that will lock in and return proofs to all proof requests
- Submit proof requests to be fulfilled by the Broker

### Spin up

An instance of the Market needs to be running in the background for applications to interact with, using the included `make` utilities or manually.

#### `make`

The included `makefile` in boundless is the most effective way to do this, and can be modified to suit specific needs.

##### 1. Start a local devnet service (running in the background)

```sh [Terminal]
make devnet-up
source .env
```

🎉 Congratulations!
You now have a local devnet service running in the background and a prover that will respond to proving requests.

When finished, tear down a running devnet run:

```sh [Terminal]
make devnet-down
```

#### Manually

If you require customizing a local devnet configuration, and need to operate it manually, you can run the following commands:

##### 1. Build the contracts

```sh [Terminal]
forge build
```

##### 2. Build the project

```sh [Terminal]
cargo build
```

##### 3. Start `anvil`

```sh [Terminal]
anvil -b 2
```

##### 4. Deploy market contracts

This will deploy the market contracts.
Configuration environment variables are read from the `.env` file.
By setting the environment variable `RISC0_DEV_MODE`, a mock verifier will be deployed.

```sh [Terminal]
source .env
DEPLOYER_PRIVATE_KEY=$PRIVATE_KEY CHAIN_KEY=anvil RISC0_DEV_MODE=$RISC0_DEV_MODE PROOF_MARKET_OWNER=$PUBLIC_KEY forge script contracts/scripts/Deploy.s.sol --rpc-url $RPC_URL --broadcast -vv
```

> NOTE: Starting from a fresh `anvil` instance, the deployed contract addresses will match the values in `.env`.
> If you need to deploy again, restart `anvil` first or change the `.env` file to match your newly deployed contract addresses.

##### 5. Deposit Prover funds and start the [Broker][page-broker]

Here we will use a mock prover by setting `RISC0_DEV_MODE`.
The Broker can use either [Bonsai][bonsai-homepage] or [Bento][page-bento] as backend, remove `RISC0_DEV_MODE` and:

- To use Bonsai, export the `BONSAI_API_URL` and `BONSAI_API_KEY` env vars, or the the associated CLI flags.
- To use Bento, export the `BENTO_API_URL` env var or use the `--bento-api-url` CLI flag.
  _This requires there is a Bento service listening, refer to the [Running Bento][page-bento-running] guide to configure and deploy one._

The Broker needs to have funds deposited on the Boundless market contract to cover lock-in stake on requests.
Setting the `--deposit-amount` flag below has the Broker deposit 10 ETH to the market upon startup.

```sh [Terminal]
RISC0_DEV_MODE=1 RUST_LOG=info cargo run --bin broker -- --private-key ${PRIVATE_KEY:?} --proof-market-addr ${PROOF_MARKET_ADDRESS:?} --set-verifier-addr ${SET_VERIFIER_ADDRESS:?} --deposit-amount 10
```

🎉 Congratulations!
You now have a local devnet running and a prover that will respond to proving requests.

### Submit Proof Requests

Test your devnet with the boundless CLI:

```sh [Terminal]
RISC0_DEV_MODE=1 RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-request request.yaml --wait
```

> If you see `Error: Market error: Failed to check fulfillment status`,
> check the `make devnet-up` deployment logs and ensure addresses match those listed in `.env`
> If they don't match, adjust the `.env` file and try again.

Try editing `request.yaml` to send a request to the Market with different values.

### Tear down

When finished, tear down a running devnet from the [`make devnet-up` workflow](#make) run:

```sh [Terminal]
make devnet-down
```

If running [manually](#manually), kill services and cleanup as needed.

## Application Development

Further instructions for:

- the [Requestor Broadcasting][page-requestor-broadcast] page for submitting proofs
  - See the [Boundless Foundry template][boundless-foundry-template-repo] for building a stand-alone application to interact with the Market
- the [Prover Manual][page-prover-manual] for fulfilling Market requests

[bonsai-homepage]: https://www.bonsai.xyz
[boundless-foundry-template-repo]: https://github.com/boundless-xyz/boundless-foundry-template
[page-bento]: ../prover-manual/bento/README.md
[page-bento-running]: ../prover-manual/bento/running.md
[page-broker]: ../prover-manual/broker/README.md
[page-market-design]: ./matching.md
[page-prover-manual]: ../prover-manual/README.md
[page-requestor-broadcast]: ../requestor-manual/broadcasting.md
[term-broker]: ../prover-manual/broker/README.md
