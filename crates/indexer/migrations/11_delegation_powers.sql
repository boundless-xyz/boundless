-- Vote delegation powers by epoch - historical snapshots
CREATE TABLE IF NOT EXISTS vote_delegation_powers_by_epoch (
    delegate_address TEXT NOT NULL,
    epoch BIGINT NOT NULL,
    vote_power TEXT NOT NULL,
    delegator_count INTEGER NOT NULL,
    delegators TEXT,  -- JSON array of delegator addresses
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (delegate_address, epoch)
);

-- Indexes for efficient querying
CREATE INDEX idx_vote_delegation_epoch ON vote_delegation_powers_by_epoch(epoch);
CREATE INDEX idx_vote_delegation_power_by_epoch ON vote_delegation_powers_by_epoch(epoch, vote_power DESC);

-- Reward delegation powers by epoch - historical snapshots
CREATE TABLE IF NOT EXISTS reward_delegation_powers_by_epoch (
    delegate_address TEXT NOT NULL,
    epoch BIGINT NOT NULL,
    reward_power TEXT NOT NULL,
    delegator_count INTEGER NOT NULL,
    delegators TEXT,  -- JSON array of delegator addresses
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (delegate_address, epoch)
);

-- Indexes for efficient querying
CREATE INDEX idx_reward_delegation_epoch ON reward_delegation_powers_by_epoch(epoch);
CREATE INDEX idx_reward_delegation_power_by_epoch ON reward_delegation_powers_by_epoch(epoch, reward_power DESC);

-- Vote delegation powers aggregate - current state after all events
CREATE TABLE IF NOT EXISTS vote_delegation_powers_aggregate (
    delegate_address TEXT NOT NULL PRIMARY KEY,
    total_vote_power TEXT NOT NULL,
    delegator_count INTEGER NOT NULL,
    delegators TEXT,  -- JSON array of delegator addresses
    epochs_participated BIGINT NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Index for leaderboard sorting
CREATE INDEX idx_vote_delegation_aggregate_power ON vote_delegation_powers_aggregate(total_vote_power DESC);

-- Reward delegation powers aggregate - current state after all events
CREATE TABLE IF NOT EXISTS reward_delegation_powers_aggregate (
    delegate_address TEXT NOT NULL PRIMARY KEY,
    total_reward_power TEXT NOT NULL,
    delegator_count INTEGER NOT NULL,
    delegators TEXT,  -- JSON array of delegator addresses
    epochs_participated BIGINT NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Index for leaderboard sorting
CREATE INDEX idx_reward_delegation_aggregate_power ON reward_delegation_powers_aggregate(total_reward_power DESC);