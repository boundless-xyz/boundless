# Composition Example

> This example should be run against a deployment of the Boundless market.
> Environment variables for connecting to and interacting with the network are defined in a `.env` file. See [.env.testnet](../../.env.testnet) for testnet environment variables or [.env.localnet-template](../../.env.localnet-template) for local network config.

## Build

To build the example run:

```bash
cargo build
forge build
```

## Deploy

To deploy the Counter contract run:

```bash
forge script contracts/scripts/Deploy.s.sol --rpc-url ${RPC_URL:?} --broadcast -vv
```

eat
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

Running this example requires having access to a Boundless market deployment.

To run the example run:

```bash
RISC0_DEV_MODE=1 RUST_LOG=info cargo run --bin example-composition -- --counter-address ${COUNTER_ADDRESS:?}
```

By setting the `RISC0_DEV_MODE` env variable, a temporary file storage provider will be used.

Alternatively, you can also use an IPFS or AWS S3 provider. For IPFS, we suggest using [Pinata](https://www.pinata.cloud) as a pinning service, and have implemented builtin support for uploading files there.

To use IPFS via Pinata, just export the following env variables:

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
