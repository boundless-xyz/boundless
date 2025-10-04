// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Commands for managing ZKC rewards, staking, and PoVW.

mod balance_zkc;
mod claim_povw_rewards;
mod claim_staking_rewards;
mod delegate;
mod get_current_epoch;
mod get_delegate;
mod list_povw_rewards;
mod list_staking_rewards;
mod stake_zkc;
mod staked_balance_zkc;
mod submit_povw;

pub use balance_zkc::RewardsBalanceZkc;
pub use claim_povw_rewards::RewardsClaimPovwRewards;
pub use claim_staking_rewards::RewardsClaimStakingRewards;
pub use delegate::RewardsDelegate;
pub use get_current_epoch::RewardsGetCurrentEpoch;
pub use get_delegate::RewardsGetDelegate;
pub use list_povw_rewards::RewardsListPovwRewards;
pub use list_staking_rewards::RewardsListStakingRewards;
pub use stake_zkc::RewardsStakeZkc;
pub use staked_balance_zkc::RewardsStakedBalanceZkc;
pub use submit_povw::RewardsSubmitPovw;

use clap::Subcommand;

use crate::config::GlobalConfig;

/// Commands for rewards management
#[derive(Subcommand, Clone, Debug)]
pub enum RewardsCommands {
    /// Stake ZKC tokens
    #[command(name = "stake-zkc")]
    StakeZkc(RewardsStakeZkc),
    /// Get ZKC balance for a specified address
    #[command(name = "balance-zkc")]
    BalanceZkc(RewardsBalanceZkc),
    /// Get staked ZKC balance for a specified address
    #[command(name = "staked-balance-zkc")]
    StakedBalanceZkc(RewardsStakedBalanceZkc),
    /// List historical staking rewards for an address
    #[command(name = "list-staking-rewards")]
    ListStakingRewards(RewardsListStakingRewards),
    /// List historical PoVW rewards for an address
    #[command(name = "list-povw-rewards")]
    ListPovwRewards(RewardsListPovwRewards),
    /// Submit a work log update to the PoVW accounting contract
    #[command(name = "submit-povw")]
    SubmitPovw(RewardsSubmitPovw),
    /// Claim PoVW rewards associated with submitted work log updates
    #[command(name = "claim-povw-rewards")]
    ClaimPovwRewards(RewardsClaimPovwRewards),
    /// Claim staking rewards for a specified address
    #[command(name = "claim-staking-rewards")]
    ClaimStakingRewards(RewardsClaimStakingRewards),
    /// Delegate rewards to a specified address
    Delegate(RewardsDelegate),
    /// Get rewards delegates for a specified address
    #[command(name = "get-delegate")]
    GetDelegate(RewardsGetDelegate),
    /// Get current epoch information
    #[command(name = "get-current-epoch")]
    GetCurrentEpoch(RewardsGetCurrentEpoch),
}

impl RewardsCommands {
    /// Run the command
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::StakeZkc(cmd) => cmd.run(global_config).await,
            Self::BalanceZkc(cmd) => cmd.run(global_config).await,
            Self::StakedBalanceZkc(cmd) => cmd.run(global_config).await,
            Self::ListStakingRewards(cmd) => cmd.run(global_config).await,
            Self::ListPovwRewards(cmd) => cmd.run(global_config).await,
            Self::SubmitPovw(cmd) => cmd.run(global_config).await,
            Self::ClaimPovwRewards(cmd) => cmd.run(global_config).await,
            Self::ClaimStakingRewards(cmd) => cmd.run(global_config).await,
            Self::Delegate(cmd) => cmd.run(global_config).await,
            Self::GetDelegate(cmd) => cmd.run(global_config).await,
            Self::GetCurrentEpoch(cmd) => cmd.run(global_config).await,
        }
    }
}
