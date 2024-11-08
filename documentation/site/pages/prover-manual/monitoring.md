# Monitoring

## Grafana

The bento / broker docker compose including a grafana instance with some template dashboards.

In order to access them, grafana is hosted at `http://localhost:3000`. Default credentials are defined in `.env-compose` as `admin:admin`

### Bento Dashboard

The bento dashboard connects to the taskdb postgresql instance to get live data for the status of different proofs flowing through the proving cluster. It is useful to monitor performance and queue depth.

### Broker Dashboard

The broker dashboard connects to the broker's sqlite database to see the status of different orders and batches moving through the broker's workflows.

## Onchain

Using the [Boundless market indexer](https://boundless-indexer-risczero.vercel.app/orders) is one of the best ways to monitor your broker's activity / health.

### Balances

When running the broker it is critical to monitor both your hot wallet balance of ETH and the proof market balance. If your broker runs out of ETH balance it will be unable to cover gas costs for transactions to locking / fulfill orders. If you run out of balance on the proof-market contract the broker will be unable to lockin orders with higher stakes. It is strongly recommended to keep your proof-market balance above the brokers configured `max_stake` parameter.

### Broker Logs

The logs from the broker service are the most helpful for monitoring what your prover is doing when interacting with the market. It is designed with the intention that DEBUG / INFO / WARN log should not require manual intervention, but anything logged at a ERROR level should be looked at by a operator.
