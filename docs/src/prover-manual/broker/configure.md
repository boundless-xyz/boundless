# Broker Node Configuration

Broker configuration is primarily managed through the `broker.toml` file in the boundless directory. This file is mounted into the Broker container and is used to configure the Broker daemon. This allows for dynamic configuration of the Broker without needing to restart the daemon as in most cases variables are refreshed. If you have changed a `broker.toml` configuration, and it does not appear to take effect you can restart the Broker service to apply the changes.

### Tuning market settings

`broker.toml` contains the following settings for the market:

| setting                | initial value | description                                                                                                    |
| ---------------------- | ------------- | -------------------------------------------------------------------------------------------------------------- |
| mcycle_price           | `".001"`      | The price (in native token of target market) of proving 1M cycles.                                             |
| assumption_price       | `"0.1"`       | Currently unused.                                                                                              |
| peak_prove_khz         | `500`         | This should correspond to the maximum number of cycles per second (in kHz) your proving backend can operate.   |
| min_deadline           | `150`         | This is a minimum number of blocks before the requested job expiration that Broker will attempt to lock a job. |
| lookback_blocks        | `100`         | This is used on Broker initialization, and sets the number of blocks to look back for candidate proofs.        |
| max_stake              | `"0.5"`       | The maximum amount used to lock in a job for any single order.                                                 |
| skip_preflight_ids     | `[]`          | A list of `imageID`s that the Broker should skip preflight checks in format `["0xID1","0xID2"]`.               |
| max_file_size          | `50_000_000`  | The maximum guest image size in bytes that the Broker will accept.                                             |
| allow_client_addresses | `[]`          | When defined, acts as a firewall to limit proving only to specific client addresses.                           |
| lockin_priority_gas    | `100`         | Additional gas to add to the base price when locking in stake on a contract to increase priority.              |

_note: pay particular attention to quotation for settings._

#### Making your Broker more competitive

The following examples would be methods of making your Broker more competitive in the market, either economically or accelerating the bidding process:

1. Decreasing the `mcycle_price` would tune your Broker to bid at lower prices for proofs.
2. Increasing `lockin_priority_gas` would expedite your market operations by consuming more has which could outrun other bidders.
3. Adding known `imageID`s from market observation to `skip_preflight_ids` would reduce the delay of preflight/execution on a binary allow you to beat other Brokers to transmitting a bid.

Before running Broker you will need to ensure you have setup and are able to run Bento, the documentation for that can be found in [Running Bento][page-running-bento].

```bash
docker compose --profile broker --env-file ./.env-compose up --build
```

### Tuning service settings

The `prover` settings in `broker.toml` are used to configure the prover service and largely impact the operation of the service rather than the market dynamics.

The most important one to monitor/tune on initial configuration is `txn_timeout`. This is the number of seconds to wait for a transaction to be mined before timing out, if you see timeouts in your logs this can be increased to enable TX operations to chain to finish.

## Using Broker with Bonsai

If you cannot run an instance of [Bento][page-bento] independently, it is possible to run Broker using the [Bonsai][bonsai-home] proving backend adding the `--bonsai-api-url` and `--bonsai-api-key` flags in the Service CLI config.

[page-bento]: ../bento/README.md
[page-running-bento]: ../bento/running.md
[page-perf]: ../bento/performance.md
[bonsai-home]: https://www.bonsai.xyz
