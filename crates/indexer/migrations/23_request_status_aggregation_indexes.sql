-- Migration 23: Request Status Aggregation Indexes
-- Optimizes aggregation queries that run on every block during market data computation
-- These indexes target the most expensive queries filtering by request_status and timestamps

-- For expired request queries (COUNT and lock_collateral retrieval)
-- Optimizes: get_period_expired_count
-- Query pattern: WHERE request_status = 'expired' AND expires_at >= $1 AND expires_at < $2
CREATE INDEX IF NOT EXISTS idx_request_status_expired_expires
    ON request_status (request_status, expires_at)
    WHERE request_status = 'expired';

-- For expired + locked queries (COUNT and lock_collateral retrieval)
-- Optimizes: get_period_locked_and_expired_count, get_period_locked_and_expired_collateral
-- Query pattern: WHERE request_status = 'expired' AND locked_at IS NOT NULL AND expires_at >= $1 AND expires_at < $2
-- Partial index for efficiency - only indexes rows that match the WHERE predicate
CREATE INDEX IF NOT EXISTS idx_request_status_expired_locked_expires
    ON request_status (request_status, locked_at, expires_at)
    WHERE request_status = 'expired' AND locked_at IS NOT NULL;

-- For fulfilled request queries (COUNT, SUM(program_cycles), SUM(total_cycles))
-- Optimizes: get_period_locked_and_fulfilled_count, get_period_total_program_cycles, get_period_total_cycles
-- Query pattern: WHERE request_status = 'fulfilled' AND fulfilled_at >= $1 AND fulfilled_at < $2
CREATE INDEX IF NOT EXISTS idx_request_status_fulfilled_fulfilled_at
    ON request_status (request_status, fulfilled_at)
    WHERE request_status = 'fulfilled';

-- For lock pricing data query with prover address comparison
-- Optimizes: get_period_lock_pricing_data (returns 8K-50K rows per query)
-- Query pattern: WHERE fulfilled_at >= $1 AND fulfilled_at < $2
--                AND request_status = 'fulfilled'
--                AND lock_prover_address = fulfill_prover_address
--                AND lock_prover_address IS NOT NULL
-- Covering index includes both prover address columns to avoid table lookups
CREATE INDEX IF NOT EXISTS idx_request_status_fulfilled_pricing_data
    ON request_status (fulfilled_at, lock_prover_address, fulfill_prover_address)
    WHERE request_status = 'fulfilled' 
      AND lock_prover_address IS NOT NULL
      AND fulfill_prover_address IS NOT NULL;

