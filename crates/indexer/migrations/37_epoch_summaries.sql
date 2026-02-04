-- Epoch-based summary tables
-- Uses period_timestamp to store epoch start time (Unix timestamp) for reuse with generic DB queries
-- Epoch number is calculated from timestamp using EpochCalculator when needed

-- Epoch-based market summary table
CREATE TABLE IF NOT EXISTS epoch_market_summary (
  period_timestamp                          BIGINT PRIMARY KEY,
  total_fulfilled                           BIGINT NOT NULL,
  unique_provers_locking_requests           BIGINT NOT NULL,
  unique_requesters_submitting_requests     BIGINT NOT NULL,
  total_fees_locked                         TEXT NOT NULL,
  total_collateral_locked                   TEXT NOT NULL,
  total_locked_and_expired_collateral       TEXT NOT NULL,
  p10_lock_price_per_cycle                  TEXT NOT NULL,
  p25_lock_price_per_cycle                  TEXT NOT NULL,
  p50_lock_price_per_cycle                  TEXT NOT NULL,
  p75_lock_price_per_cycle                  TEXT NOT NULL,
  p90_lock_price_per_cycle                  TEXT NOT NULL,
  p95_lock_price_per_cycle                  TEXT NOT NULL,
  p99_lock_price_per_cycle                  TEXT NOT NULL,
  total_requests_submitted                  BIGINT NOT NULL,
  total_requests_submitted_onchain          BIGINT NOT NULL,
  total_requests_submitted_offchain         BIGINT NOT NULL,
  total_requests_locked                     BIGINT NOT NULL,
  total_requests_slashed                    BIGINT NOT NULL,
  total_expired                             BIGINT NOT NULL DEFAULT 0,
  total_locked_and_expired                  BIGINT NOT NULL DEFAULT 0,
  total_locked_and_fulfilled                BIGINT NOT NULL DEFAULT 0,
  total_secondary_fulfillments              BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate            DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  total_program_cycles                      TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_cycles                              TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  best_peak_prove_mhz                       DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_peak_prove_mhz_prover                TEXT,
  best_peak_prove_mhz_request_id            TEXT,
  best_effective_prove_mhz                  DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_effective_prove_mhz_prover           TEXT,
  best_effective_prove_mhz_request_id       TEXT,
  best_peak_prove_mhz_v2                    DOUBLE PRECISION,
  best_effective_prove_mhz_v2               DOUBLE PRECISION,
  updated_at                                TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_epoch_market_period_timestamp ON epoch_market_summary(period_timestamp DESC);

-- Epoch-based prover summary table
CREATE TABLE IF NOT EXISTS epoch_prover_summary (
  period_timestamp                          BIGINT NOT NULL,
  prover_address                            TEXT NOT NULL,
  total_requests_locked                     BIGINT NOT NULL,
  total_requests_fulfilled                  BIGINT NOT NULL,
  total_unique_requestors                   BIGINT NOT NULL,
  total_fees_earned                         TEXT NOT NULL,
  total_collateral_locked                   TEXT NOT NULL,
  total_collateral_slashed                  TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_collateral_earned                   TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_requests_locked_and_expired         BIGINT NOT NULL DEFAULT 0,
  total_requests_locked_and_fulfilled       BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate            DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  p10_lock_price_per_cycle                  TEXT NOT NULL,
  p25_lock_price_per_cycle                  TEXT NOT NULL,
  p50_lock_price_per_cycle                  TEXT NOT NULL,
  p75_lock_price_per_cycle                  TEXT NOT NULL,
  p90_lock_price_per_cycle                  TEXT NOT NULL,
  p95_lock_price_per_cycle                  TEXT NOT NULL,
  p99_lock_price_per_cycle                  TEXT NOT NULL,
  total_program_cycles                      TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_cycles                              TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  best_peak_prove_mhz                       DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_peak_prove_mhz_request_id            TEXT,
  best_effective_prove_mhz                  DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_effective_prove_mhz_request_id       TEXT,
  updated_at                                TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (period_timestamp, prover_address)
);

CREATE INDEX IF NOT EXISTS idx_epoch_prover_period_timestamp ON epoch_prover_summary(period_timestamp DESC, prover_address);
CREATE INDEX IF NOT EXISTS idx_epoch_prover_address ON epoch_prover_summary(prover_address, period_timestamp DESC);

-- Epoch-based requestor summary table
CREATE TABLE IF NOT EXISTS epoch_requestor_summary (
  period_timestamp                          BIGINT NOT NULL,
  requestor_address                         TEXT NOT NULL,
  total_fulfilled                           BIGINT NOT NULL,
  unique_provers_locking_requests           BIGINT NOT NULL,
  total_fees_locked                         TEXT NOT NULL,
  total_collateral_locked                   TEXT NOT NULL,
  total_locked_and_expired_collateral       TEXT NOT NULL,
  p10_lock_price_per_cycle                  TEXT NOT NULL,
  p25_lock_price_per_cycle                  TEXT NOT NULL,
  p50_lock_price_per_cycle                  TEXT NOT NULL,
  p75_lock_price_per_cycle                  TEXT NOT NULL,
  p90_lock_price_per_cycle                  TEXT NOT NULL,
  p95_lock_price_per_cycle                  TEXT NOT NULL,
  p99_lock_price_per_cycle                  TEXT NOT NULL,
  total_requests_submitted                  BIGINT NOT NULL,
  total_requests_submitted_onchain          BIGINT NOT NULL,
  total_requests_submitted_offchain         BIGINT NOT NULL,
  total_requests_locked                     BIGINT NOT NULL,
  total_requests_slashed                    BIGINT NOT NULL,
  total_expired                             BIGINT NOT NULL DEFAULT 0,
  total_locked_and_expired                  BIGINT NOT NULL DEFAULT 0,
  total_locked_and_fulfilled                BIGINT NOT NULL DEFAULT 0,
  total_secondary_fulfillments              BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate            DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  locked_orders_fulfillment_rate_adjusted   DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  total_program_cycles                      TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_cycles                              TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  best_peak_prove_mhz                       DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_peak_prove_mhz_prover                TEXT,
  best_peak_prove_mhz_request_id            TEXT,
  best_effective_prove_mhz                  DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_effective_prove_mhz_prover           TEXT,
  best_effective_prove_mhz_request_id       TEXT,
  best_peak_prove_mhz_v2                    DOUBLE PRECISION,
  best_effective_prove_mhz_v2               DOUBLE PRECISION,
  updated_at                                TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (period_timestamp, requestor_address)
);

CREATE INDEX IF NOT EXISTS idx_epoch_requestor_period_timestamp ON epoch_requestor_summary(period_timestamp DESC, requestor_address);
CREATE INDEX IF NOT EXISTS idx_epoch_requestor_address ON epoch_requestor_summary(requestor_address, period_timestamp DESC);
