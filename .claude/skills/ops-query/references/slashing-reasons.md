# Slashing Reasons

Determine why a prover was slashed on specific requests. A slash occurs when a prover locks a request, fails to fulfill it before the lock expires, and gets penalized.

## Step 1: Find slashed requests from the indexer

A prover is slashed on a request when they are the `lock_prover_address` and `slashed_at` is populated. Filter on `slashed_at` being non-null rather than `request_status == "slashed"`. A slashed request can have `request_status` of `"fulfilled"` (when another prover fulfilled it after the lock prover failed) or `"slashed"` (when nobody fulfilled it). The `slashed_at` field is the reliable indicator.

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

## Step 2: Look up telemetry for the slashed requests

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

## Step 3: Check broker load at the time of slashing

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

## Step 4: Analyze root causes

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
