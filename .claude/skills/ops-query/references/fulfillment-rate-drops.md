# Fulfillment Rate Drops

Investigate why the market or a specific prover's fulfillment rate dropped.

## Step 0: Check for recent deployments (for provers we operate)

If the alarm involves one of our provers (e.g. `BoundlessAWSSingle1`, `BPNightlyAWS`, `BPLatitudeNightly`, etc.), check the bento prover logs for recent deployments first. Nightly deployments restart Docker Compose and can cause extended outages if the new image is broken. See the "Checking for Recent Deployments" section in the ops-logs-query skill for query patterns.

## Step 1: Quantify the drop from the indexer

Get market aggregates at hourly or daily granularity to find the period where the rate dropped:

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/aggregates?aggregation=hourly&limit=48" | jq '
  .data[] | {
    timestamp_iso,
    total_requests_locked,
    total_locked_and_fulfilled,
    total_locked_and_expired,
    locked_orders_fulfillment_rate
  }
'
```

For a specific prover:

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/provers/${PROVER_ADDRESS}/aggregates?aggregation=hourly&limit=48" | jq '
  .data[] | {
    timestamp_iso,
    total_requests_locked,
    total_requests_fulfilled,
    total_requests_locked_and_expired,
    locked_orders_fulfillment_rate
  }
'
```

## Step 2: Get error breakdown from telemetry

For the same time window, check what completion outcomes brokers experienced:

```sql
SELECT
  outcome,
  error_code,
  error_reason,
  COUNT(*) AS count,
  ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 1) AS pct
FROM telemetry.request_completions
WHERE completed_at BETWEEN '<START>' AND '<END>'
  -- optionally filter to specific broker
  -- AND broker_address = '<PROVER_ADDRESS>'
GROUP BY outcome, error_code, error_reason
ORDER BY count DESC;
```

## Step 3: Check broker health during the period

```sql
SELECT
  broker_address,
  committed_orders_count,
  pending_preflight_count,
  version,
  timestamp
FROM telemetry.broker_heartbeats
WHERE timestamp BETWEEN '<START>' AND '<END>'
ORDER BY timestamp;
```

## Step 4: Compare proving time estimates vs actuals

```sql
SELECT
  ROUND(AVG(estimated_proving_time_secs), 1) AS avg_estimated_s,
  ROUND(AVG(actual_total_proving_time_secs), 1) AS avg_actual_s,
  ROUND(AVG(actual_total_proving_time_secs::FLOAT / NULLIF(estimated_proving_time_secs, 0)), 2) AS avg_ratio,
  COUNT(*) AS count
FROM telemetry.request_completions
WHERE completed_at BETWEEN '<START>' AND '<END>'
  AND estimated_proving_time_secs > 0;
```
