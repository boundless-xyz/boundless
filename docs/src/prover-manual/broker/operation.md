# Broker Operation

<div class="warning">

Before operation Bento (except in basic [local development][page-local-dev] only), we highly suggest:

1. [Optimizing Bento performance][page-bento-perf] you will connect the Broker to
2. Researching the best [configuration][page-broker-config] options fitting your specific needs

</div>

## Connect a Bento instance

A Broker requires a [running Bento instance][page-bento-run], that can be affirmed with a test proof fulfillment:

```bash
RUST_LOG=info cargo run --bin bento_cli -- -c 32
```

```txt
2024-10-23T14:37:37.364844Z  INFO bento_cli: image_id: a0dfc25e54ebde808e4fd8c34b6549bbb91b4928edeea90ceb7d1d8e7e9096c7 | input_id: eccc8f06-488a-426c-ae3d-e5acada9ae22
2024-10-23T14:37:37.368613Z  INFO bento_cli: STARK job_id: 0d89e2ca-a1e3-478f-b89d-8ab23b89f51e
2024-10-23T14:37:37.369346Z  INFO bento_cli: STARK Job running....
2024-10-23T14:37:39.371331Z  INFO bento_cli: STARK Job running....
2024-10-23T14:37:41.373508Z  INFO bento_cli: STARK Job running....
2024-10-23T14:37:43.375780Z  INFO bento_cli: Job done!
```

## Funding your Broker on the proving market

To have the Broker interact with a [market deployment][page-deployments], an account for the network you wish to operate on is required, with sufficient native token (`ETH` typically) to pay for gas fees.

<div class="warning">

For market v0 Sepolia testnet purposes, we recommend around 1-2 Sepolia ETH.
Gas costs for market operation in future market versions should be significantly less.

</div>

The following process will guide you through setting up a new wallet and funding it with testnet ETH:

1. Set the environment variables `PRIVATE_KEY`, `SET_VERIFIER_ADDR`,`PROOF_MARKET_ADDR` in `.env-compose`:

```bash
# Prover node configs
...
PRIVATE_KEY=0xYOUR_TEST_WALLET_PRIVATE_KEY_HERE
...
PROOF_MARKET_ADDR=0x261D8c5e9742e6f7f1076Fa1F560894524e19cad # This is the address of the market contract on the target chain.
...
RPC_URL="https://rpc.sepolia.org" # This is the RPC URL of the target chain.
...
```

2. Load the `.env-compose` file into the environment:

```bash
source .env-compose
```

3. The Broker needs to have funds deposited on the Boundless market contract to cover lock-in stake on requests. Run the following command to deposit an initial amount of ETH into the market contract.

```bash
# Set amount in ETH denominated units
# 0.5 Sepolia ETH should be OK for basic testing
# BOUNDLESS_DEPOSIT=0.5

RUST_LOG=info,boundless_market=debug cargo run --bin cli --  deposit ${BOUNDLESS_DEPOSIT:?}
```

```txt
2024-10-23T14:29:52.704754Z DEBUG boundless_market::contracts::proof_market: Calling deposit() value: 500000000000000000
2024-10-23T14:29:52.993892Z DEBUG boundless_market::contracts::proof_market: Broadcasting deposit tx 0xfc5c11e75101a9158735ec9e9519f5692b2f64b3337268b7ed999502956cd982
2024-10-23T14:30:07.175952Z DEBUG boundless_market::contracts::proof_market: Submitted deposit 0xfc5c11e75101a9158735ec9e9519f5692b2f64b3337268b7ed999502956cd982
2024-10-23T14:30:07.175994Z  INFO cli: Deposited: 500000000000000000
```

## Debugging

### Orders stuck in 'lockin' or submit_merkle confirmation timeouts

If on the indexer you see your broker having a high number of orders locked-in but not being fulfilled it might be due to TXN confirmation timeouts. Initially increasing the `txn_timeout` in the `broker.toml` file is a good start to try and ensure the fulfillment completes.

Additionally it is possible to re-drive orders that are "stuck" via the following:

1. Manually connect to the sqlite DB for broker. This can be done inside the broker container via `sqlite3 /db/broker.db` or by mounting the `broker-data` docker volume

2. Finding the batch that contains the order:

```bash
SELECT id FROM batches WHERE data->>'orders' LIKE '%"TARGET_ORDER_ID"%';
# Example: SELECT id FROM batches WHERE data->>'orders' LIKE '%"0x466acfc0f27bba9fbb7a8508f576527e81e83bd00000caa"%';
```

3. Trigger a rerun of the submitter task:

```bash
UPDATE batches SET data = json_set(data, '$.status', 'Complete') WHERE id = YOUR_BATCH_ID_FROM_STEP_2;
# Example: UPDATE batches SET data = json_set(data, '$.status', 'Complete') WHERE id = 1;
```

[page-broker-config]: ./configure.md
[page-local-dev]: ../../market/local-development.md
[page-bento-perf]: ../bento/performance.md
[page-deployments]: ../../market/deployments.md
[page-bento-run]: ../bento/running.md
