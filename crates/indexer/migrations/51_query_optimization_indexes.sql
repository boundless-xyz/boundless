-- Migration 51: Query Optimization Indexes
-- Removes redundant index, adds targeted partial indexes for aggregation queries

-- Drop redundant index superseded by migration 48's idx_request_status_client_fulfilled_input_image
-- which uses md5(input_data) instead of raw input_data for reduced index size.
-- Original purpose (get_period_requestor_locked_and_fulfilled_count_adjusted) is now served
-- by idx_request_status_client_fulfilled_input_image.
DROP INDEX IF EXISTS idx_request_status_client_fulfilled_locked_digest;

-- Optimizes: get_period_total_program_cycles, get_period_requestor_total_program_cycles
-- Query pattern: SELECT program_cycles FROM request_status
--   WHERE request_status = 'fulfilled' AND program_cycles IS NOT NULL
--   AND fulfilled_at >= $1 AND fulfilled_at < $2
-- Previously used idx_request_status_fulfilled_fulfilled_at but filtered ~10K NULL rows post-scan.
-- This partial index eliminates the NULL filter and enables index-only scans.
CREATE INDEX IF NOT EXISTS idx_request_status_fulfilled_program_cycles
    ON request_status (fulfilled_at, program_cycles)
    WHERE request_status = 'fulfilled' AND program_cycles IS NOT NULL;

-- Optimizes: get_period_total_cycles, get_period_requestor_total_cycles
-- Same pattern as above but for total_cycles column.
CREATE INDEX IF NOT EXISTS idx_request_status_fulfilled_total_cycles
    ON request_status (fulfilled_at, total_cycles)
    WHERE request_status = 'fulfilled' AND total_cycles IS NOT NULL;

-- Optimizes: get_period_prover_median_effective_prove_mhz (prover aggregation percentile)
-- Query pattern: SELECT prover_effective_prove_mhz FROM request_status
--   WHERE fulfill_prover_address = $1 AND request_status = 'fulfilled'
--   AND fulfilled_at >= $2 AND fulfilled_at < $3 AND prover_effective_prove_mhz IS NOT NULL
-- Previously used BitmapAnd of two separate indexes (fulfill_prover_updated + fulfilled_fulfilled_at).
-- This composite index enables a single index scan.
CREATE INDEX IF NOT EXISTS idx_request_status_prover_fulfilled_mhz
    ON request_status (fulfill_prover_address, fulfilled_at, prover_effective_prove_mhz)
    WHERE request_status = 'fulfilled'
      AND fulfill_prover_address IS NOT NULL
      AND prover_effective_prove_mhz IS NOT NULL;

-- Optimizes: get_period_requestor_total_program_cycles, get_period_requestor_total_cycles
-- Query pattern: SELECT program_cycles FROM request_status
--   WHERE request_status = 'fulfilled' AND program_cycles IS NOT NULL
--   AND fulfilled_at >= $1 AND fulfilled_at < $2 AND client_address = $3
-- Previously used BitmapAnd of two separate indexes. This composite index enables a single scan.
CREATE INDEX IF NOT EXISTS idx_request_status_client_fulfilled_program_cycles
    ON request_status (client_address, fulfilled_at, program_cycles)
    WHERE request_status = 'fulfilled' AND program_cycles IS NOT NULL;

-- Same for total_cycles per requestor.
CREATE INDEX IF NOT EXISTS idx_request_status_client_fulfilled_total_cycles
    ON request_status (client_address, fulfilled_at, total_cycles)
    WHERE request_status = 'fulfilled' AND total_cycles IS NOT NULL;

-- Optimizes: get_period_prover_total_program_cycles, get_period_prover_total_cycles
-- Query pattern: SELECT program_cycles FROM request_status
--   WHERE fulfill_prover_address = $1 AND request_status = 'fulfilled'
--   AND program_cycles IS NOT NULL AND fulfilled_at >= $2 AND fulfilled_at < $3
-- Previously used BitmapAnd. This composite index enables a single scan.
CREATE INDEX IF NOT EXISTS idx_request_status_prover_fulfilled_program_cycles
    ON request_status (fulfill_prover_address, fulfilled_at, program_cycles)
    WHERE request_status = 'fulfilled'
      AND fulfill_prover_address IS NOT NULL
      AND program_cycles IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_request_status_prover_fulfilled_total_cycles
    ON request_status (fulfill_prover_address, fulfilled_at, total_cycles)
    WHERE request_status = 'fulfilled'
      AND fulfill_prover_address IS NOT NULL
      AND total_cycles IS NOT NULL;

-- Optimizes: get_cycle_counts_by_updated_at_range (runs EVERY BLOCK)
-- Query pattern: SELECT request_digest FROM cycle_counts WHERE updated_at >= $1 AND updated_at <= $2
-- Currently does a full seq scan on 80K+ rows every block because there's no index on updated_at.
CREATE INDEX IF NOT EXISTS idx_cycle_counts_updated_at
    ON cycle_counts (updated_at);

-- Optimizes: get_period_all_lock_collateral (market summary aggregation)
-- Query pattern: SELECT pr.lock_collateral FROM request_locked_events rle
--   JOIN proof_requests pr ON rle.request_digest = pr.request_digest
--   WHERE rle.block_timestamp >= $1 AND rle.block_timestamp < $2
-- The JOIN currently seq scans proof_requests (271K+ rows). Adding a covering index
-- on proof_requests allows the join to use an index lookup instead.
CREATE INDEX IF NOT EXISTS idx_proof_requests_digest_lock_collateral
    ON proof_requests (request_digest)
    INCLUDE (lock_collateral);

-- Optimizes: get_period_secondary_fulfillments_count (aggregation)
-- Query pattern: SELECT COUNT(*) FROM request_fulfilled_events rfe
--   WHERE rfe.block_timestamp >= $1 AND rfe.block_timestamp < $2
--   AND NOT EXISTS (SELECT 1 FROM request_locked_events rle
--     WHERE rle.request_digest = rfe.request_digest AND rle.prover_address = rfe.prover_address)
-- The anti-join currently seq scans request_locked_events (518K+ rows).
-- This composite index enables an index lookup for the NOT EXISTS check.
CREATE INDEX IF NOT EXISTS idx_request_locked_events_digest_prover
    ON request_locked_events (request_digest, prover_address);

-- Optimizes: get_period_requestor_total_requests_slashed
-- Query pattern: SELECT COUNT(*) FROM request_status
--   WHERE slashed_at >= $1 AND slashed_at < $2 AND client_address = $3
-- Very few rows are slashed (<1%), so a partial index is tiny but eliminates a full seq scan.
CREATE INDEX IF NOT EXISTS idx_request_status_client_slashed_at
    ON request_status (client_address, slashed_at)
    WHERE slashed_at IS NOT NULL;
