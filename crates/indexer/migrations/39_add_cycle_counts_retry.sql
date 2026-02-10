-- Add retry columns to cycle_counts for scheduled retries with backoff

ALTER TABLE cycle_counts
    ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;

ALTER TABLE cycle_counts
    ADD COLUMN retry_after BIGINT;

ALTER TABLE cycle_counts
    ADD COLUMN last_error TEXT;
