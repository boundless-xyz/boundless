-- Add epoch_number_period_start to all aggregation tables
-- This field stores the epoch number at the start of each aggregation period
-- Calculated from period_timestamp using the EpochCalculator

-- Market summary tables
ALTER TABLE hourly_market_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE daily_market_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE weekly_market_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE monthly_market_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE epoch_market_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE all_time_market_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;

-- Prover summary tables
ALTER TABLE hourly_prover_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE daily_prover_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE weekly_prover_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE monthly_prover_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE epoch_prover_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE all_time_prover_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;

-- Requestor summary tables
ALTER TABLE hourly_requestor_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE daily_requestor_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE weekly_requestor_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE monthly_requestor_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE epoch_requestor_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
ALTER TABLE all_time_requestor_summary ADD COLUMN IF NOT EXISTS epoch_number_period_start BIGINT;
