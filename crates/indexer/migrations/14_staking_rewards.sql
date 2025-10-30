-- Per-epoch staking rewards (mirrors povw_rewards_by_epoch structure)
CREATE TABLE IF NOT EXISTS staking_rewards_by_epoch (
    staker_address TEXT NOT NULL,
    epoch BIGINT NOT NULL,
    staking_power TEXT NOT NULL,
    percentage DOUBLE PRECISION NOT NULL,
    rewards_earned TEXT NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (staker_address, epoch)
);

-- Create indexes for efficient queries
CREATE INDEX idx_staking_rewards_epoch ON staking_rewards_by_epoch(epoch);
CREATE INDEX idx_staking_rewards_earned ON staking_rewards_by_epoch(rewards_earned DESC);
CREATE INDEX idx_staking_rewards_staker ON staking_rewards_by_epoch(staker_address);

-- Add total rewards to existing staking positions aggregate table
ALTER TABLE staking_positions_aggregate
ADD COLUMN total_rewards_earned TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';

-- Add staking rewards fields to existing epoch staking summary (mirrors epoch_povw_summary)
ALTER TABLE epoch_staking_summary
ADD COLUMN total_staking_emissions TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';
ALTER TABLE epoch_staking_summary
ADD COLUMN total_staking_power TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';
ALTER TABLE epoch_staking_summary
ADD COLUMN num_reward_recipients BIGINT NOT NULL DEFAULT 0;

-- Add staking rewards fields to existing global staking summary (mirrors povw_summary_stats)
ALTER TABLE staking_summary_stats
ADD COLUMN total_staking_emissions_all_time TEXT;