# Order Stream

## Telemetry

The order stream exposes two telemetry endpoints that brokers use to report operational metrics. Payloads are authenticated with EIP-191 signatures and can be forwarded to a data store of users choosing. (the repo provides an implementation for forwarding to AWS Kinesis Data Streams).

### `POST /api/v2/heartbeats`

Broker identity heartbeat, sent hourly. Contains the broker's config, queue sizes, version, and uptime.

Payload: `BrokerHeartbeat` (see `crates/boundless-market/src/telemetry.rs`).

### `POST /api/v2/heartbeats/requests`

Request evaluation and completion heartbeat, sent every minute. Contains lists of `RequestEvaluated` (orders the broker assessed) and `RequestCompleted` (orders that reached a terminal state).

Payload: `RequestHeartbeat` (see `crates/boundless-market/src/telemetry.rs`).

### Authentication

Both endpoints require an `X-Signature` header containing a hex-encoded EIP-191 `personal_sign` signature over the raw request body. The server recovers the signer address and verifies it matches the `broker_address` in the payload, then checks that the address has sufficient ZKC collateral on the market contract.

## Integration Test

An end-to-end integration test lives at `tests/telemetry.rs`. It sends telemetry to a live order-stream deployment on AWS, then polls Redshift to verify the data arrived.

The test is skipped automatically as it is intended to test the AWS specific funtionality of the crate, and thus requires AWS credentials. It is only intended for manually running to verify the Order Stream -> AWS Kinesis logic.

### Required Environment Variables

| Variable            | Description                                                                |
| ------------------- | -------------------------------------------------------------------------- |
| `ORDER_STREAM_URL`  | Base URL of the order stream (e.g. `https://order-stream-dev.example.com`) |
| `PRIVATE_KEY`       | Hex private key of a broker with ZKC collateral deposited on the market    |
| `BOUNDLESS_ADDRESS` | BoundlessMarket contract address                                           |
| `CHAIN_ID`          | Network chain ID (e.g. `11155111` for Sepolia)                             |
| `REDSHIFT_URL`      | Postgres-compatible connection string for Redshift                         |

### Running

E.g:
```bash
export ORDER_STREAM_URL="https://order-stream-dev.example.com"
export PRIVATE_KEY="0xabc..."
export BOUNDLESS_ADDRESS="0x2d124b86..."
export CHAIN_ID="11155111"
export REDSHIFT_URL="postgres://readonly:pass@workgroup.123.us-west-2.redshift-serverless.amazonaws.com:5439/telemetry"

cargo test -p order-stream --test telemetry -- --nocapture
```

The test generates a unique `run_id` per invocation, embeds it in all payloads, and uses it to query Redshift for exactly the test data. It polls up to 90 seconds to allow for materialized view auto-refresh latency.
