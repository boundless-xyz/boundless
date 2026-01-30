-- Migration 36: Request status count-distinct indexes
-- Optimizes requestor/prover aggregate queries over request_status

CREATE INDEX IF NOT EXISTS idx_request_status_lock_prover_locked_at_client
    ON request_status (lock_prover_address, locked_at, client_address)
    WHERE lock_prover_address IS NOT NULL
      AND locked_at IS NOT NULL;

-- Optimizes: get_period_prover_unique_requestors, get_all_time_prover_unique_requestors
CREATE INDEX IF NOT EXISTS idx_request_status_fulfill_prover_fulfilled_at_client
    ON request_status (fulfill_prover_address, fulfilled_at, client_address)
    WHERE fulfill_prover_address IS NOT NULL
      AND fulfilled_at IS NOT NULL;

-- Optimizes: get_period_requestor_unique_provers, get_all_time_requestor_unique_provers
CREATE INDEX IF NOT EXISTS idx_request_status_client_locked_at_lock_prover
    ON request_status (client_address, locked_at, lock_prover_address)
    WHERE locked_at IS NOT NULL
      AND lock_prover_address IS NOT NULL;

-- Optimizes: get_period_requestor_locked_and_fulfilled_count_adjusted, get_all_time_requestor_locked_and_fulfilled_count_adjusted
CREATE INDEX IF NOT EXISTS idx_request_status_client_fulfilled_input_image
    ON request_status (client_address, fulfilled_at, input_data, image_url)
    WHERE fulfilled_at IS NOT NULL
      AND locked_at IS NOT NULL;

-- Optimizes: get_period_requestor_locked_and_expired_count_adjusted, get_all_time_requestor_locked_and_expired_count_adjusted
CREATE INDEX IF NOT EXISTS idx_request_status_client_expired_input_image
    ON request_status (client_address, expires_at, input_data, image_url)
    WHERE request_status = 'expired'
      AND locked_at IS NOT NULL;
