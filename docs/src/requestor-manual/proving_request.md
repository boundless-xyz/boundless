# Submit Proving Request

Programmatic interaction with the market is accomplished through `boundless-market` crate, using the `ProofMarketService` struct.
An example is provided in the [examples/counter](../../../examples/counter) directory.

You can also interact with the market via a market client CLI.
It builds upon the [boundless_market::contracts](../../../crates/boundless-market/src/contracts/proof_market.rs) library.

## CLI Usage

> **NOTE**: all the following commands can be run with the environment variable `RISC0_DEV_MODE` set;
> this should be done only while testing within a [local devnet](../broker/local_devnet.md) as the
> default storage provider will use temporary files.

The [client-cli](../../../crates/boundless-market/src/bin/cli.rs) allows to:

1. Submit proving request via a YAML file, an example can be found [here](../../../request.yaml).

   ```console
   RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-request request.yaml
   ```

   Should output something similar to

   ```console
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

   ````
   You can also add the `--wait` option to wait until the submitted request has been fulfilled:

   ```console
   RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-request request.yaml --wait
   ````

2. Request the status of a given proving request:

   ```console
   RUST_LOG=info,boundless_market=debug cargo run --bin cli -- status 3554585979324098154284013313896898623039163403618679259143
   ```

   While not fulfilled, this will print something like

   ```console
   2024-09-17T15:07:50.598471Z DEBUG boundless_market::contracts::proof_market: Calling requestIsFulfilled(3554585979324098154284013313896898623039163403618679259143)
   2024-09-17T15:07:50.598873Z DEBUG boundless_market::contracts::proof_market: Calling requestIsLocked(3554585979324098154284013313896898623039163403618679259143)
   2024-09-17T15:07:50.599142Z  INFO cli: Status: Locked
   ```

   or when fulfilled:

   ```console
   2024-09-17T15:10:15.807123Z DEBUG boundless_market::contracts::proof_market: Calling requestIsFulfilled(3554585979324098154284013313896898623039163403618679259143)
   2024-09-17T15:10:15.807584Z  INFO cli: Status: Fulfilled
   ```

3. Get the proof of a request

   With the `get-proof` command you can get the Journal and Seal of a fulfilled request:

   ```console
   RUST_LOG=info,boundless_market=debug cargo run --bin cli -- get-proof 3554585979324098154284013313896898623039163403618679259143
   ```

   Should output something like:

   ```console
   2024-09-17T15:14:01.312995Z DEBUG boundless_market::contracts::proof_market: Calling requestIsFulfilled(3554585979324098154284013313896898623039163403618679259143)
   2024-09-17T15:14:01.314302Z  INFO cli: Journal: "0x576564204a756c2020332031343a33373a31322050445420323032340a" - Seal: "0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000164578a3cc24cf38d1173509a99db4f70d57ff3a6c43cb2e8552a2a5d252968ba"
   ```

4. Send an offer with the requirements specified as command line arguments:

   With the `submit-offer` subcommand, you can specify the requirements and input as command-line options.
   It will upload the image and input, and place public URLs in the request.

   Images and (optionally) input can be hosted on IPFS via [Pinata](https://pinata.cloud).
   In order to use this command, setup an account with Pinata and provide your JWT API key.
   If instead the env variable `RISC0_DEV_MODE` is enabled, a temporary file storage provider will be used,
   and the Pinata one will be ignored.

   ```console
   PINATA_JWT="YOUR_PINATA_JWT" RUST_LOG=info,boundless_market=debug cargo run --bin cli -- submit-offer offer.yaml --wait --input "hello" --encode-input --journal-prefix ""
   ```

5. Slash a request and get back funds

   With the `slash` subcommand, you can slash a given `request ID` and get a refund of your offer:

   ```console
   RUST_LOG=info,boundless_market=debug cargo run --bin cli -- slash 3554585979324098154284013313896898623039163403618679259143
   ```

<!-- 🔗 Reference Style -->

{{#include ../links.md:21:}}
