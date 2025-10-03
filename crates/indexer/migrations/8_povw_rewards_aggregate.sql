CREATE TABLE IF NOT EXISTS povw_rewards_aggregate (
    work_log_id TEXT PRIMARY KEY,
    total_work_submitted TEXT NOT NULL,
    total_actual_rewards TEXT NOT NULL,
    total_uncapped_rewards TEXT NOT NULL,
    epochs_participated BIGINT DEFAULT 0,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_povw_aggregate_rewards ON povw_rewards_aggregate(total_actual_rewards DESC);
CREATE INDEX idx_povw_aggregate_work ON povw_rewards_aggregate(total_work_submitted DESC);

-- Table to store indexer state (current epoch, last processed block, etc)
CREATE TABLE IF NOT EXISTS indexer_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);