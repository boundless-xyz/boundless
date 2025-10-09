-- Add updated_at timestamps to summary tables
-- Using TEXT for better compatibility with sqlx Any driver

-- Add updated_at to global PoVW summary statistics
ALTER TABLE povw_summary_stats
ADD COLUMN updated_at TEXT;

-- Add updated_at to per-epoch PoVW summary
ALTER TABLE epoch_povw_summary
ADD COLUMN updated_at TEXT;

-- Add updated_at to global staking summary statistics
ALTER TABLE staking_summary_stats
ADD COLUMN updated_at TEXT;

-- Add updated_at to per-epoch staking summary
ALTER TABLE epoch_staking_summary
ADD COLUMN updated_at TEXT;