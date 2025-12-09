CREATE TABLE IF NOT EXISTS all_time_market_summary (
  period_timestamp                        BIGINT PRIMARY KEY,
  total_fulfilled                         BIGINT NOT NULL,
  unique_provers_locking_requests         BIGINT NOT NULL,
  unique_requesters_submitting_requests   BIGINT NOT NULL,
  total_fees_locked                       TEXT NOT NULL,
  total_collateral_locked                 TEXT NOT NULL,
  total_locked_and_expired_collateral     TEXT NOT NULL,
  total_requests_submitted                BIGINT NOT NULL,
  total_requests_submitted_onchain        BIGINT NOT NULL,
  total_requests_submitted_offchain       BIGINT NOT NULL,
  total_requests_locked                   BIGINT NOT NULL,
  total_requests_slashed                  BIGINT NOT NULL,
  total_expired                           BIGINT NOT NULL DEFAULT 0,
  total_locked_and_expired                BIGINT NOT NULL DEFAULT 0,
  total_locked_and_fulfilled              BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate          DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  total_program_cycles                    TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_cycles                            TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  best_peak_prove_mhz                     BIGINT NOT NULL DEFAULT 0,
  best_peak_prove_mhz_prover              TEXT,
  best_peak_prove_mhz_request_id          TEXT,
  best_effective_prove_mhz                BIGINT NOT NULL DEFAULT 0,
  best_effective_prove_mhz_prover         TEXT,
  best_effective_prove_mhz_request_id     TEXT,
  updated_at                              TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_all_time_market_period_timestamp ON all_time_market_summary(period_timestamp DESC);

-- Indexes for efficient unique count queries
-- Check if indexes exist before creating (they may already exist from previous migrations)
CREATE INDEX IF NOT EXISTS idx_request_status_locked_at ON request_status(locked_at) WHERE locked_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_request_status_created_at_for_unique ON request_status(created_at);

