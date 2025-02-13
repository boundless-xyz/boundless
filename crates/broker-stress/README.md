# Broker Stress Test

This is a stress test for running the broker as a whole, with some chaos testing with random TCP errors.

## Usage

[Install toxiproxy](https://github.com/Shopify/toxiproxy#1-installing-toxiproxy)

```sh
RUST_LOG=info cargo run -p broker-stress -- --spawners 4 --rpc-reset-toxicity 0.0
```

> --rpc-reset-toxicity is the probability that the RPC connection will be reset (0.0-1.0)

And to run with a file based DB:

```sh
# Initialize the DB by calling VACUUM
sqlite3 /tmp/broker.db "VACUUM;"

RUST_LOG=info cargo run -p broker-stress -- --spawners 4 --database-url "sqlite:///tmp/broker.db"
```
