-- Migration 35: Count Distinct Query Indexes
-- Optimizes slow COUNT DISTINCT queries and fulfilled request status queries

-- For all-time unique requesters query
-- Optimizes: get_all_time_unique_requesters
CREATE INDEX IF NOT EXISTS idx_request_status_created_at_client_address
    ON request_status (created_at, client_address);

-- For all-time unique provers query
-- Optimizes: get_all_time_unique_provers
CREATE INDEX IF NOT EXISTS idx_proof_delivered_events_block_timestamp_prover
    ON proof_delivered_events (block_timestamp, prover_address);

-- For all-time prover unique requestors query (request_locked_events part)
-- Optimizes: get_all_time_prover_unique_requestors (first UNION branch)
CREATE INDEX IF NOT EXISTS idx_request_locked_events_prover_timestamp_digest
    ON request_locked_events (prover_address, block_timestamp, request_digest);

-- For all-time prover unique requestors query (request_fulfilled_events part)
-- Optimizes: get_all_time_prover_unique_requestors (second UNION branch)
CREATE INDEX IF NOT EXISTS idx_request_fulfilled_events_prover_timestamp_digest
    ON request_fulfilled_events (prover_address, block_timestamp, request_digest);

-- For requestor fulfilled lock pricing data query
-- Optimizes: get_period_requestor_lock_pricing_data
CREATE INDEX IF NOT EXISTS idx_request_status_client_fulfilled_provers
    ON request_status (client_address, fulfilled_at, lock_prover_address, fulfill_prover_address)
    WHERE fulfilled_at IS NOT NULL
      AND lock_prover_address IS NOT NULL
      AND fulfill_prover_address IS NOT NULL;

-- For requestor locked and fulfilled count adjusted query
-- Optimizes: get_period_requestor_locked_and_fulfilled_count_adjusted
CREATE INDEX IF NOT EXISTS idx_request_status_client_fulfilled_locked_digest
    ON request_status (client_address, fulfilled_at, locked_at, request_digest)
    WHERE fulfilled_at IS NOT NULL
      AND locked_at IS NOT NULL;
