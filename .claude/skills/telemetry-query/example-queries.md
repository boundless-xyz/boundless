# Example Telemetry Queries

## Broker Health

```sql
-- Active brokers (heartbeat in last 24 hours)
SELECT broker_address, version, uptime_secs / 3600 AS uptime_hours,
       committed_orders_count, pending_preflight_count, timestamp
FROM telemetry.broker_heartbeats
WHERE timestamp > GETDATE() - INTERVAL '24 hours'
ORDER BY timestamp DESC;
```

```sql
-- Broker version distribution
SELECT version, COUNT(DISTINCT broker_address) AS broker_count
FROM telemetry.broker_heartbeats
WHERE timestamp > GETDATE() - INTERVAL '24 hours'
GROUP BY version
ORDER BY broker_count DESC;
```

## Evaluation Analysis

```sql
-- Accept vs skip rate over last 24h
SELECT outcome, COUNT(*) AS count,
       ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 1) AS pct
FROM telemetry.request_evaluations
WHERE evaluated_at > GETDATE() - INTERVAL '24 hours'
GROUP BY outcome;
```

```sql
-- Top skip reasons
SELECT skip_code, COUNT(*) AS count,
       ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 1) AS pct
FROM telemetry.request_evaluations
WHERE outcome = 'Skipped'
  AND evaluated_at > GETDATE() - INTERVAL '24 hours'
GROUP BY skip_code
ORDER BY count DESC;
```

```sql
-- Evaluations by broker (accepted = not Skipped)
SELECT broker_address,
       COUNT(*) AS total,
       SUM(CASE WHEN outcome != 'Skipped' THEN 1 ELSE 0 END) AS accepted,
       SUM(CASE WHEN outcome = 'Skipped' THEN 1 ELSE 0 END) AS skipped
FROM telemetry.request_evaluations
WHERE evaluated_at > GETDATE() - INTERVAL '24 hours'
GROUP BY broker_address
ORDER BY total DESC;
```

```sql
-- Cycle distribution of evaluated requests
SELECT
  CASE
    WHEN total_cycles < 1000000 THEN '<1M'
    WHEN total_cycles < 10000000 THEN '1M-10M'
    WHEN total_cycles < 100000000 THEN '10M-100M'
    WHEN total_cycles < 1000000000 THEN '100M-1B'
    ELSE '>1B'
  END AS cycle_bucket,
  COUNT(*) AS count
FROM telemetry.request_evaluations
WHERE evaluated_at > GETDATE() - INTERVAL '24 hours'
  AND total_cycles IS NOT NULL
GROUP BY 1
ORDER BY MIN(total_cycles);
```

## Completion Analysis

```sql
-- Success vs failure rate over last 24h
SELECT outcome, COUNT(*) AS count,
       ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 1) AS pct
FROM telemetry.request_completions
WHERE completed_at > GETDATE() - INTERVAL '24 hours'
GROUP BY outcome;
```

```sql
-- Failure breakdown by error code
SELECT error_code, error_reason, COUNT(*) AS count
FROM telemetry.request_completions
WHERE outcome != 'Fulfilled'
  AND completed_at > GETDATE() - INTERVAL '24 hours'
GROUP BY error_code, error_reason
ORDER BY count DESC;
```

```sql
-- Average proving durations (successful orders, last 24h)
SELECT
  COUNT(*) AS completed,
  ROUND(AVG(proving_duration_secs), 1) AS avg_proving_s,
  ROUND(AVG(aggregation_duration_secs), 1) AS avg_aggregation_s,
  ROUND(AVG(submission_duration_secs), 1) AS avg_submission_s,
  ROUND(AVG(total_duration_secs), 1) AS avg_total_s
FROM telemetry.request_completions
WHERE outcome = 'Fulfilled'
  AND completed_at > GETDATE() - INTERVAL '24 hours';
```

```sql
-- Proving time accuracy (estimated vs actual)
SELECT
  ROUND(AVG(estimated_proving_time_secs), 1) AS avg_estimated_s,
  ROUND(AVG(actual_total_proving_time_secs), 1) AS avg_actual_s,
  ROUND(AVG(actual_total_proving_time_secs::FLOAT / NULLIF(estimated_proving_time_secs, 0)), 2) AS avg_ratio
FROM telemetry.request_completions
WHERE outcome = 'Fulfilled'
  AND estimated_proving_time_secs > 0
  AND completed_at > GETDATE() - INTERVAL '24 hours';
```

```sql
-- Detailed proving phase breakdown (successful, last 24h)
SELECT
  ROUND(AVG(stark_proving_secs), 2) AS avg_stark_s,
  ROUND(AVG(proof_compression_secs), 2) AS avg_compression_s,
  ROUND(AVG(set_builder_proving_secs), 2) AS avg_set_builder_s,
  ROUND(AVG(assessor_proving_secs), 2) AS avg_assessor_s,
  ROUND(AVG(assessor_compression_proof_secs), 2) AS avg_assessor_compress_s
FROM telemetry.request_completions
WHERE outcome = 'Fulfilled'
  AND completed_at > GETDATE() - INTERVAL '24 hours';
```

## Time Series

```sql
-- Hourly completion counts over last 7 days
SELECT
  DATE_TRUNC('hour', completed_at) AS hour,
  SUM(CASE WHEN outcome = 'Fulfilled' THEN 1 ELSE 0 END) AS successes,
  SUM(CASE WHEN outcome != 'Fulfilled' THEN 1 ELSE 0 END) AS failures
FROM telemetry.request_completions
WHERE completed_at > GETDATE() - INTERVAL '7 days'
GROUP BY 1
ORDER BY 1;
```

```sql
-- Daily evaluation volume by broker
SELECT
  DATE_TRUNC('day', evaluated_at) AS day,
  broker_address,
  COUNT(*) AS evaluations
FROM telemetry.request_evaluations
WHERE evaluated_at > GETDATE() - INTERVAL '7 days'
GROUP BY 1, 2
ORDER BY 1, 3 DESC;
```

## Specific Order Lookup

```sql
-- Find all telemetry for a specific request
SELECT 'evaluation' AS source, broker_address, order_id, outcome, evaluated_at AS ts
FROM telemetry.request_evaluations
WHERE request_id = '0x...'
UNION ALL
SELECT 'completion', broker_address, order_id, outcome, completed_at
FROM telemetry.request_completions
WHERE request_id = '0x...'
ORDER BY ts;
```
