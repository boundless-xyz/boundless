// Copyright 2026 Boundless Foundation, Inc.
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
mod claim_mining_rewards;
mod claim_staking_rewards;
mod config;
mod delegate;
mod epoch;
mod get_delegate;
mod inspect_mining_state;
mod list_mining_rewards;
mod list_staking_rewards;
mod mining_state;
mod power;
mod prepare_mining;
mod stake_zkc;
mod staked_balance_zkc;
mod submit_mining;

pub use balance_zkc::RewardsBalanceZkc;
pub use claim_mining_rewards::RewardsClaimMiningRewards;
pub use claim_staking_rewards::RewardsClaimStakingRewards;
pub use config::RewardsConfigCmd;
pub use delegate::RewardsDelegate;
pub use epoch::RewardsEpoch;
pub use get_delegate::RewardsGetDelegate;
pub use inspect_mining_state::RewardsInspectMiningState;
pub use list_mining_rewards::RewardsListMiningRewards;
pub use list_staking_rewards::RewardsListStakingRewards;
pub use mining_state::State;
pub use power::RewardsPower;
pub use prepare_mining::RewardsPrepareMining;
pub use stake_zkc::RewardsStakeZkc;
pub use staked_balance_zkc::RewardsStakedBalanceZkc;
pub use submit_mining::RewardsSubmitMining;

use clap::Subcommand;

use crate::{commands::setup::RewardsSetup, config::GlobalConfig};

/// Commands for rewards management
#[derive(Subcommand, Clone, Debug)]
#[command(after_help = "\x1b[1;4mModule Configuration:\x1b[0m
  Run 'boundless rewards setup' for interactive setup

  Alternatively set environment variables:
    \x1b[1mREWARD_RPC_URL\x1b[0m            RPC endpoint for rewards module
    \x1b[1mREWARD_PRIVATE_KEY\x1b[0m        Private key for reward transactions
    \x1b[1mSTAKING_PRIVATE_KEY\x1b[0m       Private key for staking (can differ from reward key)
    \x1b[1mMINING_STATE_FILE\x1b[0m         Path to mining state file (optional)
    \x1b[1mZKC_ADDRESS\x1b[0m               ZKC token contract (optional, has default)
    \x1b[1mVEZKC_ADDRESS\x1b[0m             Staked ZKC NFT contract (optional, has default)
    \x1b[1mSTAKING_REWARDS_ADDRESS\x1b[0m   Rewards distribution contract (optional, has default)
    \x1b[1mBEACON_API_URL\x1b[0m            Beacon API URL (optional)

  Or configure while executing commands:
    Example: \x1b[1mboundless rewards balance-zkc --reward-rpc-url <url> --staking-private-key <key>\x1b[0m")]
pub enum RewardsCommands {
    /// Show rewards configuration status
    Config(RewardsConfigCmd),
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
    /// List mining rewards by epoch
    #[command(name = "list-mining-rewards")]
    ListMiningRewards(RewardsListMiningRewards),
    /// Prepare mining work log update
    #[command(name = "prepare-mining")]
    PrepareMining(RewardsPrepareMining),
    /// Submit mining work updates
    #[command(name = "submit-mining")]
    SubmitMining(RewardsSubmitMining),
    /// Claim mining rewards
    #[command(name = "claim-mining-rewards")]
    ClaimMiningRewards(RewardsClaimMiningRewards),
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
    /// Inspect mining state file and display detailed statistics
    #[command(name = "inspect-mining-state")]
    InspectMiningState(RewardsInspectMiningState),
    /// Interactive setup wizard for rewards configuration
    Setup(RewardsSetup),
}

impl RewardsCommands {
    /// Run the command
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::Config(cmd) => cmd.run(global_config).await,
            Self::StakeZkc(cmd) => cmd.run(global_config).await,
            Self::BalanceZkc(cmd) => cmd.run(global_config).await,
            Self::StakedBalanceZkc(cmd) => cmd.run(global_config).await,
            Self::ListStakingRewards(cmd) => cmd.run(global_config).await,
            Self::ListMiningRewards(cmd) => cmd.run(global_config).await,
            Self::PrepareMining(cmd) => cmd.run(global_config).await,
            Self::SubmitMining(cmd) => cmd.run(global_config).await,
            Self::ClaimMiningRewards(cmd) => cmd.run(global_config).await,
            Self::ClaimStakingRewards(cmd) => cmd.run(global_config).await,
            Self::Delegate(cmd) => cmd.run(global_config).await,
            Self::GetDelegate(cmd) => cmd.run(global_config).await,
            Self::Epoch(cmd) => cmd.run(global_config).await,
            Self::Power(cmd) => cmd.run(global_config).await,
            Self::InspectMiningState(cmd) => cmd.run(global_config).await,
            Self::Setup(cmd) => cmd.run(global_config).await,
        }
    }
}
