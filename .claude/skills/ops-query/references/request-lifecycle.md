# Request Lifecycle

Trace a specific request across both data sources to understand its full lifecycle.

## Step 1: Get on-chain status from indexer

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

## Step 2: Get all telemetry for this request

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

## Step 3: Synthesize

Combine the indexer timeline (submitted -> locked -> fulfilled/slashed/expired) with telemetry details (evaluation decisions across multiple brokers, proving durations, errors) to tell the full story of the request.
