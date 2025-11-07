CREATE TABLE IF NOT EXISTS hourly_market_summary (
  period_timestamp                        BIGINT PRIMARY KEY,
  total_fulfilled                         BIGINT NOT NULL,
  unique_provers_locking_requests         BIGINT NOT NULL,
  unique_requesters_submitting_requests   BIGINT NOT NULL,
  total_fees_locked                       TEXT NOT NULL,
  total_collateral_locked                 TEXT NOT NULL,
  p10_fees_locked                         TEXT NOT NULL,
  p25_fees_locked                         TEXT NOT NULL,
  p50_fees_locked                         TEXT NOT NULL,
  p75_fees_locked                         TEXT NOT NULL,
  p90_fees_locked                         TEXT NOT NULL,
  p95_fees_locked                         TEXT NOT NULL,
  p99_fees_locked                         TEXT NOT NULL,
  total_requests_submitted                BIGINT NOT NULL,
  total_requests_submitted_onchain        BIGINT NOT NULL,
  total_requests_submitted_offchain       BIGINT NOT NULL,
  total_requests_locked                   BIGINT NOT NULL,
  total_requests_slashed                  BIGINT NOT NULL,
  total_expired                           BIGINT NOT NULL DEFAULT 0,
  total_locked_and_expired                BIGINT NOT NULL DEFAULT 0,
  total_locked_and_fulfilled              BIGINT NOT NULL DEFAULT 0,
  locked_orders_fulfillment_rate          REAL NOT NULL DEFAULT 0.0,
  updated_at                              TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_hourly_market_period_timestamp ON hourly_market_summary(period_timestamp DESC);
