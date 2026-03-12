-- Market efficiency tracking tables
-- Tracks whether the most profitable orders are being locked first

-- Per-order debugging table
CREATE TABLE IF NOT EXISTS market_efficiency_orders (
  request_digest TEXT PRIMARY KEY,
  request_id TEXT NOT NULL,
  locked_at BIGINT NOT NULL,
  lock_price TEXT NOT NULL,
  program_cycles TEXT NOT NULL,
  lock_price_per_cycle TEXT NOT NULL,
  
  -- Efficiency metrics
  num_orders_more_profitable BIGINT NOT NULL,
  num_orders_less_profitable BIGINT NOT NULL,
  num_orders_available_unfulfilled BIGINT NOT NULL,
  is_most_profitable BOOLEAN NOT NULL,
  
  -- Top 5 more profitable orders (JSON)
  more_profitable_sample JSONB,
  
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_orders_locked_at 
  ON market_efficiency_orders(locked_at DESC);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_orders_is_most_profitable 
  ON market_efficiency_orders(is_most_profitable, locked_at DESC);

-- Hourly aggregation table
CREATE TABLE IF NOT EXISTS market_efficiency_hourly (
  period_timestamp BIGINT PRIMARY KEY,
  num_most_profitable_locked BIGINT NOT NULL,
  num_not_most_profitable_locked BIGINT NOT NULL,
  efficiency_rate DOUBLE PRECISION NOT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_hourly_period 
  ON market_efficiency_hourly(period_timestamp DESC);

-- Daily aggregation table
CREATE TABLE IF NOT EXISTS market_efficiency_daily (
  period_timestamp BIGINT PRIMARY KEY,
  num_most_profitable_locked BIGINT NOT NULL,
  num_not_most_profitable_locked BIGINT NOT NULL,
  efficiency_rate DOUBLE PRECISION NOT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_daily_period 
  ON market_efficiency_daily(period_timestamp DESC);

-- Track last processed timestamp for incremental processing
CREATE TABLE IF NOT EXISTS market_efficiency_state (
  id INTEGER PRIMARY KEY DEFAULT 0,
  last_processed_locked_at BIGINT NOT NULL DEFAULT 0,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT market_efficiency_state_singleton CHECK (id = 0)
);

-- Insert default state row
INSERT INTO market_efficiency_state (id, last_processed_locked_at) 
VALUES (0, 0) 
ON CONFLICT (id) DO NOTHING;
