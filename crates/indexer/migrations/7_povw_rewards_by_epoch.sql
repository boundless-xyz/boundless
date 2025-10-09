CREATE TABLE IF NOT EXISTS povw_rewards_by_epoch (
    work_log_id TEXT NOT NULL,
    epoch BIGINT NOT NULL,
    work_submitted TEXT NOT NULL,
    percentage DOUBLE PRECISION NOT NULL,
    uncapped_rewards TEXT NOT NULL,
    reward_cap TEXT NOT NULL,
    actual_rewards TEXT NOT NULL,
    is_capped INTEGER NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (work_log_id, epoch)
);

CREATE INDEX idx_povw_rewards_epoch ON povw_rewards_by_epoch(epoch);
CREATE INDEX idx_povw_rewards_actual ON povw_rewards_by_epoch(actual_rewards DESC);
CREATE INDEX idx_povw_rewards_work_log ON povw_rewards_by_epoch(work_log_id);