# Blake3-Groth16 Example

Boundless supports the standard Risc0 Groth16 compression which outputs the ReceiptClaim which can be hashed using SHA2 to get the claim digest. This is not ideal for environments which SHA2 hashing may be costly or impossible. The Chainway Labs team has written a new Groth16 circuit which has a single public output which is the BLAKE3 claim digest of a Risc0 execution.

This is an example of using the Boundless SDK to request a proof with this new proof type.

## Build

To build the example run:

```bash
forge build
cargo build
```

## Deploy

Set up your environment:

```bash
# Example environment for Sepolia
export RPC_URL=https://ethereum-sepolia-rpc.publicnode.com
export VERIFIER_ADDRESS=0xb121b667dd2cf27f95f9f5107137696f56f188f6
export PRIVATE_KEY=# ADD YOUR PRIVATE KEY HERE
```

> If you need a Sepolia testnet account, you can quickly create one with `cast wallet new`
> You'll need some Sepolia ETH, and a good source is the <a href="https://www.sepoliafaucet.io/">Automata Faucet</a>.

[Automata Faucet]: https://www.sepoliafaucet.io/

## Run the example

Running this example will send a proof request to the Boundless Market on Sepolia.

Alternatively, you can run a [local devnet](#local-development)

In order to send a request to Boundless, you'll need to upload your program to a public URL.
You can use any file hosting service, and the Boundless SDK provides built-in support uploading to AWS S3, and to IPFS via [Pinata](https://www.pinata.cloud).

To use IPFS via Pinata, just export the following env variables:

```bash
# The JWT from your Pinata account: https://app.pinata.cloud/developers/api-keys
# Run `cargo run --bin example-counter -- --help` for a full list of options.
export PINATA_JWT="YOUR_PINATA_JWT"
```

Set up your environment:

```bash
# Example environment for Sepolia
export RPC_URL=https://ethereum-sepolia-rpc.publicnode.com
export PRIVATE_KEY=# ADD YOUR PRIVATE KEY HERE
```

To run the example run:

```bash
cargo run --bin example-blake3-groth16
```

> TIP: You can get more detail about what is happening with `RUST_LOG=info,boundless_market=debug`

You can additionally monitor your request on the [Boundless Explorer](https://explorer.boundless.network).

## Local development

You can also run this example against a local devnet.
If you have [`just` installed](https://github.com/casey/just), then the following command to start an [Anvil](https://book.getfoundry.sh/anvil/) instance and deploy the contracts.

> Make sure you've cloned the full repository, and have Docker installed. This will build from source.

```bash
# In this directory, examples/counter
RISC0_DEV_MODE=1 just localnet up
source ../../.env.localnet

# Start a broker to accept orders
RUST_LOG=info cargo run --bin broker
```

By setting the `RISC0_DEV_MODE` env variable, the market will be deployed to use a mock verifier, and the prover will generate fake proofs.
Additionally, the app will default to using the local file system for programs and inputs, instead of uploading them to a public server.

Run the example:

```bash
RISC0_DEV_MODE=1 cargo run --bin example-blake3-groth16 -- --storage-provider file
```

When you are down, or you want to redeploy, run the following command.

```bash
just localnet down
```
