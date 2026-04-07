-- 004_add_global_telemetry_fields.sql
-- Adds global_committed_orders_count and global_pending_preflight_count to
-- broker_heartbeats view. These fields report cross-chain totals for
-- multichain broker deployments.

DROP VIEW IF EXISTS telemetry.broker_heartbeats;
CREATE VIEW telemetry.broker_heartbeats AS
SELECT
    broker_address, config, committed_orders_count,
    global_committed_orders_count, pending_preflight_count,
    global_pending_preflight_count,
    version, uptime_secs, events_dropped, "timestamp", received_at
FROM (
    SELECT
        data.broker_address::VARCHAR(42)                    AS broker_address,
        data.config                                         AS config,
        data.committed_orders_count::INTEGER                AS committed_orders_count,
        data.global_committed_orders_count::INTEGER         AS global_committed_orders_count,
        data.pending_preflight_count::INTEGER               AS pending_preflight_count,
        data.global_pending_preflight_count::INTEGER        AS global_pending_preflight_count,
        data.version::VARCHAR(64)                           AS version,
        data.uptime_secs::BIGINT                            AS uptime_secs,
        data.events_dropped::BIGINT                         AS events_dropped,
        data."timestamp"::TIMESTAMPTZ                       AS "timestamp",
        received_at,
        ROW_NUMBER() OVER (
            PARTITION BY data.broker_address, data."timestamp"
            ORDER BY received_at DESC
        ) AS rn
    FROM telemetry.heartbeats_raw
)
WHERE rn = 1;

-- Re-grant permissions lost by DROP VIEW
GRANT SELECT ON telemetry.broker_heartbeats TO readonly;

INSERT INTO telemetry._migrations (version, name) VALUES (4, '004_add_global_telemetry_fields');
