-- 001_initial_telemetry_tables.sql
-- Sets up streaming ingestion from Kinesis and creates typed views on top.
--
-- Prerequisites:
--   - Kinesis streams must exist before running this migration.
--   - The IAM role ARN must be substituted for ${IAM_ROLE_ARN} before applying.
--     The apply.sh script handles this via envsubst.

CREATE SCHEMA IF NOT EXISTS telemetry;

-- External schema: Kinesis streaming ingestion

CREATE EXTERNAL SCHEMA IF NOT EXISTS kds
FROM KINESIS
IAM_ROLE '${IAM_ROLE_ARN}';

-- Layer 1: Raw materialized views (stable, never need schema changes)
-- Each stores the full JSON payload as a SUPER column.

CREATE MATERIALIZED VIEW telemetry.heartbeats_raw AUTO REFRESH YES AS
SELECT
    approximate_arrival_timestamp AS received_at,
    partition_key,
    JSON_PARSE(from_varbyte(kinesis_data, 'utf-8')) AS data
FROM kds."${KINESIS_HEARTBEAT_STREAM}"
WHERE CAN_JSON_PARSE(kinesis_data);

CREATE MATERIALIZED VIEW telemetry.evaluations_raw AUTO REFRESH YES AS
SELECT
    approximate_arrival_timestamp AS received_at,
    partition_key,
    JSON_PARSE(from_varbyte(kinesis_data, 'utf-8')) AS data
FROM kds."${KINESIS_EVALUATIONS_STREAM}"
WHERE CAN_JSON_PARSE(kinesis_data);

CREATE MATERIALIZED VIEW telemetry.completions_raw AUTO REFRESH YES AS
SELECT
    approximate_arrival_timestamp AS received_at,
    partition_key,
    JSON_PARSE(from_varbyte(kinesis_data, 'utf-8')) AS data
FROM kds."${KINESIS_COMPLETIONS_STREAM}"
WHERE CAN_JSON_PARSE(kinesis_data);

-- Layer 2: Typed views (easily replaceable when fields are added)

CREATE OR REPLACE VIEW telemetry.broker_heartbeats AS
SELECT
    data.broker_address::VARCHAR(42)       AS broker_address,
    data.config                            AS config,
    data.committed_orders_count::INTEGER   AS committed_orders_count,
    data.pending_preflight_count::INTEGER  AS pending_preflight_count,
    data.version::VARCHAR(64)              AS version,
    data.uptime_secs::BIGINT               AS uptime_secs,
    data.events_dropped::BIGINT            AS events_dropped,
    data.timestamp::TIMESTAMPTZ            AS timestamp,
    received_at
FROM telemetry.heartbeats_raw;

CREATE OR REPLACE VIEW telemetry.request_evaluations AS
SELECT
    data.broker_address::VARCHAR(42)              AS broker_address,
    data.request_id::VARCHAR(78)                  AS request_id,
    data.requestor::VARCHAR(42)                   AS requestor,
    data.outcome::VARCHAR(32)                     AS outcome,
    data.skip_reason::VARCHAR(512)                AS skip_reason,
    data.total_cycles::BIGINT                     AS total_cycles,
    data.estimated_proving_time_secs::BIGINT      AS estimated_proving_time_secs,
    data.fulfillment_type::VARCHAR(32)            AS fulfillment_type,
    data.preflight_cache_hit::BOOLEAN             AS preflight_cache_hit,
    data.queue_duration_ms::BIGINT                AS queue_duration_ms,
    data.preflight_duration_ms::BIGINT            AS preflight_duration_ms,
    data.evaluated_at::TIMESTAMPTZ                AS evaluated_at,
    received_at
FROM telemetry.evaluations_raw;

CREATE OR REPLACE VIEW telemetry.request_completions AS
SELECT
    data.broker_address::VARCHAR(42)              AS broker_address,
    data.request_id::VARCHAR(78)                  AS request_id,
    data.outcome::VARCHAR(32)                     AS outcome,
    data.error_code::VARCHAR(32)                  AS error_code,
    data.lock_duration_secs::BIGINT               AS lock_duration_secs,
    data.proving_duration_secs::BIGINT            AS proving_duration_secs,
    data.aggregation_duration_secs::BIGINT        AS aggregation_duration_secs,
    data.submission_duration_secs::BIGINT         AS submission_duration_secs,
    data.total_duration_secs::BIGINT              AS total_duration_secs,
    data.estimated_proving_time_secs::BIGINT      AS estimated_proving_time_secs,
    data.actual_proving_time_secs::BIGINT         AS actual_proving_time_secs,
    data.concurrent_proving_jobs::INTEGER         AS concurrent_proving_jobs,
    data.total_cycles::BIGINT                     AS total_cycles,
    data.fulfillment_type::VARCHAR(32)            AS fulfillment_type,
    data.completed_at::TIMESTAMPTZ                AS completed_at,
    received_at
FROM telemetry.completions_raw;

-- Read-only user grants (user created by apply.sh before running migrations)
GRANT USAGE ON SCHEMA telemetry TO readonly;
GRANT SELECT ON ALL TABLES IN SCHEMA telemetry TO readonly;
ALTER DEFAULT PRIVILEGES IN SCHEMA telemetry GRANT SELECT ON TABLES TO readonly;

-- Migration tracking

CREATE TABLE IF NOT EXISTS telemetry._migrations (
    version     INTEGER       PRIMARY KEY,
    name        VARCHAR(256)  NOT NULL,
    applied_at  TIMESTAMPTZ   NOT NULL DEFAULT GETDATE()
);

INSERT INTO telemetry._migrations (version, name) VALUES (1, '001_initial_telemetry_tables');
