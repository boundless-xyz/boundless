-- Staking positions by epoch - historical snapshots
CREATE TABLE IF NOT EXISTS staking_positions_by_epoch (
    staker_address TEXT NOT NULL,
    epoch BIGINT NOT NULL,
    staked_amount TEXT NOT NULL,
    is_withdrawing INTEGER NOT NULL,
    rewards_delegated_to TEXT,
    votes_delegated_to TEXT,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (staker_address, epoch)
);

-- Indexes for efficient querying
CREATE INDEX idx_staking_epoch ON staking_positions_by_epoch(epoch);
CREATE INDEX idx_staking_amount_by_epoch ON staking_positions_by_epoch(epoch, staked_amount DESC);
CREATE INDEX idx_staking_rewards_delegated ON staking_positions_by_epoch(rewards_delegated_to);
CREATE INDEX idx_staking_votes_delegated ON staking_positions_by_epoch(votes_delegated_to);

-- Staking positions aggregate - current state after all events
CREATE TABLE IF NOT EXISTS staking_positions_aggregate (
    staker_address TEXT NOT NULL PRIMARY KEY,
    total_staked TEXT NOT NULL,
    is_withdrawing INTEGER NOT NULL,
    rewards_delegated_to TEXT,
    votes_delegated_to TEXT,
    epochs_participated BIGINT NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Index for leaderboard sorting
CREATE INDEX idx_staking_aggregate_amount ON staking_positions_aggregate(total_staked DESC);
CREATE INDEX idx_staking_aggregate_rewards_delegated ON staking_positions_aggregate(rewards_delegated_to);
CREATE INDEX idx_staking_aggregate_votes_delegated ON staking_positions_aggregate(votes_delegated_to);