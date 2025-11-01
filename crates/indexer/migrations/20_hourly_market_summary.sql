CREATE TABLE IF NOT EXISTS hourly_market_summary (
  hour_timestamp                          BIGINT PRIMARY KEY,
  total_fulfilled                         BIGINT NOT NULL,
  unique_provers_locking_requests         BIGINT NOT NULL,
  unique_requesters_submitting_requests   BIGINT NOT NULL,
  total_fees_locked                       TEXT NOT NULL,
  total_collateral_locked                 TEXT NOT NULL,
  p50_fees_locked                         TEXT NOT NULL,
  percentile_fees_locked                  BYTEA NOT NULL,
  updated_at                              TIMESTAMP DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_hourly_market_timestamp ON hourly_market_summary(hour_timestamp DESC);
