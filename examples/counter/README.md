# Counter Example

> This example should be run against a deployment of the Boundless market.
> See the [local devnet doc](../../docs/src/broker/local_devnet.md) for info on running one locally.
> Environment variables for connecting to and interacting with the network are defined in a [.env file](../../.env).

## Build

To build the example run:

```bash
cargo build
forge build
```

## Deploy

To deploy the Counter contract run:

```bash
forge script contracts/scripts/Deploy.s.sol --rpc-url ${L1_RPC_URL:?} --broadcast -vv
```

Save the `Counter` contract address to an env variable:

```bash
export COUNTER_ADDRESS=#COPY COUNTER ADDRESS FROM DEPLOY LOGS
```

> You can also use the following command to set the contract address if you have `jq` installed:
>
> ```bash
> export COUNTER_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "Counter") | .contractAddress' ./broadcast/Deploy.s.sol/31337/run-latest.json)
> ```
> 
> This command reads the Counter address from the broadcast logs generated when deploying to Anvil, using as default chain ID `31337`; 
> you can modify the chain ID if instead you are deploying to a different chain, e.g., on Sepolia use `11155111`.

## Run the example

Running this example requires having access to a Boundless market deployment. You can set a local one up by following the [local devnet](../../docs/src/broker/local_devnet.md) instructions. Alternatively, you can use the [Sepolia deployment](../../docs/src/broker/sepolia.md).

To run the example run:

```bash
RISC0_DEV_MODE=1 RUST_LOG=info cargo run --bin example-counter -- --counter-address ${COUNTER_ADDRESS:?}
```

By setting the `RISC0_DEV_MODE` env variable, a temporary file storage provider will be used.

Alternatively, you can also use am IPFS provider. We suggest using [Pinata](https://www.pinata.cloud).

Just export the following env variables:

```bash
# The JWT from your Pinata account, used to host guest binaries.
export PINATA_JWT="YOUR_PINATA_JWT"
# Optional: the IPFS Gateway URL, e.g., https://silver-adjacent-louse-491.mypinata.cloud
# default value is: https://dweb.link
export IPFS_GATEWAY_URL="YOUR_IPFS_GATEWAY_URL"
```

```bash
RUST_LOG=info cargo run --bin example-counter -- --counter-address ${COUNTER_ADDRESS:?}
```
