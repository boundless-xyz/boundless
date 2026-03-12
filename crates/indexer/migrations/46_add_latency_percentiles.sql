-- Add time-to-lock and time-to-fulfill latency percentile columns
-- to all requestor summary tables (hourly, daily, weekly, monthly, epoch).
-- Values are stored as seconds (DOUBLE PRECISION for sub-second interpolation).

-- Hourly
ALTER TABLE hourly_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE hourly_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE hourly_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds DOUBLE PRECISION;
ALTER TABLE hourly_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds DOUBLE PRECISION;

-- Daily
ALTER TABLE daily_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE daily_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE daily_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds DOUBLE PRECISION;
ALTER TABLE daily_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds DOUBLE PRECISION;

-- Weekly
ALTER TABLE weekly_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE weekly_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE weekly_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds DOUBLE PRECISION;
ALTER TABLE weekly_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds DOUBLE PRECISION;

-- Monthly
ALTER TABLE monthly_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE monthly_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE monthly_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds DOUBLE PRECISION;
ALTER TABLE monthly_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds DOUBLE PRECISION;

-- Epoch
ALTER TABLE epoch_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE epoch_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_lock_seconds DOUBLE PRECISION;
ALTER TABLE epoch_requestor_summary ADD COLUMN IF NOT EXISTS p50_time_to_fulfill_seconds DOUBLE PRECISION;
ALTER TABLE epoch_requestor_summary ADD COLUMN IF NOT EXISTS p90_time_to_fulfill_seconds DOUBLE PRECISION;
