-- Global PoVW summary statistics
CREATE TABLE IF NOT EXISTS povw_summary_stats (
    id INTEGER PRIMARY KEY,
    total_epochs_with_work BIGINT NOT NULL,
    total_unique_work_log_ids BIGINT NOT NULL,
    total_work_all_time TEXT NOT NULL,
    total_emissions_all_time TEXT NOT NULL,
    total_capped_rewards_all_time TEXT NOT NULL,
    total_uncapped_rewards_all_time TEXT NOT NULL
);

-- Per-epoch PoVW summary
CREATE TABLE IF NOT EXISTS epoch_povw_summary (
    epoch BIGINT PRIMARY KEY,
    total_work TEXT NOT NULL,
    total_emissions TEXT NOT NULL,
    total_capped_rewards TEXT NOT NULL,
    total_uncapped_rewards TEXT NOT NULL,
    epoch_start_time BIGINT NOT NULL,
    epoch_end_time BIGINT NOT NULL,
    num_participants BIGINT NOT NULL
);

-- Global staking summary statistics
CREATE TABLE IF NOT EXISTS staking_summary_stats (
    id INTEGER PRIMARY KEY,
    current_total_staked TEXT NOT NULL,
    total_unique_stakers BIGINT NOT NULL,
    current_active_stakers BIGINT NOT NULL,
    current_withdrawing BIGINT NOT NULL
);

-- Per-epoch staking summary
CREATE TABLE IF NOT EXISTS epoch_staking_summary (
    epoch BIGINT PRIMARY KEY,
    total_staked TEXT NOT NULL,
    num_stakers BIGINT NOT NULL,
    num_withdrawing BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_epoch_povw_summary_epoch ON epoch_povw_summary(epoch);
CREATE INDEX IF NOT EXISTS idx_epoch_staking_summary_epoch ON epoch_staking_summary(epoch);