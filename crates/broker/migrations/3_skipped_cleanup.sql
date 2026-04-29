-- Composite expression index on the JSON status and expire_timestamp fields.
-- Speeds up:
--   * status-IN polls (get_proving_order, get_aggregation_proofs, get_committed_orders, ...)
--   * the reaper's expired-committed-order scan
--   * the new skipped-cleanup DELETE
-- The expression syntax must match the queries' `data->>'status'` form so that
-- SQLite's planner picks the index.
CREATE INDEX IF NOT EXISTS idx_orders_status_expire
    ON orders(data->>'status', data->>'expire_timestamp');

-- One-shot backfill: delete every Skipped order whose expire_timestamp has
-- already passed, regardless of grace. This drains the historical backlog
-- accumulated before this migration. Steady-state cleanup is handled by the
-- reaper using the configured grace period.
DELETE FROM orders
 WHERE data->>'status' = 'Skipped'
   AND data->>'expire_timestamp' IS NOT NULL
   AND data->>'expire_timestamp' < unixepoch();
