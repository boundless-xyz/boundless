# Broker Node Configuration

The broker node is an optional service that runs within the [Bento][page-bento] docker-compose stack. It is responsible for market interactions including bidding on jobs, locking them, issuing job requests to the bento proving cluster, and submitting on chain.

## Confirm your bento proving stack is operational
A broker requires a proving backend (typically bento) to be operational.  Ensure that yours is operational before proceeding. The following command will verify that your bento stack is operational:

```bash
bento-alpha:~/boundless$ RUST_LOG=info cargo run --bin bento_cli -- -c 32
2024-10-23T14:37:37.364844Z  INFO bento_cli: image_id: a0dfc25e54ebde808e4fd8c34b6549bbb91b4928edeea90ceb7d1d8e7e9096c7 | input_id: eccc8f06-488a-426c-ae3d-e5acada9ae22
2024-10-23T14:37:37.368613Z  INFO bento_cli: STARK job_id: 0d89e2ca-a1e3-478f-b89d-8ab23b89f51e
2024-10-23T14:37:37.369346Z  INFO bento_cli: STARK Job running....
2024-10-23T14:37:39.371331Z  INFO bento_cli: STARK Job running....
2024-10-23T14:37:41.373508Z  INFO bento_cli: STARK Job running....
2024-10-23T14:37:43.375780Z  INFO bento_cli: Job done!
```

Before starting we also recommend you [optimize the performance][page-perf] of your bento stack.
## Funding your broker on the proving market

To operate broker you will need to have an Ethereum wallet setup and funded with enough ETH to pay for gas fees. For test net purposes we recommend using a new wallet and funding it with 1-2 Sep Eth. Using metamask to initialize this wallet is typical. 

The following process will guide you through setting up a new wallet and funding it with testnet ETH:

1. Set the environment variables `PRIVATE_KEY`, `SET_VERIFIER_ADDR`,`PROOF_MARKET_ADDR` in `.env-compose`:
```
# Prover node configs
...
PRIVATE_KEY=0xYOUR_TEST_WALLET_PRIVATE_KEY_HERE
...
PROOF_MARKET_ADDR=0x261D8c5e9742e6f7f1076Fa1F560894524e19cad # This is the address of the market contract on the target chain.
...
RPC_URL="https://rpc.sepolia.org" # This is the RPC URL of the target chain.
...
```

2. Load the .env-compose file into the environment:
```bash
source .env-compose
```

3. The Broker needs to have funds deposited on the Boundless market contract to cover lock-in stake on requests. Run the following command to deposit an initial amount of ETH into the market contract. 
```bash
RUST_LOG=info,boundless_market=debug cargo run --bin cli --  deposit .N
```
For most provers on testnet an N of .5 should be sufficient for initial testing.

4. On success you should see a transaction hash and the amount deposited:
```bash
2024-10-23T14:29:52.704754Z DEBUG boundless_market::contracts::proof_market: Calling deposit() value: 10000000000000000
2024-10-23T14:29:52.993892Z DEBUG boundless_market::contracts::proof_market: Broadcasting deposit tx 0xfc5c11e75101a9158735ec9e9519f5692b2f64b3337268b7ed999502956cd982
2024-10-23T14:30:07.175952Z DEBUG boundless_market::contracts::proof_market: Submitted deposit 0xfc5c11e75101a9158735ec9e9519f5692b2f64b3337268b7ed999502956cd982
2024-10-23T14:30:07.175994Z  INFO cli: Deposited: 10000000000000000
```

## Configuring your broker 

Broker configuration is primarily managed through the `broker.toml` file in the boundless directory. This file is mounted into the broker container and is used to configure the broker daemon. This allows for dynamic configuration of the broker without needing to restart the daemon as in most cases variables are refreshed.  If you have changed a broker.toml configuration and it does not appear to take effect you can restart the broker service to apply the changes.

### Tuning market settings

`broker.toml` contains the following settings for the market:
| setting | initial value | description |
| ------- | ------------- | ----------- |
| mcycle_price | `".001"`' | The price (in native token of target market) of proving 1M cycles. |
| assumption_price | `"0.1"` | Currently unused. |
| peak_prove_khz| `500`| This should correspond to the maximum number of cycles per second (in kHz) your proving backend can operate. | 
| min_deadline | `150` | This is a minimum number of blocks before the requested job expiration that broker will attempt to lock a job. |
| lookback_blocks | `100` | This is used on broker initialization, and sets the number of blocks to look back for candidate proofs. |
| max_stake | `"0.5"` | The maximum amount used to lock in a job for any single order.|
| skip_preflight_ids | `[]` | A list of imageIDs that the broker should skip preflight checks in format `["0xID1","0xID2"]. |
| max_file_size = `50_000_000` | The maximum guest image size in bytes that the broker will accept. |
| allow_client_addresses | `[]` | When defined, acts as a firewall to limit proving only to specific client addresses. |
| lockin_priority_gas | `100` | Additional gas to add to the base price when locking in stake on a contract to increase priority. |

*note: pay particular attention to quotation for settings.*

#### Making your broker more competitive
The following examples would be methods of making your broker more competitive in the market, either economically or accelerating the bidding process:

1. Decreasing the `mcycle_price` would tune your broker to bid at lower prices for proofs.
2. Increasing `lockin_priority_gas` would expedite your market operations by consuming more has which could outrun other brokers/bidders.
3. Adding known imageIDs from market obvservation to `skip_preflight_ids` would reduce the delay of preflight/excution on a binary allow you to beat other brokers to transmitting a bid.

Before running broker you will need to ensure you have setup and are able to run Bento, the documentation for that can be found in [Running Bento][page-running-bento].

```bash
docker compose --profile broker --env-file ./.env-compose up --build
```

### Tuning service settings:
The `prover` settings in `broker.toml` are used to configure the prover service and largely impact the operation of the service rather than the market dynamics.

The most important one to monitor/tune on initial configuration is `txn_timeout`. This is the number of seconds to wait for a transaction to be mined before timing out, if you see timeouts in your logs this can be increased to enable TX operations to chain to finish.

## Using broker with bonsai:

Optionally if you can't run a Bento stack is is possible to run Broker using the Bonsai proving backend adding the `--bonsai-api-url` and `--bonsai-api-key` flags in the Service CLI config.

[page-bento]: ../bento/README.md
[page-running-bento]: ../bento/running_bento.md
[page-perf]: ../bento/performance.md

