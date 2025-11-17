CREATE TABLE IF NOT EXISTS cycle_counts (
    request_digest TEXT PRIMARY KEY,
    cycle_status TEXT NOT NULL, -- 'PENDING', 'COMPLETED', 'FAILED'
    program_cycles TEXT,
    total_cycles TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cycle_counts_cycle_status
    ON cycle_counts (cycle_status);

ALTER TABLE request_status ADD COLUMN cycle_status TEXT;
