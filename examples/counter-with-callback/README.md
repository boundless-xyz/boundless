# Counter with Callback Example

This is an example of using Boundless to produce proofs, for use in running a smart contract with a callback.
The smart contract is a simple counter of how many proofs have been verified, and the guest simply echos the input to the journal.
Unlike the basic counter example, this example demonstrates using a callback so the contract is automatically called when the proof is verified.

## Build

To build the example run:

```bash
cargo build
forge build
```

## Deploy

Set up your environment:

```bash
# Example environment for Sepolia
export RPC_URL=https://ethereum-sepolia-rpc.publicnode.com
export VERIFIER_ADDRESS=0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187
export PRIVATE_KEY=# ADD YOUR PRIVATE KEY HERE
```

> If you need a Sepolia testnet account, you can quickly create one with `cast wallet new`
> You'll need some Sepolia ETH, and a good source is the <a href="https://www.sepoliafaucet.io/">Automata Faucet</a>.

To deploy the Counter contract run:

```bash
forge script contracts/scripts/Deploy.s.sol --rpc-url ${RPC_URL:?} --broadcast -vv
```

Save the `Counter` contract address to an env variable:

```bash
export COUNTER_ADDRESS=# COPY COUNTER ADDRESS FROM DEPLOY LOGS
```

> You can also use the following command to set the contract address if you have `jq` installed, adjusting the `CHAIN_ID` depending on the network:
>
> ```bash
> CHAIN_ID=11155111 export COUNTER_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "Counter") | .contractAddress' ./broadcast/Deploy.s.sol/${CHAIN_ID:?}/run-latest.json)
> ```

## Run the example

Running this example will send a proof request to the Boundless Market on Sepolia.

Alternatively, you can run a [local devnet](#local-development)

In order to send a request to Boundless, you'll need to upload your program to a public URL.
You can use any file hosting service, and the Boundless SDK provides built-in support uploading to AWS S3, and to IPFS via [Pinata](https://www.pinata.cloud).

To use IPFS via Pinata, just export the following env variables:

```bash
# The JWT from your Pinata account: https://app.pinata.cloud/developers/api-keys
# Run `cargo run --bin example-counter-with-callback -- --help` for a full list of options.
export PINATA_JWT="YOUR_PINATA_JWT"
```

Set up your environment:

```bash
# Example environment for Sepolia
export RPC_URL=https://ethereum-sepolia-rpc.publicnode.com
export COUNTER_ADDRESS=# COPY COUNTER ADDRESS FROM DEPLOY LOGS
export PRIVATE_KEY=# ADD YOUR PRIVATE KEY HERE
```

To run the example run:

```bash
cargo run --bin example-counter-with-callback -- --counter-address ${COUNTER_ADDRESS:?}
```

> TIP: You can get more detail about what is happening with `RUST_LOG=info,boundless_market=debug`

You can additionally monitor your request on the [Boundless Explorer](https://explorer.beboundless.xyz).

## Local development

You can also run this example against a local devnet.
If you have [`just` installed](https://github.com/casey/just), then the following command to start an [Anvil](https://book.getfoundry.sh/anvil/) instance and deploy the contracts.

> Make sure you've cloned the full repository, and have Docker installed. This will build from source.

```bash
# In this directory, examples/counter-with-callback
RISC0_DEV_MODE=1 just localnet up
source ../../.env.localnet
```

By setting the `RISC0_DEV_MODE` env variable, the market will be deployed to use a mock verifier, and the prover will generate fake proofs.
Additionally, the app will default to using the local file system for programs and inputs, instead of uploading them to a public server.

Deploy your application to the local devnet:

```bash
forge script contracts/scripts/Deploy.s.sol --rpc-url ${RPC_URL:?} --broadcast -vv
```

Run the example:

```bash
RISC0_DEV_MODE=1 cargo run --bin example-counter-with-callback -- --counter-address ${COUNTER_ADDRESS:?} --storage-provider file
```

When you are down, or you want to redeploy, run the following command.

```bash
just localnet down
```