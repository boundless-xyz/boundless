-- Add epoch start and end times to staking summary table
ALTER TABLE epoch_staking_summary
ADD COLUMN epoch_start_time BIGINT NOT NULL DEFAULT 0;

ALTER TABLE epoch_staking_summary
ADD COLUMN epoch_end_time BIGINT NOT NULL DEFAULT 0;