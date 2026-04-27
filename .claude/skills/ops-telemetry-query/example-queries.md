# Example Telemetry Queries

## Broker Health

```sql
-- Most recent heartbeat for each broker
SELECT broker_address, version, uptime_secs / 3600 AS uptime_hours,
       committed_orders_count, pending_preflight_count, timestamp
FROM (
  SELECT *,
    ROW_NUMBER() OVER (PARTITION BY broker_address ORDER BY timestamp DESC) AS rn
  FROM telemetry.broker_heartbeats
)
WHERE rn = 1
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
-- Evaluations by broker (outcome: Locked, FulfillAfterLockExpire, Skipped)
SELECT broker_address,
       COUNT(*) AS total,
       SUM(CASE WHEN outcome = 'Locked' THEN 1 ELSE 0 END) AS locked,
       SUM(CASE WHEN outcome = 'FulfillAfterLockExpire' THEN 1 ELSE 0 END) AS fulfill_after_expire,
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
-- Average durations by phase (successful orders, last 24h, excludes 0s)
SELECT
  COUNT(*) AS completed,
  ROUND(AVG(NULLIF(committed_to_application_proof_duration_secs, 0)), 1) AS avg_app_proof_s,
  ROUND(AVG(NULLIF(aggregation_duration_secs, 0)), 1) AS avg_aggregation_s,
  ROUND(AVG(NULLIF(submission_duration_secs, 0)), 1) AS avg_submission_s,
  ROUND(AVG(NULLIF(actual_total_proving_time_secs, 0)), 1) AS avg_proving_total_s,
  ROUND(AVG(NULLIF(total_duration_secs, 0)), 1) AS avg_total_s,
  ROUND(AVG(NULLIF(stark_proving_secs, 0)), 2) AS avg_stark_s,
  ROUND(AVG(NULLIF(proof_compression_secs, 0)), 2) AS avg_compression_s,
  ROUND(AVG(NULLIF(set_builder_proving_secs, 0)), 2) AS avg_set_builder_s,
  ROUND(AVG(NULLIF(assessor_proving_secs, 0)), 2) AS avg_assessor_s,
  ROUND(AVG(NULLIF(assessor_compression_proof_secs, 0)), 2) AS avg_assessor_compress_s
FROM telemetry.request_completions
WHERE outcome = 'Fulfilled'
  AND completed_at > GETDATE() - INTERVAL '24 hours';
```

```sql
-- Median durations by phase (successful orders, last 24h, excludes 0s)
-- Redshift only allows one MEDIAN per SELECT, so each phase is a separate subquery.
WITH fulfilled AS (
  SELECT * FROM telemetry.request_completions
  WHERE outcome = 'Fulfilled' AND completed_at > GETDATE() - INTERVAL '24 hours'
)
SELECT * FROM (
  SELECT 'app_proof' AS phase, ROUND(MEDIAN(NULLIF(committed_to_application_proof_duration_secs, 0)), 1) AS median_s FROM fulfilled
  UNION ALL
  SELECT 'aggregation', ROUND(MEDIAN(NULLIF(aggregation_duration_secs, 0)), 1) FROM fulfilled
  UNION ALL
  SELECT 'submission', ROUND(MEDIAN(NULLIF(submission_duration_secs, 0)), 1) FROM fulfilled
  UNION ALL
  SELECT 'proving_total', ROUND(MEDIAN(NULLIF(actual_total_proving_time_secs, 0)), 1) FROM fulfilled
  UNION ALL
  SELECT 'total', ROUND(MEDIAN(NULLIF(total_duration_secs, 0)), 1) FROM fulfilled
  UNION ALL
  SELECT 'stark', ROUND(MEDIAN(NULLIF(stark_proving_secs, 0)), 2) FROM fulfilled
  UNION ALL
  SELECT 'compression', ROUND(MEDIAN(NULLIF(proof_compression_secs, 0)), 2) FROM fulfilled
  UNION ALL
  SELECT 'set_builder', ROUND(MEDIAN(NULLIF(set_builder_proving_secs, 0)), 2) FROM fulfilled
  UNION ALL
  SELECT 'assessor', ROUND(MEDIAN(NULLIF(assessor_proving_secs, 0)), 2) FROM fulfilled
  UNION ALL
  SELECT 'assessor_compress', ROUND(MEDIAN(NULLIF(assessor_compression_proof_secs, 0)), 2) FROM fulfilled
);
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
-- Find all telemetry for a specific request (wrapped subquery for Redshift ORDER BY compatibility)
SELECT * FROM (
  SELECT 'evaluation' AS source, broker_address, order_id, outcome, evaluated_at AS ts
  FROM telemetry.request_evaluations
  WHERE request_id = '0x...'
  UNION ALL
  SELECT 'completion', broker_address, order_id, outcome, completed_at
  FROM telemetry.request_completions
  WHERE request_id = '0x...'
)
ORDER BY ts;
```

## Archive (Through 2026-04-24) Queries

Use these when the user's time window ends before `2026-04-25T00:00:00Z`. They run via DuckDB against S3 Parquet — not psql. See the Archive Backend section in SKILL.md for bucket mapping, credentials, and routing rules.

Dialect reminders vs the Redshift queries above:

- `NOW()` replaces `GETDATE()`.
- `MEDIAN()` / `PERCENTILE_CONT()` have no single-aggregate restriction — you can compute all phase percentiles in one SELECT.
- `DISTINCT ON` and `LATERAL` joins work.
- Use explicit timestamp literals: `TIMESTAMP '2026-04-25 00:00:00+00'`.

### Top skip reasons, last 7 days of archive (staging_base_sepolia)

```bash
AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... duckdb <<SQL
INSTALL httpfs; LOAD httpfs;
SET s3_region='us-west-2';
SET s3_access_key_id='${AWS_ACCESS_KEY_ID}';
SET s3_secret_access_key='${AWS_SECRET_ACCESS_KEY}';

SELECT skip_code, COUNT(*) AS count,
       ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 1) AS pct
FROM read_parquet('s3://telemetry-snapshot-staging-84532-04-24-26/*/request_evaluations/*.parquet')
WHERE outcome = 'Skipped'
  AND evaluated_at BETWEEN TIMESTAMP '2026-04-18 00:00:00+00'
                       AND TIMESTAMP '2026-04-25 00:00:00+00'
GROUP BY skip_code
ORDER BY count DESC;
SQL
```

### Hourly completion volume for a broker, last 14 days of archive (prod_base)

```bash
AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... duckdb <<SQL
INSTALL httpfs; LOAD httpfs;
SET s3_region='us-west-2';
SET s3_access_key_id='${AWS_ACCESS_KEY_ID}';
SET s3_secret_access_key='${AWS_SECRET_ACCESS_KEY}';

SELECT DATE_TRUNC('hour', completed_at) AS hour,
       SUM(CASE WHEN outcome = 'Fulfilled' THEN 1 ELSE 0 END) AS successes,
       SUM(CASE WHEN outcome <> 'Fulfilled' THEN 1 ELSE 0 END) AS failures
FROM read_parquet('s3://telemetry-snapshot-prod-8453-04-24-26/*/request_completions/*.parquet')
WHERE broker_address = '0xabc...'
  AND completed_at BETWEEN TIMESTAMP '2026-04-11 00:00:00+00'
                       AND TIMESTAMP '2026-04-25 00:00:00+00'
GROUP BY 1
ORDER BY 1;
```

### Cross-cutover: run archive + live as two queries

When the user asks for a window that spans `2026-04-25T00:00:00Z`, run both sides separately. Do **not** try to UNION them. Example for "completions by outcome, 2026-04-22 through 2026-04-28":

```bash
# Side 1: archive (through end of 2026-04-24)
AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... duckdb <<SQL
INSTALL httpfs; LOAD httpfs;
SET s3_region='us-west-2';
SET s3_access_key_id='${AWS_ACCESS_KEY_ID}';
SET s3_secret_access_key='${AWS_SECRET_ACCESS_KEY}';

SELECT outcome, COUNT(*) AS count
FROM read_parquet('s3://telemetry-snapshot-staging-84532-04-24-26/*/request_completions/*.parquet')
WHERE completed_at BETWEEN TIMESTAMP '2026-04-22 00:00:00+00'
                       AND TIMESTAMP '2026-04-25 00:00:00+00'
GROUP BY outcome
ORDER BY count DESC;
SQL
```

```bash
# Side 2: live (from 2026-04-25 onwards)
psql "$REDSHIFT_URL" <<'SQL'
SELECT outcome, COUNT(*) AS count
FROM telemetry.request_completions
WHERE completed_at >= TIMESTAMPTZ '2026-04-25 00:00:00+00'
  AND completed_at <  TIMESTAMPTZ '2026-04-29 00:00:00+00'
GROUP BY outcome
ORDER BY count DESC;
SQL
```

Present both result sets to the user with a note that the window crossed the 2026-04-25 cutover.
