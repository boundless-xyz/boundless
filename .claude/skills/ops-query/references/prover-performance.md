# Prover Performance

Deep dive into a specific prover's operational health across both data sources.

## Step 1: Get prover overview from indexer

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

## Step 2: Get prover aggregates from indexer

Use the per-prover aggregates endpoint to see hourly or daily trends. This is the best way to spot when performance changed:

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

For a longer view:

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/provers/${PROVER_ADDRESS}/aggregates?aggregation=daily&limit=14" | jq '
  .data[] | {
    timestamp_iso,
    total_requests_locked,
    total_requests_fulfilled,
    total_requests_locked_and_expired,
    locked_orders_fulfillment_rate
  }
'
```

Look for: hours/days with 0 locks (prover was down), drops in fulfillment rate, sudden volume changes.

## Step 3: Get telemetry completion and evaluation summary

When presenting prover telemetry, pivot outcomes into **columns** rather than showing one row per outcome. Include fulfilled count, cancelled (race losses), failure count, and average timing. `Cancelled` means the broker completed a proof but another prover fulfilled first — it's wasted work but not an error.

For completions:

```sql
SELECT
  broker_address,
  COUNT(*) AS completions,
  SUM(CASE WHEN outcome = 'Fulfilled' THEN 1 ELSE 0 END) AS fulfilled,
  SUM(CASE WHEN outcome = 'Cancelled' THEN 1 ELSE 0 END) AS cancelled,
  SUM(CASE WHEN outcome NOT IN ('Fulfilled', 'Cancelled') THEN 1 ELSE 0 END) AS failures,
  ROUND(AVG(CASE WHEN outcome = 'Fulfilled' THEN total_duration_secs END), 0) AS avg_total_s,
  ROUND(AVG(CASE WHEN outcome = 'Fulfilled' THEN actual_total_proving_time_secs END), 0) AS avg_proving_s
FROM telemetry.request_completions
WHERE broker_address = '<PROVER_ADDRESS>'
  AND completed_at > GETDATE() - INTERVAL '24 hours'
GROUP BY broker_address
ORDER BY completions DESC;
```

For evaluations:

```sql
SELECT
  broker_address,
  COUNT(*) AS evaluations,
  SUM(CASE WHEN outcome = 'Locked' THEN 1 ELSE 0 END) AS locked,
  SUM(CASE WHEN outcome = 'Skipped' THEN 1 ELSE 0 END) AS skipped,
  SUM(CASE WHEN commitment_outcome = 'Committed' THEN 1 ELSE 0 END) AS committed,
  SUM(CASE WHEN commitment_outcome = 'Dropped' THEN 1 ELSE 0 END) AS dropped
FROM telemetry.request_evaluations
WHERE broker_address = '<PROVER_ADDRESS>'
  AND evaluated_at > GETDATE() - INTERVAL '24 hours'
GROUP BY broker_address
ORDER BY evaluations DESC;
```

## Step 4: Failure breakdown

If the prover has failures, get the breakdown by outcome and error code. Exclude `Cancelled` (race losses, not errors):

```sql
SELECT
  outcome, error_code,
  MIN(error_reason) AS example_reason,
  COUNT(*) AS count
FROM telemetry.request_completions
WHERE broker_address = '<PROVER_ADDRESS>'
  AND completed_at > GETDATE() - INTERVAL '24 hours'
  AND outcome NOT IN ('Fulfilled', 'Cancelled')
GROUP BY outcome, error_code
ORDER BY count DESC;
```

## Step 5: Skip breakdown

Skips are orders rejected during pricing in the OrderPicker. Group by `skip_code` only since reasons contain per-request details.

```sql
SELECT
  skip_code,
  MIN(skip_reason) AS example_reason,
  COUNT(*) AS count
FROM telemetry.request_evaluations
WHERE broker_address = '<PROVER_ADDRESS>'
  AND evaluated_at > GETDATE() - INTERVAL '24 hours'
  AND outcome = 'Skipped'
GROUP BY skip_code
ORDER BY count DESC;
```

## Step 6: Drop breakdown

Drops are orders that passed pricing but were NOT committed to the proving pipeline by the OrderMonitor. High counts from `[B-OM-020]` (fulfilled by another prover) and `[B-OM-023]` (locked by another prover) are normal competitive behavior. Watch for `[B-OM-010]` (insufficient balance), `[B-OM-007]` (lock tx failed), or `[B-OM-026]` (can't complete by deadline) which indicate prover-side issues.

```sql
SELECT
  commitment_skip_code,
  MIN(commitment_skip_reason) AS example_reason,
  COUNT(*) AS count
FROM telemetry.request_evaluations
WHERE broker_address = '<PROVER_ADDRESS>'
  AND evaluated_at > GETDATE() - INTERVAL '24 hours'
  AND commitment_outcome = 'Dropped'
GROUP BY commitment_skip_code
ORDER BY count DESC;
```

## Step 7: Check capacity utilization from heartbeats

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
