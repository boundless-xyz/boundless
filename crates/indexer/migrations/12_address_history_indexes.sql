-- These composite indexes optimize queries that filter by address and optionally by epoch range

-- Index for staking history by address
CREATE INDEX IF NOT EXISTS idx_staking_address_epoch
ON staking_positions_by_epoch(staker_address, epoch DESC);

-- Index for PoVW rewards history by address
CREATE INDEX IF NOT EXISTS idx_povw_rewards_address_epoch
ON povw_rewards_by_epoch(work_log_id, epoch DESC);

-- Index for vote delegations received by delegate address
CREATE INDEX IF NOT EXISTS idx_vote_delegation_delegate_epoch
ON vote_delegation_powers_by_epoch(delegate_address, epoch DESC);

-- Index for reward delegations received by delegate address
CREATE INDEX IF NOT EXISTS idx_reward_delegation_delegate_epoch
ON reward_delegation_powers_by_epoch(delegate_address, epoch DESC);