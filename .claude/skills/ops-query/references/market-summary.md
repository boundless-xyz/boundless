# Market Summary

Overview of market health and prover activity over a time window (default: last 24 hours). Combines indexer aggregates with telemetry breakdowns.

## Presentation: Executive Summary

After collecting all data from the steps below, lead the response with a concise **Executive Summary** before showing the detailed tables. The executive summary should cover three areas in this order:

**1. Market & Requestors**: Overall fulfillment rate, total volume, any hours with 0% or degraded rates. List the top 5 most active requestors with labels. Call out any new/unknown requestors not in the address labels. Flag any requestors with low fulfillment rates or high expiration counts.

**2. Prover Ecosystem (from telemetry)**: Summarize common failure patterns across all telemetry brokers -- what error codes are appearing, whether they're new compared to yesterday, and at what rate. Call out any provers that are struggling (high failure rates, connection errors, insufficient balance) or that have stopped fulfilling requests, and explain why based on their skip/drop/failure data. Note unusual skip reasons (not just normal competitive skips) or notable drop reasons beyond `[B-OM-020]`/`[B-OM-023]`. Note the day-over-day trend (failure rate up/down, skip rate up/down, volume change).

**3. Our Provers**: For each prover we operate, give a health assessment: Are they online (check last activity time)? What's their fulfillment rate? What errors are they hitting? What are they skipping and why? Any capacity issues (drops from insufficient balance, lock failures)? Compare to yesterday if data is available.

After the executive summary, include the detailed data tables for reference.

## Step 1: Market health from indexer

Get hourly aggregates to see fulfillment rates and volume:

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/aggregates?aggregation=hourly&limit=24" | jq '
  .data[] | {
    timestamp_iso,
    total_requests_locked,
    total_locked_and_fulfilled,
    total_locked_and_expired,
    locked_orders_fulfillment_rate
  }
'
```

Summarize: total locks, total fulfilled, total expired, overall fulfillment rate, and number of active provers.

## Step 2: Requestor activity from indexer

Get the requestor leaderboard to see who is submitting requests, how many, and their fulfillment rates:

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/requestors?period=1d" | jq '
  .data[] | {
    requestor_address,
    requests_submitted,
    requests_fulfilled,
    requests_expired,
    fulfillment_rate,
    last_activity_time_iso
  }
'
```

Label addresses from `network_address_labels.json`. Note which are order generators we operate (e.g. `og_onchain`, `og_offchain`), which are external customers (e.g. Kailua, Signal, Citrea, BoB), and any new/unknown requestors. Flag any requestors with low fulfillment rates.

## Step 3: Prover leaderboard from indexer

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/provers?period=1d" | jq '
  .data[] | {
    prover_address,
    orders_locked,
    orders_fulfilled,
    locked_order_fulfillment_rate,
    fees_earned_formatted,
    last_activity_time_iso
  }
'
```

Label addresses from `network_address_labels.json`. Identify which are ours.

## Step 4: Telemetry completion summary

For brokers that send telemetry, get a pivoted view with fulfilled, failures, and average timings per broker:

```sql
SELECT
  broker_address,
  COUNT(*) AS completions,
  SUM(CASE WHEN outcome = 'Fulfilled' THEN 1 ELSE 0 END) AS fulfilled,
  SUM(CASE WHEN outcome <> 'Fulfilled' THEN 1 ELSE 0 END) AS failures,
  ROUND(AVG(CASE WHEN outcome = 'Fulfilled' THEN total_duration_secs END), 0) AS avg_total_s,
  ROUND(AVG(CASE WHEN outcome = 'Fulfilled' THEN actual_total_proving_time_secs END), 0) AS avg_proving_s
FROM telemetry.request_completions
WHERE completed_at > GETDATE() - INTERVAL '24 hours'
GROUP BY broker_address
ORDER BY completions DESC;
```

## Step 5: Telemetry evaluation summary

Get a pivoted view of evaluation outcomes per broker. `committed` = orders that entered the proving pipeline, `dropped` = orders that passed pricing but were not committed (lost race, failed lock, etc.):

```sql
SELECT
  broker_address,
  COUNT(*) AS evaluations,
  SUM(CASE WHEN outcome = 'Locked' THEN 1 ELSE 0 END) AS locked,
  SUM(CASE WHEN outcome = 'Skipped' THEN 1 ELSE 0 END) AS skipped,
  SUM(CASE WHEN commitment_outcome = 'Committed' THEN 1 ELSE 0 END) AS committed,
  SUM(CASE WHEN commitment_outcome = 'Dropped' THEN 1 ELSE 0 END) AS dropped
FROM telemetry.request_evaluations
WHERE evaluated_at > GETDATE() - INTERVAL '24 hours'
GROUP BY broker_address
ORDER BY evaluations DESC;
```

## Step 6: Failure breakdown

Failures are orders that entered the proving pipeline but did not result in a successful fulfillment. Common failure outcomes include `ProvingFailed`, `AggregationFailed`, `TxFailed`, `ExpiredWhileProving`, and `LockFailed`. The `error_code` identifies the stage (e.g. `[B-PRO-501]` = proving, `[B-AGG-*]` = aggregation, `[B-SUB-*]` = submission) and `error_reason` gives the specific message.

Show a per-prover table for the top 5 provers by volume plus all provers we operate:

```sql
SELECT
  broker_address, outcome, error_code,
  MIN(error_reason) AS example_reason,
  COUNT(*) AS count
FROM telemetry.request_completions
WHERE completed_at > GETDATE() - INTERVAL '24 hours'
  AND outcome <> 'Fulfilled'
GROUP BY broker_address, outcome, error_code
ORDER BY broker_address, count DESC;
```

## Step 7: Skip breakdown

Skips are orders rejected during pricing in the OrderPicker, before they ever reach the OrderMonitor or proving pipeline. The broker evaluated the order and decided not to attempt locking it. Common skip reasons include: unprofitable pricing (`[S-008]`), order too large for configured limits (`[S-007]`), requestor on deny list (`[S-OP-002]`), insufficient collateral (`[S-OP-008]`), order already locked/fulfilled (`[S-OP-004]`/`[S-OP-005]`), and input fetch failures (`[S-OP-009]`).

Show a per-prover table for the top 5 provers by volume plus all provers we operate:

```sql
SELECT
  broker_address, skip_code,
  MIN(skip_reason) AS example_reason,
  COUNT(*) AS count
FROM telemetry.request_evaluations
WHERE evaluated_at > GETDATE() - INTERVAL '24 hours'
  AND outcome = 'Skipped'
GROUP BY broker_address, skip_code
ORDER BY broker_address, count DESC;
```

## Step 8: Drop breakdown

Drops are orders that passed pricing in the OrderPicker but were NOT committed to the proving pipeline by the OrderMonitor. This happens when the broker wanted to lock an order but couldn't: another prover locked or fulfilled it first (`[B-OM-023]`/`[B-OM-020]`), the lock transaction failed or reverted (`[B-OM-007]`), the broker had insufficient balance (`[B-OM-010]`), or the order expired/became invalid before the broker could act (`[B-OM-021]`/`[B-OM-022]`). High drop counts from `[B-OM-020]` and `[B-OM-023]` are normal in a competitive market -- they just mean other provers won the race.

Show a per-prover table for the top 5 provers by volume plus all provers we operate:

```sql
SELECT
  broker_address, commitment_skip_code,
  MIN(commitment_skip_reason) AS example_reason,
  COUNT(*) AS count
FROM telemetry.request_evaluations
WHERE evaluated_at > GETDATE() - INTERVAL '24 hours'
  AND commitment_outcome = 'Dropped'
GROUP BY broker_address, commitment_skip_code
ORDER BY broker_address, count DESC;
```

## Step 9: Day-over-day comparison

Compare today's key metrics against yesterday to surface notable changes. Use the indexer's daily aggregates for the on-chain view:

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/aggregates?aggregation=daily&limit=2" | jq '
  .data[] | {
    timestamp_iso,
    total_requests_locked,
    total_locked_and_fulfilled,
    total_locked_and_expired,
    locked_orders_fulfillment_rate
  }
'
```

And compare telemetry failure/skip rates between the two periods:

```sql
SELECT
  CASE WHEN completed_at > GETDATE() - INTERVAL '24 hours' THEN 'today' ELSE 'yesterday' END AS period,
  COUNT(*) AS completions,
  SUM(CASE WHEN outcome = 'Fulfilled' THEN 1 ELSE 0 END) AS fulfilled,
  SUM(CASE WHEN outcome <> 'Fulfilled' THEN 1 ELSE 0 END) AS failures,
  ROUND(100.0 * SUM(CASE WHEN outcome <> 'Fulfilled' THEN 1 ELSE 0 END) / COUNT(*), 1) AS failure_pct
FROM telemetry.request_completions
WHERE completed_at > GETDATE() - INTERVAL '48 hours'
GROUP BY 1
ORDER BY 1 DESC;
```

```sql
SELECT
  CASE WHEN evaluated_at > GETDATE() - INTERVAL '24 hours' THEN 'today' ELSE 'yesterday' END AS period,
  COUNT(*) AS evaluations,
  SUM(CASE WHEN outcome = 'Locked' THEN 1 ELSE 0 END) AS locked,
  SUM(CASE WHEN outcome = 'Skipped' THEN 1 ELSE 0 END) AS skipped,
  ROUND(100.0 * SUM(CASE WHEN outcome = 'Skipped' THEN 1 ELSE 0 END) / COUNT(*), 1) AS skip_pct
FROM telemetry.request_evaluations
WHERE evaluated_at > GETDATE() - INTERVAL '48 hours'
GROUP BY 1
ORDER BY 1 DESC;
```

Call out anything notable: significant changes in volume, fulfillment rate, failure rate, new error codes that weren't present yesterday, provers that appeared or disappeared, etc.
