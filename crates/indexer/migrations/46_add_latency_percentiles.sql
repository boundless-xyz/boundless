-- Add time-to-lock and time-to-fulfill latency percentile columns to market summary tables

ALTER TABLE hourly_market_summary
    ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds BIGINT DEFAULT 0;

ALTER TABLE daily_market_summary
    ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds BIGINT DEFAULT 0;

ALTER TABLE weekly_market_summary
    ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds BIGINT DEFAULT 0;

ALTER TABLE monthly_market_summary
    ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds BIGINT DEFAULT 0;

ALTER TABLE epoch_market_summary
    ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds BIGINT DEFAULT 0;
