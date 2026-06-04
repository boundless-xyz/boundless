-- Off-chain proving-cost queries for the assessor cost analysis.
--
-- Source: broker telemetry, view `telemetry.request_completions`
--   (schema: crates/boundless-market/src/telemetry.rs -> RequestCompleted).
--
-- These quantify the off-chain cost a GUEST-BASED assessor always pays per
-- batch and that the on-chain assessor (PR 2005) avoids:
--   * assessor_proving_secs            -- assessor STARK (Path B only)
--   * set_builder_proving_secs         -- set-builder aggregation STARK (Path B only)
--   * assessor_compression_proof_secs  -- Groth16 compression of the aggregation proof
--
-- Proof paths (from the RequestCompleted doc comment):
--   Path A  -> proof_type in ('Groth16','Blake3Groth16'): STARK -> Groth16 ->
--              submit INDIVIDUALLY. No set builder, no shared assessor.
--              This is the "2 Groth16" world (app Groth16 + ... ).
--   Path B  -> proof_type = 'Merkle': STARK -> aggregation (set builder +
--              assessor) -> one Groth16 -> submit as batch. The "best case".
--
-- HOW TO RUN (see the ops-telemetry-query skill):
--   export REDSHIFT_URL="postgres://readonly:<pw>@<host>:5439/telemetry?sslmode=require"
--   psql "$REDSHIFT_URL" -f contracts/benchmarks/telemetry-queries.sql
-- Run once per environment (prod_base = chain 8453 is the primary target).
-- For windows before 2026-04-25, use the DuckDB/S3 archive path instead (the
-- skill documents the bucket globs; PERCENTILE_CONT can be mixed there).
--
-- Redshift notes: percentile aggregates can't be mixed in one SELECT, so each
-- percentile is its own UNION ALL subquery; use <> not !=, GETDATE() not NOW().

\echo '== Q1: path usage split (last 30d, fulfilled) =='
SELECT proof_type,
       COUNT(*)                                   AS completions,
       COUNT(assessor_proving_secs)               AS with_assessor_secs,
       COUNT(set_builder_proving_secs)            AS with_set_builder_secs
FROM telemetry.request_completions
WHERE outcome = 'Fulfilled'
  AND completed_at >= GETDATE() - INTERVAL '30 days'
GROUP BY proof_type
ORDER BY completions DESC;

\echo '== Q2: Path B (Merkle) off-chain component means (last 30d) =='
SELECT COUNT(*)                                       AS n_orders,
       ROUND(AVG(assessor_proving_secs), 3)           AS assessor_mean_s,
       ROUND(AVG(set_builder_proving_secs), 3)        AS set_builder_mean_s,
       ROUND(AVG(assessor_compression_proof_secs), 3) AS assessor_compress_mean_s,
       ROUND(AVG(stark_proving_secs), 3)              AS app_stark_mean_s,
       ROUND(AVG(total_cycles), 0)                    AS total_cycles_mean
FROM telemetry.request_completions
WHERE proof_type = 'Merkle' AND outcome = 'Fulfilled'
  AND completed_at >= GETDATE() - INTERVAL '30 days';

\echo '== Q3: Path B off-chain component percentiles (last 30d) =='
SELECT metric, ROUND(secs, 3) AS secs FROM (
  SELECT 'assessor_p50'           AS metric, PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY assessor_proving_secs)           AS secs FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND assessor_proving_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'assessor_p90', PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY assessor_proving_secs)           FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND assessor_proving_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'assessor_p99', PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY assessor_proving_secs)          FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND assessor_proving_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'set_builder_p50', PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY set_builder_proving_secs)     FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND set_builder_proving_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'set_builder_p90', PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY set_builder_proving_secs)     FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND set_builder_proving_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'set_builder_p99', PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY set_builder_proving_secs)    FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND set_builder_proving_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'assessor_compress_p50', PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY assessor_compression_proof_secs)  FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND assessor_compression_proof_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'assessor_compress_p90', PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY assessor_compression_proof_secs)  FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND assessor_compression_proof_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'assessor_compress_p99', PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY assessor_compression_proof_secs) FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND assessor_compression_proof_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
) t;

\echo '== Q4: total_cycles + app STARK percentiles, by path (last 30d) =='
SELECT metric, ROUND(val, 3) AS val FROM (
  SELECT 'cycles_p50_pathB'   AS metric, PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY total_cycles)     AS val FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND total_cycles IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'cycles_p90_pathB', PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY total_cycles)       FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND total_cycles IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'app_stark_p50_pathB', PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY stark_proving_secs) FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND stark_proving_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
  UNION ALL SELECT 'app_stark_p90_pathB', PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY stark_proving_secs) FROM telemetry.request_completions WHERE proof_type='Merkle' AND outcome='Fulfilled' AND stark_proving_secs IS NOT NULL AND completed_at >= GETDATE() - INTERVAL '30 days'
) t;

\echo '== Q5: approx batch-size proxy (orders sharing one aggregation event) =='
-- No batch_id column exists; assessor/set_builder secs are shared across a
-- batch, so group on (broker, rounded assessor secs, day) as a rough proxy.
-- Treat as indicative only.
SELECT orders_in_group, COUNT(*) AS num_batches
FROM (
  SELECT broker_address,
         DATE_TRUNC('day', completed_at)            AS day,
         ROUND(assessor_proving_secs, 2)            AS assessor_s,
         ROUND(set_builder_proving_secs, 2)         AS set_builder_s,
         COUNT(*)                                   AS orders_in_group
  FROM telemetry.request_completions
  WHERE proof_type = 'Merkle' AND outcome = 'Fulfilled'
    AND assessor_proving_secs IS NOT NULL
    AND completed_at >= GETDATE() - INTERVAL '30 days'
  GROUP BY 1, 2, 3, 4
) g
GROUP BY orders_in_group
ORDER BY orders_in_group;
