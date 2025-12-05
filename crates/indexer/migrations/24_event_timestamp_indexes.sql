-- Migration 24: Event Table Timestamp Indexes
-- Optimizes timestamp-based queries on event tables that run during aggregation
-- These indexes enable efficient range scans for period-based metrics

-- For request_locked_events timestamp queries
-- Optimizes: get_period_all_lock_collateral (returns 50K-91K rows per query)
-- Query pattern: WHERE block_timestamp >= $1 AND block_timestamp < $2
-- Used for JOIN with proof_requests to retrieve lock_collateral values
CREATE INDEX IF NOT EXISTS idx_request_locked_events_block_timestamp
    ON request_locked_events (block_timestamp);

-- For request_fulfilled_events timestamp queries
-- Optimizes: get_period_fulfilled_count
-- Query pattern: WHERE block_timestamp >= $1 AND block_timestamp < $2
CREATE INDEX IF NOT EXISTS idx_request_fulfilled_events_block_timestamp
    ON request_fulfilled_events (block_timestamp);

-- For request_submitted_events timestamp queries
-- Optimizes: get_period_total_requests_submitted_onchain
-- Query pattern: WHERE block_timestamp >= $1 AND block_timestamp < $2
CREATE INDEX IF NOT EXISTS idx_request_submitted_events_block_timestamp
    ON request_submitted_events (block_timestamp);

-- Composite indexes for event table JOINs with request_status (for requestor queries)
-- Optimizes: get_period_requestor_fulfilled_count, get_period_requestor_unique_provers, etc.
-- Query pattern: JOIN request_status ON event.request_digest = request_status.request_digest
--                WHERE event.block_timestamp >= $1 AND event.block_timestamp < $2
--                AND request_status.client_address = $3
CREATE INDEX IF NOT EXISTS idx_request_fulfilled_events_digest_timestamp
    ON request_fulfilled_events (request_digest, block_timestamp);

CREATE INDEX IF NOT EXISTS idx_request_locked_events_digest_timestamp
    ON request_locked_events (request_digest, block_timestamp);

CREATE INDEX IF NOT EXISTS idx_request_submitted_events_digest_timestamp
    ON request_submitted_events (request_digest, block_timestamp);

