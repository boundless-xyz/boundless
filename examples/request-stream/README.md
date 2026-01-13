# Request Stream Example

This example demonstrates how to send a continuous stream of proof requests to the Boundless Market based on blockchain events. It showcases a common pattern: monitoring the blockchain for new blocks and submitting proof requests for each block range.

## What This Example Does

1. **Monitors the blockchain** for new blocks using a Rust stream
2. **Collects block hashes** every N blocks (configurable, default: 2)
3. **Constructs proof requests** with the block hashes as input
4. **Submits requests** to the Boundless Market
5. **Waits for fulfillment** and processes the next block range

## Key Concepts Demonstrated

- **Stream-based Request Processing**: Using Rust streams to process events asynchronously
- **Request Construction**: Building proof requests with custom input and request IDs
- **Request Lifecycle**: Submitting requests and waiting for fulfillment

## Build

To build the example run:

```bash
cargo build
```

## Run the example

Running this example will continuously monitor the blockchain and submit proof requests for each block range.

### Prerequisites

In order to send a request to Boundless, you'll need to upload your program to a public URL.
You can use any file hosting service, and the Boundless SDK provides built-in support for uploading to AWS S3, and to IPFS via [Pinata](https://www.pinata.cloud).

To use IPFS via Pinata, just export the following env variables:

```bash
# The JWT from your Pinata account: https://app.pinata.cloud/developers/api-keys
export PINATA_JWT="YOUR_PINATA_JWT"
```

### Set up your environment

```bash
# Example environment for Sepolia
export RPC_URL=https://ethereum-sepolia-rpc.publicnode.com
export PRIVATE_KEY=# ADD YOUR PRIVATE KEY HERE
```

> If you need a Sepolia testnet account, you can quickly create one with `cast wallet new`
> You'll need some Sepolia ETH, and a good source is the [Automata Faucet](https://www.sepoliafaucet.io/).

### Run

To run the example with default settings (2 blocks per request):

```bash
cargo run --bin example-request-stream
```

To customize the number of blocks per request:

```bash
cargo run --bin example-request-stream -- --blocks-per-request 10
```

> TIP: You can get more detail about what is happening with `RUST_LOG=info,boundless_market=debug`

You can additionally monitor your requests on the [Boundless Explorer](https://explorer.boundless.network).

## Local development

You can also run this example against a local devnet.
If you have [`just` installed](https://github.com/casey/just), then the following command will start an [Anvil](https://book.getfoundry.sh/anvil/) instance and deploy the contracts.

> Make sure you've cloned the full repository, and have Docker installed. This will build from source.

```bash
# In this directory, examples/request-stream
RISC0_DEV_MODE=1 just localnet up
source ../../.env.localnet

# Start a broker to accept orders
cargo run --bin broker
```

By setting the `RISC0_DEV_MODE` env variable, the market will be deployed to use a mock verifier, and the prover will generate fake proofs.
Additionally, the app will default to using the local file system for programs and inputs, instead of uploading them to a public server.

Run the example:

```bash
# Source env if running in separate terminal window
source ../../.env.localnet

# Run with local file storage
RISC0_DEV_MODE=1 cargo run --bin example-request-stream -- --storage-provider file
```

The example will:

- Monitor blocks on the local Anvil instance
- Submit a request every 2 blocks (default)
- Wait for each request to be fulfilled before processing the next block range

When you are done, or you want to redeploy, run the following command:

```bash
just localnet down
```

## How It Works

### Stream Architecture

The example uses Rust's async streams to process block range events:

```rust
while let Some(event) = stream.next().await {
    let input = input_function(&event.block_hashes);
    let request_id = request_id_function(address, event.start_block);
    
    let request = client
        .new_request()
        .with_program_url(program_url)?
        .with_request_input(input)
        .with_request_id(request_id);
    
    let (request_id, expires_at) = client.submit_onchain(request).await?;
    // Wait for fulfillment...
}
```

### Request ID Construction

Each request ID is constructed from the block range:

- Uses the start block number as the index
- Ensures each block range gets a unique, deterministic request ID
- Makes it easy to identify which block range a request corresponds to

### Input Construction

The input is constructed by concatenating all block hashes in the range:

- Each block hash is 32 bytes
- The guest program receives this concatenated data
- The guest can deserialize and process the block hashes as needed
