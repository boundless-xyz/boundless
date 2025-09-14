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

//! Commands of the Boundless CLI for ZKC operations.

mod balance_of;
mod calculate_rewards;
mod claim_rewards;
mod delegate_rewards;
mod get_active_token_id;
mod get_current_epoch;
mod get_epoch_end_time;
mod get_rewards_delegates;
mod get_staked_amount;
mod stake;
mod unstake;

pub use balance_of::{balance_of, ZkcBalanceOf};
pub use calculate_rewards::{calculate_rewards, ZkcCalculateRewards};
pub use claim_rewards::{claim_rewards, ZkcClaimRewards};
pub use delegate_rewards::ZkcDelegateRewards;
pub use get_active_token_id::{get_active_token_id, ZkcGetActiveTokenId};
pub use get_current_epoch::{get_current_epoch, ZkcGetCurrentEpoch};
pub use get_epoch_end_time::{get_epoch_end_time, ZkcGetEpochEndTime};
pub use get_rewards_delegates::{get_rewards_delegates, ZkcGetRewardsDelegates};
pub use get_staked_amount::{get_staked_amount, ZkcGetStakedAmount};
pub use stake::ZkcStake;
pub use unstake::ZkcUnstake;

use clap::Subcommand;

use crate::config::GlobalConfig;

/// Commands for ZKC operations.
#[derive(Subcommand, Clone, Debug)]
pub enum ZKCCommands {
    /// Stake ZKC tokens.
    Stake(ZkcStake),
    /// Unstake ZKC tokens.
    Unstake(ZkcUnstake),
    /// Get staked amount and withdrawable time for a specified address.
    GetStakedAmount(ZkcGetStakedAmount),
    /// Delegate rewards to a specified address.
    DelegateRewards(ZkcDelegateRewards),
    /// Get active token ID for a specified address.
    GetActiveTokenId(ZkcGetActiveTokenId),
    /// Get balance for a specified address.
    BalanceOf(ZkcBalanceOf),
    /// Get current epoch for a specified address.
    GetCurrentEpoch(ZkcGetCurrentEpoch),
    /// Get epoch end time for a specified address.
    GetEpochEndTime(ZkcGetEpochEndTime),
    /// Calculate rewards for a specified address.
    CalculateRewards(ZkcCalculateRewards),
    /// Claim rewards for a specified address.
    ClaimRewards(ZkcClaimRewards),
    /// Get rewards delegates for a specified address.
    GetRewardsDelegates(ZkcGetRewardsDelegates),
}

impl ZKCCommands {
    /// Run the command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::Stake(cmd) => cmd.run(global_config).await,
            Self::DelegateRewards(cmd) => cmd.run(global_config).await,
            Self::GetStakedAmount(cmd) => cmd.run(global_config).await,
            Self::GetActiveTokenId(cmd) => cmd.run(global_config).await,
            Self::BalanceOf(cmd) => cmd.run(global_config).await,
            Self::GetCurrentEpoch(cmd) => cmd.run(global_config).await,
            Self::GetEpochEndTime(cmd) => cmd.run(global_config).await,
            Self::Unstake(cmd) => cmd.run(global_config).await,
            Self::CalculateRewards(cmd) => cmd.run(global_config).await,
            Self::ClaimRewards(cmd) => cmd.run(global_config).await,
            Self::GetRewardsDelegates(cmd) => cmd.run(global_config).await,
        }
    }
}
