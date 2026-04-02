# Investigation Procedures

## Table of Contents

- [Slashing Reasons](#slashing-reasons)
- [Fulfillment Rate Drops](#fulfillment-rate-drops)
- [Prover Performance](#prover-performance)
- [Request Lifecycle](#request-lifecycle)

## Slashing Reasons

Determine why a prover was slashed on specific requests. A slash occurs when a prover locks a request, fails to fulfill it before the lock expires, and gets penalized.

### Step 1: Find slashed requests from the indexer

Query the market indexer for requests where the prover was slashed. Filter on `slashed_at` being non-null rather than `request_status == "slashed"`. A slashed request can have `request_status` of `"fulfilled"` (when another prover fulfilled it after the lock prover failed) or `"slashed"` (when nobody fulfilled it). The `slashed_at` field is the reliable indicator.

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/provers/${PROVER_ADDRESS}/requests?limit=100" | jq '
  .data | map(select(.slashed_at != null)) |
  .[] | {
    request_id,
    request_digest,
    request_status,
    client_address,
    lock_prover_address,
    fulfill_prover_address,
    locked_at_iso,
    slashed_at_iso,
    fulfilled_at_iso,
    lock_price_formatted,
    lock_collateral_formatted,
    slash_transferred_amount_formatted,
    slash_burned_amount_formatted,
    program_cycles,
    total_cycles,
    image_id
  }
'
```

If there are many slashed requests, paginate using `next_cursor`. Record:

- The `request_id` and `request_digest` for each slashed request
- The time window (`locked_at` to `slashed_at`)
- Whether `fulfill_prover_address` differs from `lock_prover_address` (indicates another prover fulfilled it)
- The `program_cycles` / `total_cycles` to compare against telemetry estimates

### Step 2: Look up telemetry for the slashed requests

For each slashed request (or batch of them), check completions telemetry. The `broker_address` in telemetry corresponds to the indexer's `lock_prover_address`.

```sql
SELECT
  order_id, request_id, request_digest, outcome,
  error_code, error_reason,
  estimated_proving_time_secs,
  actual_total_proving_time_secs,
  total_cycles,
  concurrent_proving_jobs_start,
  concurrent_proving_jobs_end,
  committed_to_application_proof_duration_secs,
  aggregation_duration_secs,
  submission_duration_secs,
  total_duration_secs,
  stark_proving_secs,
  proof_compression_secs,
  fulfillment_type,
  completed_at
FROM telemetry.request_completions
WHERE broker_address = '<PROVER_ADDRESS>'
  AND request_id IN ('<ID1>', '<ID2>', ...)
ORDER BY completed_at;
```

Also check the evaluation records to see what the broker estimated at commitment time:

```sql
SELECT
  order_id, request_id, request_digest, outcome,
  commitment_outcome, commitment_skip_code, commitment_skip_reason,
  total_cycles,
  estimated_proving_time_secs,
  estimated_proving_time_no_load_secs,
  concurrent_proving_jobs,
  pending_commitment_count,
  max_capacity,
  peak_prove_khz,
  queue_duration_ms,
  preflight_duration_ms,
  evaluated_at
FROM telemetry.request_evaluations
WHERE broker_address = '<PROVER_ADDRESS>'
  AND request_id IN ('<ID1>', '<ID2>', ...)
ORDER BY evaluated_at;
```

### Step 3: Check broker load at the time of slashing

See how many orders the broker was juggling around the slashing window:

```sql
SELECT
  broker_address,
  committed_orders_count,
  pending_preflight_count,
  uptime_secs / 3600 AS uptime_hours,
  version,
  timestamp
FROM telemetry.broker_heartbeats
WHERE broker_address = '<PROVER_ADDRESS>'
  AND timestamp BETWEEN '<LOCKED_AT>' AND '<SLASHED_AT>'
ORDER BY timestamp;
```

Also check what other orders were being completed in the same window (to see if the broker was overloaded):

```sql
SELECT
  order_id, request_id, outcome, error_code, error_reason,
  total_cycles,
  estimated_proving_time_secs,
  actual_total_proving_time_secs,
  concurrent_proving_jobs_start,
  completed_at
FROM telemetry.request_completions
WHERE broker_address = '<PROVER_ADDRESS>'
  AND completed_at BETWEEN '<LOCKED_AT>' AND '<SLASHED_AT>'
ORDER BY completed_at;
```

### Step 4: Analyze root causes

Common slashing root causes and what to look for:

| Root Cause                         | Telemetry Signals                                                                                                                      |
| ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| **Proving took too long**          | `actual_total_proving_time_secs` >> `estimated_proving_time_secs`; the order expired while proving (`outcome = 'ExpiredWhileProving'`) |
| **Estimation was wrong**           | `estimated_proving_time_secs` << `actual_total_proving_time_secs`; compare `total_cycles` from evaluation vs completion                |
| **Broker was overloaded**          | High `concurrent_proving_jobs_start`, high `committed_orders_count` in heartbeats, many other completions in the same window           |
| **Proving failed**                 | `outcome = 'ProvingFailed'`; check `error_code` and `error_reason`                                                                     |
| **Aggregation failed**             | `outcome = 'AggregationFailed'`; check `error_code` and `error_reason`                                                                 |
| **Lock tx failed**                 | `outcome = 'LockFailed'`; the lock never actually went through                                                                         |
| **Tx submission failed**           | `outcome = 'TxFailed'`; proof was generated but the on-chain tx failed                                                                 |
| **Expired before submission**      | `outcome = 'ExpiredBeforeSubmission'`; proof was ready but queue/submission too slow                                                   |
| **Another prover fulfilled first** | `outcome = 'FulfilledBeforeSubmission'`; race condition                                                                                |
| **No telemetry data**              | Broker doesn't send telemetry; note this and rely on indexer data only (lock/slash timing, cycles)                                     |

Synthesize findings into a narrative: which requests were slashed, what the likely root cause was for each, and whether there's a pattern (e.g. all large-cycle requests, all during a specific time window, all estimation errors).

## Fulfillment Rate Drops

Investigate why the market or a specific prover's fulfillment rate dropped.

### Step 1: Quantify the drop from the indexer

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

### Step 2: Get error breakdown from telemetry

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

### Step 3: Check broker health during the period

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

### Step 4: Compare proving time estimates vs actuals

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

## Prover Performance

Deep dive into a specific prover's operational health across both data sources.

### Step 1: Get prover overview from indexer

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/provers?period=7d" | jq '
  .data[] | select(.prover_address == "<PROVER_ADDRESS>") | {
    orders_locked,
    orders_fulfilled,
    locked_order_fulfillment_rate,
    fees_earned_formatted,
    best_effective_prove_mhz,
    collateral_deposited_zkc_formatted,
    collateral_available_zkc_formatted,
    last_activity_time_iso
  }
'
```

### Step 2: Get telemetry completion stats

```sql
SELECT
  outcome,
  COUNT(*) AS count,
  ROUND(AVG(total_duration_secs), 1) AS avg_total_s,
  ROUND(AVG(actual_total_proving_time_secs), 1) AS avg_proving_s,
  ROUND(AVG(estimated_proving_time_secs), 1) AS avg_estimated_s
FROM telemetry.request_completions
WHERE broker_address = '<PROVER_ADDRESS>'
  AND completed_at > GETDATE() - INTERVAL '7 days'
GROUP BY outcome
ORDER BY count DESC;
```

### Step 3: Check evaluation patterns

```sql
SELECT
  outcome,
  commitment_outcome,
  skip_code,
  COUNT(*) AS count
FROM telemetry.request_evaluations
WHERE broker_address = '<PROVER_ADDRESS>'
  AND evaluated_at > GETDATE() - INTERVAL '7 days'
GROUP BY outcome, commitment_outcome, skip_code
ORDER BY count DESC;
```

### Step 4: Check capacity utilization from heartbeats

```sql
SELECT
  DATE_TRUNC('hour', timestamp) AS hour,
  AVG(committed_orders_count) AS avg_committed,
  MAX(committed_orders_count) AS max_committed,
  AVG(pending_preflight_count) AS avg_pending
FROM telemetry.broker_heartbeats
WHERE broker_address = '<PROVER_ADDRESS>'
  AND timestamp > GETDATE() - INTERVAL '7 days'
GROUP BY 1
ORDER BY 1;
```

## Request Lifecycle

Trace a specific request across both data sources to understand its full lifecycle.

### Step 1: Get on-chain status from indexer

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/requests/${REQUEST_ID}" | jq '
  .data[] | {
    request_id,
    request_digest,
    request_status,
    source,
    client_address,
    lock_prover_address,
    fulfill_prover_address,
    created_at_iso,
    locked_at_iso,
    fulfilled_at_iso,
    slashed_at_iso,
    lock_price_formatted,
    lock_collateral_formatted,
    program_cycles,
    total_cycles,
    effective_prove_mhz,
    submit_tx_hash,
    lock_tx_hash,
    fulfill_tx_hash
  }
'
```

### Step 2: Get all telemetry for this request

```sql
SELECT * FROM (
  SELECT 'evaluation' AS source, broker_address, order_id,
    outcome::VARCHAR, skip_code, skip_reason,
    commitment_outcome::VARCHAR, commitment_skip_code,
    total_cycles, estimated_proving_time_secs,
    concurrent_proving_jobs,
    evaluated_at AS ts
  FROM telemetry.request_evaluations
  WHERE request_id = '<REQUEST_ID>'
  UNION ALL
  SELECT 'completion', broker_address, order_id,
    outcome::VARCHAR, error_code, error_reason,
    NULL, NULL,
    total_cycles, estimated_proving_time_secs,
    concurrent_proving_jobs_start,
    completed_at
  FROM telemetry.request_completions
  WHERE request_id = '<REQUEST_ID>'
)
ORDER BY ts;
```

This shows all brokers that evaluated the request (who skipped it, who locked it) and the final completion outcome.

### Step 3: Synthesize

Combine the indexer timeline (submitted -> locked -> fulfilled/slashed/expired) with telemetry details (evaluation decisions across multiple brokers, proving durations, errors) to tell the full story of the request.
