---
title: Broadcasting Proof Requests
description: This guide covers the broadcasting of proof requests to the Boundless Market.
---

# Broadcast a Proof Request

Programmatic interaction with the market is accomplished through `boundless-market` crate, using the `ProofMarketService` struct.
An example is provided in the `examples/counter` directory of the [Boundless monorepo][boundless-repo], and the [Boundless Foundry template][boundless-foundry-template-repo] for building a stand-alone application to interact with the Market

You can also interact with the market via a market client CLI.
It builds upon the `boundless_market::contracts` library.

## Local Devnet

To setup a local devnet follow the [local development guide][local-development].

You can override settings found in `.env` and more to use local devnet settings with by exporting or prefixing commands:

```sh [Terminal]
# Use dev mode in this terminal session
export RISC0_DEV_MODE=1
# Use dev mode for this command only
RISC0_DEV_MODE=1 <ANY BOUNDLESS CLI COMMAND>
```

Notably, Developer Mode will:

- Use a storage provider that interacts with temporary files.
- Use `anvil` default dev wallets to deploy and interact with contracts.

See the [CLI usage](#cli-usage) section or `examples/counter`'s `ProofMarketService` for further instructions.

## Public Networks

The Boundless Market is officially deployed only on [the Sepolia Testnet][id-deployments-sepolia-testnet] so far, with more networks to be announced.
Before you can interact with any network, you will need to configure an EVM RPC, Funds to pay for gas, and Image Storage Provider.

### Configure an EVM RPC Provider

You need an RPC provider to interact with any EVM network. [Alchemy](https://www.alchemy.com) supports various EVM networks, so creating a free account there is recommended, but many other options exist. Set the following environment variables according to your chosen RPC:

```sh [Terminal]
export RPC_URL="<SEPOLIA-URL>"
```

Or just modify the .env file and finally run `source .env`.

### Configure a Storage Provider

Boundless requires that ELF Image of the program requested, and optionally the input, to be proven be accessible to provers.

<!-- TODO: link to rustdocs and document how one might create a storage provider (perhaps via a DA?) -->

The best supported options are listed in the `boundless-market::BuiltinStorageProvider` enum.
IPFS storage is presently the best supported, specifically through [Pinata](https://www.pinata.cloud) which offers a free tier sufficient for most Boundless use cases.
To use Pinata, [fetch the JWT credentials](https://docs.pinata.cloud/account-management/api-keys) and set the `PINATA_JWT` environment variable.

### Sepolia Testnet

To interact with [Sepolia's Boundless contracts][id-deployments-sepolia-testnet] you will need:

- A Sepolia Ethereum account with at least 0.5 Sepolia ETH for gas.
  - The tooling presently requires the use of raw private key in scripting, although there are [better ways to do this](https://book.getfoundry.sh/tutorials/best-practices#private-key-management) that one could implement.
  <!-- TODO: need better ways to get funds for Boundless users! faucets are a HUGE pain, considering the round trip gas costs! -->
  - Faucets exist to obtain 0.1 ETH at a time, but almost all require an account

Make sure to export the env variable:

```sh [Terminal]
export PRIVATE_KEY="<YOUR-WALLET-PRIVATE_KEY>"
```

Or just modify the .env file and then run `source .env`

See the [CLI usage](#cli-usage) section for further instructions.

## CLI Usage

The `cli` allows to:

### Submit a Proving Request via a YAML File

An example can be found in `request.yaml`.

```sh [Terminal]
RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-request request.yaml
```

Should output something similar to

```txt [Terminal]
2024-09-17T15:01:00.213804Z DEBUG boundless_market::contracts::proof_market: Calling requestIsFulfilled(3554585979324098154284013313896898623039163403618679259140)
2024-09-17T15:01:00.215374Z DEBUG boundless_market::contracts::proof_market: Calling requestIsLocked(3554585979324098154284013313896898623039163403618679259140)
2024-09-17T15:01:00.216056Z  INFO cli: Client addr: 0x90F79bf6EB2c4f870365E785982E1f101E93b906
2024-09-17T15:01:00.216085Z DEBUG boundless_market::contracts::proof_market: Calling deposit() value: 2000000000000000
2024-09-17T15:01:00.217754Z DEBUG boundless_market::contracts::proof_market: Broadcasting deposit tx 0x001cb8e549af5e7617c9c1eb465d81db3054870c0f197f6e860710f68b8bff91
2024-09-17T15:01:00.471591Z DEBUG boundless_market::contracts::proof_market: Submitted deposit 0x001c…ff91
2024-09-17T15:01:00.471634Z DEBUG boundless_market::contracts::proof_market: Calling submitRequest(ProvingRequest { id: 3554585979324098154284013313896898623039163403618679259140, requirements: Requirements { imageId: 0x257569e11f856439ec3c1e0fe6486fb9af90b1da7324d577f65dd0d45ec12c7d, predicate: Predicate { predicateType: PrefixMatch, data: 0x57656420 } }, imageUrl: "https://dweb.link/ipfs/QmTx3vDKicYG5RxzMxrZEiCQJqhpgYNrSFABdVz9ri2m5P", input: Input { inputType: Inline, data: 0x1d000000570000006500000064000000200000004a000000750000006c0000002000000020000000330000002000000031000000340000003a00000033000000370000003a00000031000000320000002000000050000000440000005400000020000000320000003000000032000000340000000a000000 }, offer: Offer { minPrice: 100000000000000, maxPrice: 2000000000000000, biddingStart: 619, rampUpPeriod: 1000, timeout: 2000, lockinStake: 100000000000000 } })
2024-09-17T15:01:00.476867Z DEBUG boundless_market::contracts::proof_market: Broadcasting tx 0xd25d00d87fc57c8c5da47236dd6980fb250ae748f2e38e33f7c17cd3cb968b7e
2024-09-17T15:01:02.480340Z  INFO cli: Proving request ID 3554585979324098154284013313896898623039163403618679259140, bidding start at block number 619
```

You can also add the `--wait` option to wait until the submitted request has been fulfilled:

```sh [Terminal]
RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-request request.yaml --wait
```

### Request the Status of a Given Proving Request

```sh [Terminal]
RUST_LOG=info,boundless_market=debug cargo run --bin cli -- status 3554585979324098154284013313896898623039163403618679259143
```

While not fulfilled, this will print something like

```txt [Terminal]
2024-09-17T15:07:50.598471Z DEBUG boundless_market::contracts::proof_market: Calling requestIsFulfilled(3554585979324098154284013313896898623039163403618679259143)
2024-09-17T15:07:50.598873Z DEBUG boundless_market::contracts::proof_market: Calling requestIsLocked(3554585979324098154284013313896898623039163403618679259143)
2024-09-17T15:07:50.599142Z  INFO cli: Status: Locked
```

or when fulfilled:

```txt [Terminal]
2024-09-17T15:10:15.807123Z DEBUG boundless_market::contracts::proof_market: Calling requestIsFulfilled(3554585979324098154284013313896898623039163403618679259143)
2024-09-17T15:10:15.807584Z  INFO cli: Status: Fulfilled
```

### Get the Proof of a Request

With the `get-proof` command you can get the Journal and Seal of a fulfilled request:

```sh [Terminal]
RUST_LOG=info,boundless_market=debug cargo run --bin cli -- get-proof 3554585979324098154284013313896898623039163403618679259143
```

Should output something like:

```txt [Terminal]
2024-09-17T15:14:01.312995Z DEBUG boundless_market::contracts::proof_market: Calling requestIsFulfilled(3554585979324098154284013313896898623039163403618679259143)
2024-09-17T15:14:01.314302Z  INFO cli: Journal: "0x576564204a756c2020332031343a33373a31322050445420323032340a" - Seal: "0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000164578a3cc24cf38d1173509a99db4f70d57ff3a6c43cb2e8552a2a5d252968ba"
```

### Verify a Proof of a Request

With the `verify-proof` subcommand, you can verify a proof for a given request id and image id.

```sh [Terminal]
RUST_LOG=info,boundless_market=debug cargo run --bin cli -- verify-proof 0x466acfc0f27bba9fbb7a8508f576527e81e83bd00000052 257569e11f856439ec3c1e0fe6486fb9af90b1da7324d577f65dd0d45ec12c7d
```

Should output something like:

```sh [Terminal]
2024-10-07T14:50:54.442260Z  INFO cli: Proof for request id 0x466acfc0f27bba9fbb7a8508f576527e81e83bd00000052 verified successfully.
```

### Send an Offer with the Requirements Specified as Command Line Arguments

With the `submit-offer` subcommand, you can specify the requirements and input as command-line options.
It will upload the image and input, and place public URLs in the request.

Images and (optionally) input can be hosted on IPFS via [Pinata](https://pinata.cloud).
In order to use this command, setup an account with Pinata and provide your JWT API key.
If instead the env variable `RISC0_DEV_MODE` is enabled, a temporary file storage provider will be used,
and the Pinata one will be ignored.

```sh [Terminal]
PINATA_JWT="YOUR_PINATA_JWT" RUST_LOG=info cargo run --bin cli -- submit-offer --input "Hello world!" --inline-input --encode-input --journal-prefix "" offer.yaml
```

### Slash a Request and Get Back Funds

With the `slash` subcommand, you can slash a given `request ID` and get a refund of your offer:

```sh [Terminal]
RUST_LOG=info,boundless_market=debug cargo run --bin cli -- slash 3554585979324098154284013313896898623039163403618679259143
```

[boundless-foundry-template-repo]: https://github.com/boundless-xyz/boundless-foundry-template
[boundless-repo]: https://github.com/boundless-xyz/boundless
[id-deployments-sepolia-testnet]: /market/public-deployments#contracts
[local-development]: /market/local-development
