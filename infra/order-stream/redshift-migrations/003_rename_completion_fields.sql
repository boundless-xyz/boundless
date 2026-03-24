-- 003_rename_completion_fields.sql
-- Renames completion timing fields and removes unused preflight_cache_hit
-- from evaluations.
--
-- request_completions changes:
--   - proving_duration_secs -> committed_to_stark_proof_duration_secs
--     (JSON field renamed in broker)
--   - actual_proving_time_secs -> actual_total_proving_time_secs
--     (align view alias with JSON field name)
--
-- request_evaluations changes:
--   - Remove preflight_cache_hit (never populated; use preflight_duration_ms)

DROP VIEW IF EXISTS telemetry.request_completions;
CREATE VIEW telemetry.request_completions AS
SELECT
    broker_address, order_id, request_id, request_digest, proof_type, outcome,
    error_code, error_reason,
    lock_duration_secs, committed_to_stark_proof_duration_secs,
    aggregation_duration_secs, submission_duration_secs, total_duration_secs,
    estimated_proving_time_secs, actual_total_proving_time_secs,
    concurrent_proving_jobs_start, concurrent_proving_jobs_end,
    total_cycles, fulfillment_type,
    stark_proving_secs, proof_compression_secs,
    set_builder_proving_secs, assessor_proving_secs,
    assessor_compression_proof_secs,
    received_at_timestamp, completed_at, received_at
FROM (
    SELECT
        data.broker_address::VARCHAR(42)                           AS broker_address,
        data.order_id::VARCHAR(256)                                AS order_id,
        data.request_id::VARCHAR(78)                               AS request_id,
        data.request_digest::VARCHAR(78)                           AS request_digest,
        data.proof_type::VARCHAR(32)                               AS proof_type,
        data.outcome::VARCHAR(32)                                  AS outcome,
        data.error_code::VARCHAR(32)                               AS error_code,
        data.error_reason::VARCHAR(512)                            AS error_reason,
        data.lock_duration_secs::BIGINT                            AS lock_duration_secs,
        data.committed_to_stark_proof_duration_secs::BIGINT        AS committed_to_stark_proof_duration_secs,
        data.aggregation_duration_secs::BIGINT                     AS aggregation_duration_secs,
        data.submission_duration_secs::BIGINT                      AS submission_duration_secs,
        data.total_duration_secs::BIGINT                           AS total_duration_secs,
        data.estimated_proving_time_secs::BIGINT                   AS estimated_proving_time_secs,
        data.actual_total_proving_time_secs::BIGINT                AS actual_total_proving_time_secs,
        data.concurrent_proving_jobs_start::INTEGER                AS concurrent_proving_jobs_start,
        data.concurrent_proving_jobs_end::INTEGER                  AS concurrent_proving_jobs_end,
        data.total_cycles::BIGINT                                  AS total_cycles,
        data.fulfillment_type::VARCHAR(32)                         AS fulfillment_type,
        data.stark_proving_secs::DOUBLE PRECISION                  AS stark_proving_secs,
        data.proof_compression_secs::DOUBLE PRECISION              AS proof_compression_secs,
        data.set_builder_proving_secs::DOUBLE PRECISION            AS set_builder_proving_secs,
        data.assessor_proving_secs::DOUBLE PRECISION               AS assessor_proving_secs,
        data.assessor_compression_proof_secs::DOUBLE PRECISION     AS assessor_compression_proof_secs,
        data.received_at_timestamp::BIGINT                         AS received_at_timestamp,
        data.completed_at::TIMESTAMPTZ                             AS completed_at,
        received_at,
        ROW_NUMBER() OVER (
            PARTITION BY data.broker_address,
                         data.order_id
            ORDER BY received_at DESC
        ) AS rn
    FROM telemetry.completions_raw
)
WHERE rn = 1;

DROP VIEW IF EXISTS telemetry.request_evaluations;
CREATE VIEW telemetry.request_evaluations AS
SELECT
    broker_address, order_id, request_id, request_digest, requestor, outcome,
    skip_code, skip_reason, total_cycles, estimated_proving_time_secs,
    fulfillment_type, queue_duration_ms,
    preflight_duration_ms, received_at_timestamp, evaluated_at,
    commitment_outcome, commitment_skip_code, commitment_skip_reason,
    estimated_proving_time_no_load_secs, monitor_wait_duration_ms,
    peak_prove_khz, max_capacity, pending_commitment_count,
    concurrent_proving_jobs, lock_duration_secs, received_at
FROM (
    SELECT
        data.broker_address::VARCHAR(42)                       AS broker_address,
        data.order_id::VARCHAR(256)                            AS order_id,
        data.request_id::VARCHAR(78)                           AS request_id,
        data.request_digest::VARCHAR(78)                       AS request_digest,
        data.requestor::VARCHAR(42)                            AS requestor,
        data.outcome::VARCHAR(32)                              AS outcome,
        data.skip_code::VARCHAR(32)                            AS skip_code,
        data.skip_reason::VARCHAR(512)                         AS skip_reason,
        data.total_cycles::BIGINT                              AS total_cycles,
        data.estimated_proving_time_secs::BIGINT               AS estimated_proving_time_secs,
        data.fulfillment_type::VARCHAR(32)                     AS fulfillment_type,
        data.queue_duration_ms::BIGINT                         AS queue_duration_ms,
        data.preflight_duration_ms::BIGINT                     AS preflight_duration_ms,
        data.received_at_timestamp::BIGINT                     AS received_at_timestamp,
        data.evaluated_at::TIMESTAMPTZ                         AS evaluated_at,
        data.commitment_outcome::VARCHAR(32)                   AS commitment_outcome,
        data.commitment_skip_code::VARCHAR(32)                 AS commitment_skip_code,
        data.commitment_skip_reason::VARCHAR(512)              AS commitment_skip_reason,
        data.estimated_proving_time_no_load_secs::BIGINT       AS estimated_proving_time_no_load_secs,
        data.monitor_wait_duration_ms::BIGINT                  AS monitor_wait_duration_ms,
        data.peak_prove_khz::BIGINT                            AS peak_prove_khz,
        data.max_capacity::INTEGER                             AS max_capacity,
        data.pending_commitment_count::INTEGER                 AS pending_commitment_count,
        data.concurrent_proving_jobs::INTEGER                  AS concurrent_proving_jobs,
        data.lock_duration_secs::BIGINT                        AS lock_duration_secs,
        received_at,
        ROW_NUMBER() OVER (
            PARTITION BY data.broker_address,
                         data.order_id
            ORDER BY received_at DESC
        ) AS rn
    FROM telemetry.evaluations_raw
)
WHERE rn = 1;

-- Re-grant permissions lost by DROP VIEW
GRANT SELECT ON telemetry.request_completions TO readonly;
GRANT SELECT ON telemetry.request_evaluations TO readonly;

INSERT INTO telemetry._migrations (version, name) VALUES (3, '003_rename_completion_fields');
