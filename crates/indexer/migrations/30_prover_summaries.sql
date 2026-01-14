CREATE TABLE IF NOT EXISTS hourly_prover_summary (
  period_timestamp                        BIGINT NOT NULL,
  prover_address                         TEXT NOT NULL,
  total_requests_locked                  BIGINT NOT NULL,
  total_requests_fulfilled               BIGINT NOT NULL,
  total_unique_requestors                BIGINT NOT NULL,
  total_fees_earned                      TEXT NOT NULL,
  total_collateral_locked                TEXT NOT NULL,
  total_collateral_slashed               TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_collateral_earned                TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_requests_locked_and_expired      BIGINT NOT NULL DEFAULT 0,
  total_requests_locked_and_fulfilled    BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate         DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  p10_lock_price_per_cycle               TEXT NOT NULL,
  p25_lock_price_per_cycle               TEXT NOT NULL,
  p50_lock_price_per_cycle               TEXT NOT NULL,
  p75_lock_price_per_cycle               TEXT NOT NULL,
  p90_lock_price_per_cycle               TEXT NOT NULL,
  p95_lock_price_per_cycle               TEXT NOT NULL,
  p99_lock_price_per_cycle               TEXT NOT NULL,
  total_program_cycles                   TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_cycles                           TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  best_peak_prove_mhz                    DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_peak_prove_mhz_request_id         TEXT,
  best_effective_prove_mhz                DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_effective_prove_mhz_request_id     TEXT,
  updated_at                              TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (period_timestamp, prover_address)
);

CREATE INDEX IF NOT EXISTS idx_hourly_prover_period_timestamp ON hourly_prover_summary(period_timestamp DESC, prover_address);
CREATE INDEX IF NOT EXISTS idx_hourly_prover_address ON hourly_prover_summary(prover_address, period_timestamp DESC);

CREATE TABLE IF NOT EXISTS daily_prover_summary (
  period_timestamp                        BIGINT NOT NULL,
  prover_address                         TEXT NOT NULL,
  total_requests_locked                  BIGINT NOT NULL,
  total_requests_fulfilled               BIGINT NOT NULL,
  total_unique_requestors                BIGINT NOT NULL,
  total_fees_earned                      TEXT NOT NULL,
  total_collateral_locked                TEXT NOT NULL,
  total_collateral_slashed               TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_collateral_earned                TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_requests_locked_and_expired      BIGINT NOT NULL DEFAULT 0,
  total_requests_locked_and_fulfilled    BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate         DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  p10_lock_price_per_cycle               TEXT NOT NULL,
  p25_lock_price_per_cycle               TEXT NOT NULL,
  p50_lock_price_per_cycle               TEXT NOT NULL,
  p75_lock_price_per_cycle               TEXT NOT NULL,
  p90_lock_price_per_cycle               TEXT NOT NULL,
  p95_lock_price_per_cycle               TEXT NOT NULL,
  p99_lock_price_per_cycle               TEXT NOT NULL,
  total_program_cycles                   TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_cycles                           TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  best_peak_prove_mhz                    DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_peak_prove_mhz_request_id         TEXT,
  best_effective_prove_mhz                DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_effective_prove_mhz_request_id     TEXT,
  updated_at                              TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (period_timestamp, prover_address)
);

CREATE INDEX IF NOT EXISTS idx_daily_prover_period_timestamp ON daily_prover_summary(period_timestamp DESC, prover_address);
CREATE INDEX IF NOT EXISTS idx_daily_prover_address ON daily_prover_summary(prover_address, period_timestamp DESC);

CREATE TABLE IF NOT EXISTS weekly_prover_summary (
  period_timestamp                        BIGINT NOT NULL,
  prover_address                         TEXT NOT NULL,
  total_requests_locked                  BIGINT NOT NULL,
  total_requests_fulfilled               BIGINT NOT NULL,
  total_unique_requestors                BIGINT NOT NULL,
  total_fees_earned                      TEXT NOT NULL,
  total_collateral_locked                TEXT NOT NULL,
  total_collateral_slashed               TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_collateral_earned                TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_requests_locked_and_expired      BIGINT NOT NULL DEFAULT 0,
  total_requests_locked_and_fulfilled    BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate         DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  p10_lock_price_per_cycle               TEXT NOT NULL,
  p25_lock_price_per_cycle               TEXT NOT NULL,
  p50_lock_price_per_cycle               TEXT NOT NULL,
  p75_lock_price_per_cycle               TEXT NOT NULL,
  p90_lock_price_per_cycle               TEXT NOT NULL,
  p95_lock_price_per_cycle               TEXT NOT NULL,
  p99_lock_price_per_cycle               TEXT NOT NULL,
  total_program_cycles                   TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_cycles                           TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  best_peak_prove_mhz                    DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_peak_prove_mhz_request_id         TEXT,
  best_effective_prove_mhz                DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_effective_prove_mhz_request_id     TEXT,
  updated_at                              TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (period_timestamp, prover_address)
);

CREATE INDEX IF NOT EXISTS idx_weekly_prover_period_timestamp ON weekly_prover_summary(period_timestamp DESC, prover_address);
CREATE INDEX IF NOT EXISTS idx_weekly_prover_address ON weekly_prover_summary(prover_address, period_timestamp DESC);

CREATE TABLE IF NOT EXISTS monthly_prover_summary (
  period_timestamp                        BIGINT NOT NULL,
  prover_address                         TEXT NOT NULL,
  total_requests_locked                  BIGINT NOT NULL,
  total_requests_fulfilled               BIGINT NOT NULL,
  total_unique_requestors                BIGINT NOT NULL,
  total_fees_earned                      TEXT NOT NULL,
  total_collateral_locked                TEXT NOT NULL,
  total_collateral_slashed               TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_collateral_earned                TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_requests_locked_and_expired      BIGINT NOT NULL DEFAULT 0,
  total_requests_locked_and_fulfilled    BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate         DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  p10_lock_price_per_cycle               TEXT NOT NULL,
  p25_lock_price_per_cycle               TEXT NOT NULL,
  p50_lock_price_per_cycle               TEXT NOT NULL,
  p75_lock_price_per_cycle               TEXT NOT NULL,
  p90_lock_price_per_cycle               TEXT NOT NULL,
  p95_lock_price_per_cycle               TEXT NOT NULL,
  p99_lock_price_per_cycle               TEXT NOT NULL,
  total_program_cycles                   TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_cycles                           TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  best_peak_prove_mhz                    DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_peak_prove_mhz_request_id         TEXT,
  best_effective_prove_mhz                DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_effective_prove_mhz_request_id     TEXT,
  updated_at                              TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (period_timestamp, prover_address)
);

CREATE INDEX IF NOT EXISTS idx_monthly_prover_period_timestamp ON monthly_prover_summary(period_timestamp DESC, prover_address);
CREATE INDEX IF NOT EXISTS idx_monthly_prover_address ON monthly_prover_summary(prover_address, period_timestamp DESC);

CREATE TABLE IF NOT EXISTS all_time_prover_summary (
  period_timestamp                        BIGINT NOT NULL,
  prover_address                         TEXT NOT NULL,
  total_requests_locked                  BIGINT NOT NULL,
  total_requests_fulfilled               BIGINT NOT NULL,
  total_unique_requestors                BIGINT NOT NULL,
  total_fees_earned                      TEXT NOT NULL,
  total_collateral_locked                TEXT NOT NULL,
  total_collateral_slashed               TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_collateral_earned                TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_requests_locked_and_expired      BIGINT NOT NULL DEFAULT 0,
  total_requests_locked_and_fulfilled    BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate         DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  total_program_cycles                   TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  total_cycles                           TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000',
  best_peak_prove_mhz                    DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_peak_prove_mhz_request_id         TEXT,
  best_effective_prove_mhz                DOUBLE PRECISION NOT NULL DEFAULT 0.0,
  best_effective_prove_mhz_request_id     TEXT,
  updated_at                              TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (period_timestamp, prover_address)
);

CREATE INDEX IF NOT EXISTS idx_all_time_prover_period_timestamp ON all_time_prover_summary(period_timestamp DESC, prover_address);
CREATE INDEX IF NOT EXISTS idx_all_time_prover_address ON all_time_prover_summary(prover_address, period_timestamp DESC);
