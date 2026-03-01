-- Gas-adjusted market efficiency tables
-- Same schema as base tables; profitability computed with gas costs factored in

-- Gas-adjusted (no exclusions)
CREATE TABLE IF NOT EXISTS market_efficiency_orders_gas_adjusted (
  request_digest TEXT PRIMARY KEY,
  request_id TEXT NOT NULL,
  locked_at BIGINT NOT NULL,
  lock_price TEXT NOT NULL,
  program_cycles TEXT NOT NULL,
  lock_price_per_cycle TEXT NOT NULL,
  num_orders_more_profitable BIGINT NOT NULL,
  num_orders_less_profitable BIGINT NOT NULL,
  num_orders_available_unfulfilled BIGINT NOT NULL,
  is_most_profitable BOOLEAN NOT NULL,
  more_profitable_sample JSONB,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_orders_gas_adjusted_locked_at
  ON market_efficiency_orders_gas_adjusted(locked_at DESC);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_orders_gas_adjusted_is_most_profitable
  ON market_efficiency_orders_gas_adjusted(is_most_profitable, locked_at DESC);

CREATE TABLE IF NOT EXISTS market_efficiency_hourly_gas_adjusted (
  period_timestamp BIGINT PRIMARY KEY,
  num_most_profitable_locked BIGINT NOT NULL,
  num_not_most_profitable_locked BIGINT NOT NULL,
  efficiency_rate DOUBLE PRECISION NOT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_hourly_gas_adjusted_period
  ON market_efficiency_hourly_gas_adjusted(period_timestamp DESC);

CREATE TABLE IF NOT EXISTS market_efficiency_daily_gas_adjusted (
  period_timestamp BIGINT PRIMARY KEY,
  num_most_profitable_locked BIGINT NOT NULL,
  num_not_most_profitable_locked BIGINT NOT NULL,
  efficiency_rate DOUBLE PRECISION NOT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_daily_gas_adjusted_period
  ON market_efficiency_daily_gas_adjusted(period_timestamp DESC);

-- Gas-adjusted with exclusions (excludes signal requestors)
CREATE TABLE IF NOT EXISTS market_efficiency_orders_gas_adjusted_with_exclusions (
  request_digest TEXT PRIMARY KEY,
  request_id TEXT NOT NULL,
  locked_at BIGINT NOT NULL,
  lock_price TEXT NOT NULL,
  program_cycles TEXT NOT NULL,
  lock_price_per_cycle TEXT NOT NULL,
  num_orders_more_profitable BIGINT NOT NULL,
  num_orders_less_profitable BIGINT NOT NULL,
  num_orders_available_unfulfilled BIGINT NOT NULL,
  is_most_profitable BOOLEAN NOT NULL,
  more_profitable_sample JSONB,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_orders_gas_adjusted_with_exclusions_locked_at
  ON market_efficiency_orders_gas_adjusted_with_exclusions(locked_at DESC);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_orders_gas_adjusted_with_exclusions_is_most_profitable
  ON market_efficiency_orders_gas_adjusted_with_exclusions(is_most_profitable, locked_at DESC);

CREATE TABLE IF NOT EXISTS market_efficiency_hourly_gas_adjusted_with_exclusions (
  period_timestamp BIGINT PRIMARY KEY,
  num_most_profitable_locked BIGINT NOT NULL,
  num_not_most_profitable_locked BIGINT NOT NULL,
  efficiency_rate DOUBLE PRECISION NOT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_hourly_gas_adjusted_with_exclusions_period
  ON market_efficiency_hourly_gas_adjusted_with_exclusions(period_timestamp DESC);

CREATE TABLE IF NOT EXISTS market_efficiency_daily_gas_adjusted_with_exclusions (
  period_timestamp BIGINT PRIMARY KEY,
  num_most_profitable_locked BIGINT NOT NULL,
  num_not_most_profitable_locked BIGINT NOT NULL,
  efficiency_rate DOUBLE PRECISION NOT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_market_efficiency_daily_gas_adjusted_with_exclusions_period
  ON market_efficiency_daily_gas_adjusted_with_exclusions(period_timestamp DESC);
