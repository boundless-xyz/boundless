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
mod epoch;
mod get_delegate;
mod inspect_povw_state;
mod list_povw_rewards;
mod list_staking_rewards;
mod power;
mod prepare_povw;
mod stake_zkc;
mod staked_balance_zkc;
mod submit_povw;

pub use balance_zkc::RewardsBalanceZkc;
pub use claim_povw_rewards::RewardsClaimPovwRewards;
pub use claim_staking_rewards::RewardsClaimStakingRewards;
pub use delegate::RewardsDelegate;
pub use epoch::RewardsEpoch;
pub use get_delegate::RewardsGetDelegate;
pub use inspect_povw_state::RewardsInspectPovwState;
pub use list_povw_rewards::RewardsListPovwRewards;
pub use list_staking_rewards::RewardsListStakingRewards;
pub use power::RewardsPower;
pub use prepare_povw::RewardsPreparePoVW;
pub use stake_zkc::RewardsStakeZkc;
pub use staked_balance_zkc::RewardsStakedBalanceZkc;
pub use submit_povw::RewardsSubmitPovw;

use clap::Subcommand;

use crate::{commands::setup::SetupInteractive, config::GlobalConfig};

/// Commands for rewards management
#[derive(Subcommand, Clone, Debug)]
pub enum RewardsCommands {
    /// Stake ZKC tokens
    #[command(name = "stake-zkc")]
    StakeZkc(RewardsStakeZkc),
    /// Check ZKC balance
    #[command(name = "balance-zkc")]
    BalanceZkc(RewardsBalanceZkc),
    /// Check staked ZKC balance
    #[command(name = "staked-balance-zkc")]
    StakedBalanceZkc(RewardsStakedBalanceZkc),
    /// List staking rewards by epoch
    #[command(name = "list-staking-rewards")]
    ListStakingRewards(RewardsListStakingRewards),
    /// List PoVW rewards by epoch
    #[command(name = "list-povw-rewards")]
    ListPovwRewards(RewardsListPovwRewards),
    /// Prepare PoVW work log update
    #[command(name = "prepare-povw")]
    PreparePoVW(RewardsPreparePoVW),
    /// Submit PoVW work updates
    #[command(name = "submit-povw")]
    SubmitPovw(RewardsSubmitPovw),
    /// Claim PoVW rewards
    #[command(name = "claim-povw-rewards")]
    ClaimPovwRewards(RewardsClaimPovwRewards),
    /// Claim staking rewards
    #[command(name = "claim-staking-rewards")]
    ClaimStakingRewards(RewardsClaimStakingRewards),
    /// Delegate rewards to another address
    Delegate(RewardsDelegate),
    /// Get rewards delegate
    #[command(name = "get-delegate")]
    GetDelegate(RewardsGetDelegate),
    /// Get current epoch information
    Epoch(RewardsEpoch),
    /// Check reward power and earning potential
    Power(RewardsPower),
    /// Inspect PoVW state file and display detailed statistics
    #[command(name = "inspect-povw-state")]
    InspectPovwState(RewardsInspectPovwState),
    /// Interactive setup wizard for rewards configuration
    Setup(SetupInteractive),
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
            Self::PreparePoVW(cmd) => cmd.run(global_config).await,
            Self::SubmitPovw(cmd) => cmd.run(global_config).await,
            Self::ClaimPovwRewards(cmd) => cmd.run(global_config).await,
            Self::ClaimStakingRewards(cmd) => cmd.run(global_config).await,
            Self::Delegate(cmd) => cmd.run(global_config).await,
            Self::GetDelegate(cmd) => cmd.run(global_config).await,
            Self::Epoch(cmd) => cmd.run(global_config).await,
            Self::Power(cmd) => cmd.run(global_config).await,
            Self::InspectPovwState(cmd) => cmd.run(global_config).await,
            Self::Setup(cmd) => cmd.run_module(global_config, "rewards").await,
        }
    }
}
