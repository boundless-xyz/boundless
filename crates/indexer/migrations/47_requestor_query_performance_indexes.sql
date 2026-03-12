-- Migration 47: Requestor Query Performance Indexes
-- Optimizes the slow /v1/market/requestors and /v1/market/requestors/:address/requests endpoints

-- For list_requests_by_requestor: optimizes ROW_NUMBER() OVER (PARTITION BY request_id ...)
-- The window function partitions by request_id within a client_address filter.
-- Without this index, PostgreSQL must sort all rows for a client by request_id.
-- This composite index allows an ordered scan that naturally groups by request_id.
CREATE INDEX IF NOT EXISTS idx_request_status_client_request_id
    ON request_status (client_address, request_id, updated_at DESC, request_digest DESC);

-- For batched adjusted fulfilled count in get_requestor_leaderboard
-- Optimizes: COUNT(DISTINCT (pr.input_data, pr.image_url)) ... WHERE rs.fulfilled_at >= $1
--            AND rs.fulfilled_at < $2 AND rs.locked_at IS NOT NULL AND rs.client_address IN (...)
-- The existing idx_request_status_client_fulfilled_locked_digest covers (client_address, fulfilled_at, locked_at, request_digest)
-- but this covering index also includes request_digest for the JOIN to proof_requests
CREATE INDEX IF NOT EXISTS idx_request_status_client_fulfilled_for_adjusted
    ON request_status (client_address, fulfilled_at, request_digest)
    WHERE fulfilled_at IS NOT NULL AND locked_at IS NOT NULL;

-- For batched adjusted expired count in get_requestor_leaderboard
-- Optimizes: COUNT(DISTINCT (pr.input_data, pr.image_url)) ... WHERE rs.expires_at >= $1
--            AND rs.expires_at < $2 AND rs.request_status = 'expired' AND rs.locked_at IS NOT NULL
--            AND rs.client_address IN (...)
CREATE INDEX IF NOT EXISTS idx_request_status_client_expired_for_adjusted
    ON request_status (client_address, expires_at, request_digest)
    WHERE request_status = 'expired' AND locked_at IS NOT NULL;

