-- 005_preserve_telemetry_completion_aliases.sql
-- Keep the renamed completion timing fields available under their historical
-- view column names so existing dashboards and queries continue to work. The
-- fallback to the old JSON keys also preserves rows ingested before migration 3.

DROP VIEW IF EXISTS telemetry.request_completions;
CREATE VIEW telemetry.request_completions AS
SELECT
    broker_address, order_id, request_id, request_digest, proof_type, outcome,
    error_code, error_reason,
    lock_duration_secs,
    committed_to_application_proof_duration_secs,
    proving_duration_secs,
    aggregation_duration_secs, submission_duration_secs, total_duration_secs,
    estimated_proving_time_secs,
    actual_total_proving_time_secs,
    actual_proving_time_secs,
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
        COALESCE(
            data.committed_to_application_proof_duration_secs::BIGINT,
            data.proving_duration_secs::BIGINT
        )                                                           AS committed_to_application_proof_duration_secs,
        COALESCE(
            data.committed_to_application_proof_duration_secs::BIGINT,
            data.proving_duration_secs::BIGINT
        )                                                           AS proving_duration_secs,
        data.aggregation_duration_secs::BIGINT                     AS aggregation_duration_secs,
        data.submission_duration_secs::BIGINT                      AS submission_duration_secs,
        data.total_duration_secs::BIGINT                           AS total_duration_secs,
        data.estimated_proving_time_secs::BIGINT                   AS estimated_proving_time_secs,
        COALESCE(
            data.actual_total_proving_time_secs::BIGINT,
            data.actual_proving_time_secs::BIGINT
        )                                                           AS actual_total_proving_time_secs,
        COALESCE(
            data.actual_total_proving_time_secs::BIGINT,
            data.actual_proving_time_secs::BIGINT
        )                                                           AS actual_proving_time_secs,
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

-- Re-grant permissions lost by DROP VIEW
GRANT SELECT ON telemetry.request_completions TO readonly;

INSERT INTO telemetry._migrations (version, name)
VALUES (5, '005_preserve_telemetry_completion_aliases');
