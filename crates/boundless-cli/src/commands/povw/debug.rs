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

use alloy::{
    primitives::{utils::format_ether, Address, B256, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::{BlockNumberOrTag, Filter},
    signers::local::PrivateKeySigner,
    sol_types::SolEvent,
};
use anyhow::Context;
use boundless_povw::{deployments::Deployment, log_updater::IPovwAccounting};
use boundless_zkc::contracts::{IRewards, IStaking, IStakingRewards, IZKC};
use clap::Args;
use std::collections::HashMap;

use crate::config::GlobalConfig;

/// Debug command for PoVW operations - queries and displays comprehensive staking and work information.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct PovwDebug {
    /// Configuration for the PoVW deployment to use.
    #[clap(flatten, next_help_heading = "PoVW Deployment")]
    pub deployment: Option<Deployment>,

    /// Account address to query for (defaults to address of private key if set).
    #[clap(long)]
    pub account: Option<Address>,
}

impl PovwDebug {
    /// Run the [PovwDebug] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let rpc_url = global_config.require_rpc_url()?;

        // Determine the account to use
        let account = match self.account {
            Some(addr) => addr,
            None => {
                // Try to use the address from the private key
                let private_key = global_config.require_private_key()
                    .context("No account address provided. Please specify --account or set --private-key/PRIVATE_KEY")?;
                private_key.address()
            }
        };

        // Connect to the chain
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;

        // Get deployment configuration
        let deployment = self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id))
            .context("could not determine PoVW deployment from chain ID; please specify deployment explicitly")?;

        println!("\n=== PoVW Debug Information ===");
        println!("Account: {:#x}", account);
        println!("Chain ID: {}", chain_id);

        // Query veZKC staking information
        self.query_staking_info(&provider, &deployment, account).await?;

        // Query staking rewards
        self.query_staking_rewards(&provider, &deployment, account).await?;

        // Query vote and reward delegation information
        self.query_delegation_info(&provider, &deployment).await?;

        // Query stake positions
        self.query_stake_positions(&provider, &deployment).await?;

        // Query PoVW work information
        self.query_povw_work(&provider, &deployment, account).await?;

        Ok(())
    }

    async fn query_staking_info<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        account: Address,
    ) -> anyhow::Result<()> {
        println!("\n--- Personal Staking Information ---");

        // Get staked amount and withdrawal time
        let staking = IStaking::new(deployment.vezkc_address, provider);
        let result = staking
            .getStakedAmountAndWithdrawalTime(account)
            .call()
            .await?;
        let staked_amount = result.amount;
        let withdrawal_time = result.withdrawableAt;

        println!("Staked ZKC: {} ZKC", format_ether(staked_amount));
        if withdrawal_time > U256::ZERO {
            let current_time = provider.get_block_number().await?;
            println!("Withdrawal initiated, available at block: {}", withdrawal_time);
            if withdrawal_time <= U256::from(current_time) {
                println!("  ✓ Withdrawal available now");
            }
        }

        let rewards = IRewards::new(deployment.vezkc_address, provider);
        let reward_cap = rewards.getPoVWRewardCap(account).call().await?;

        println!("\n┌─────────────────────────────────────────────────────────────────┐");
        println!("│ PoVW Reward Cap for {:#x}          │", account);
        println!("│ Maximum PoVW rewards per epoch: {:>31} ZKC │", format_ether(reward_cap));
        println!("└─────────────────────────────────────────────────────────────────┘");

        Ok(())
    }

    async fn query_staking_rewards<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        account: Address,
    ) -> anyhow::Result<()> {
        println!("\n--- Staking Rewards ---");

        // Get staking rewards address from ZKC deployment
        let zkc_deployment = boundless_zkc::deployments::Deployment::from_chain_id(
            provider.get_chain_id().await?
        ).context("Could not determine ZKC deployment")?;

        let staking_rewards = IStakingRewards::new(zkc_deployment.staking_rewards_address, provider);

        // Get current epoch
        let current_epoch: u32 = staking_rewards.getCurrentEpoch().call().await?.try_into()?;

        // Calculate unclaimed rewards for all epochs
        let epochs: Vec<U256> = (0..current_epoch).map(U256::from).collect();
        let unclaimed_rewards = staking_rewards.calculateUnclaimedRewards(account, epochs.clone()).call().await?;

        // Find epochs with unclaimed rewards
        let mut unclaimed_epochs = vec![];
        for (i, reward) in unclaimed_rewards.iter().enumerate() {
            if *reward > U256::ZERO {
                unclaimed_epochs.push(U256::from(i));
            }
        }

        if !unclaimed_epochs.is_empty() {
            let rewards = staking_rewards.calculateRewards(account, unclaimed_epochs.clone()).call().await?;
            let total: U256 = rewards.iter().sum();
            println!("Unclaimed staking rewards: {} ZKC across {} epochs", format_ether(total), unclaimed_epochs.len());
        } else {
            println!("No unclaimed staking rewards");
        }

        // Get expected staking rewards using calculateRewards for current epoch
        // Note: This will return 0 if the epoch hasn't ended yet
        let current_epoch_vec = vec![U256::from(current_epoch)];
        let calculated_rewards = staking_rewards.calculateRewards(account, current_epoch_vec).call().await?;
        let pending_rewards = calculated_rewards.first().cloned().unwrap_or(U256::ZERO);

        // Get total staking emissions for context
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let total_staking_emissions = zkc.getStakingEmissionsForEpoch(U256::from(current_epoch)).call().await?;

        // Get total staking rewards to calculate percentage
        let rewards = IRewards::new(deployment.vezkc_address, provider);
        let user_reward_power = rewards.getStakingRewards(account).call().await?;
        let total_reward_power = rewards.getTotalStakingRewards().call().await?;

        let user_percentage = if total_reward_power > U256::ZERO {
            (user_reward_power * U256::from(10000) / total_reward_power).to::<u64>() as f64 / 100.0
        } else {
            0.0
        };

        let expected_rewards = if total_reward_power > U256::ZERO {
            total_staking_emissions * user_reward_power / total_reward_power
        } else {
            U256::ZERO
        };

        println!("\nExpected staking rewards for epoch {}:", current_epoch);
        println!("  Total epoch emissions: {} ZKC", format_ether(total_staking_emissions));
        println!("  Your reward power: {} / {} ({:.2}%)",
            format_ether(user_reward_power), format_ether(total_reward_power), user_percentage);
        if pending_rewards > U256::ZERO {
            println!("  Your calculated rewards (epoch ended): {} ZKC", format_ether(pending_rewards));
        } else {
            println!("  Your expected rewards (epoch ongoing): {} ZKC", format_ether(expected_rewards));
        }
        println!("  Calculation: {} × {:.2}% = {} ZKC",
            format_ether(total_staking_emissions), user_percentage, format_ether(expected_rewards));

        Ok(())
    }

    async fn query_delegation_info<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
    ) -> anyhow::Result<()> {
        println!("\n--- Delegation Tables ---");

        // Query DelegateVotesChanged events
        // Event signature: DelegateVotesChanged(address indexed delegate, uint256 previousVotes, uint256 newVotes)
        let vote_event_sig = B256::from(alloy::primitives::keccak256(
            "DelegateVotesChanged(address,uint256,uint256)"
        ));
        let vote_filter = Filter::new()
            .address(deployment.vezkc_address)
            .event_signature(vote_event_sig)
            .from_block(BlockNumberOrTag::Earliest);

        let vote_logs = provider.get_logs(&vote_filter).await?;
        let mut vote_powers: HashMap<Address, U256> = HashMap::new();

        for log in vote_logs {
            if log.topics().len() >= 2 {
                let delegate = Address::from_slice(&log.topics()[1][12..]);
                let data_bytes = &log.data().data;
                if data_bytes.len() >= 64 {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(&data_bytes[32..64]);
                    let new_votes = U256::from_be_bytes(bytes);
                    vote_powers.insert(delegate, new_votes);
                }
            }
        }

        // Display vote power table
        if !vote_powers.is_empty() {
            // Calculate total vote power
            let total_vote_power: U256 = vote_powers.values().sum();

            println!("\nVote Powers (Total: {} ZKC):", format_ether(total_vote_power));
            println!("{:<44} | {:<20} | {:<10}", "Address", "Vote Power", "Percentage");
            println!("{}", "-".repeat(78));

            let mut sorted_votes: Vec<_> = vote_powers.iter().collect();
            sorted_votes.sort_by(|a, b| b.1.cmp(a.1));

            for (address, power) in sorted_votes.iter() {
                let percentage = if total_vote_power > U256::ZERO {
                    (**power * U256::from(10000) / total_vote_power).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                println!("{:#x} | {:<19} ZKC | {:>9.2}%", address, format_ether(**power), percentage);
            }
        }

        // Query DelegateRewardsChanged events
        // Event signature: DelegateRewardsChanged(address indexed delegate, uint256 previousRewards, uint256 newRewards)
        let rewards_event_sig = B256::from(alloy::primitives::keccak256(
            "DelegateRewardsChanged(address,uint256,uint256)"
        ));
        let rewards_filter = Filter::new()
            .address(deployment.vezkc_address)
            .event_signature(rewards_event_sig)
            .from_block(BlockNumberOrTag::Earliest);

        let rewards_logs = provider.get_logs(&rewards_filter).await?;
        let mut reward_powers: HashMap<Address, U256> = HashMap::new();

        for log in rewards_logs {
            if log.topics().len() >= 2 {
                let delegate = Address::from_slice(&log.topics()[1][12..]);
                let data_bytes = &log.data().data;
                if data_bytes.len() >= 64 {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(&data_bytes[32..64]);
                    let new_rewards = U256::from_be_bytes(bytes);
                    reward_powers.insert(delegate, new_rewards);
                }
            }
        }

        // Display reward power table
        if !reward_powers.is_empty() {
            // Calculate total reward power
            let total_reward_power: U256 = reward_powers.values().sum();

            println!("\nReward Powers (Total: {} ZKC):", format_ether(total_reward_power));
            println!("{:<44} | {:<20} | {:<10}", "Address", "Reward Power", "Percentage");
            println!("{}", "-".repeat(78));

            let mut sorted_rewards: Vec<_> = reward_powers.iter().collect();
            sorted_rewards.sort_by(|a, b| b.1.cmp(a.1));

            for (address, power) in sorted_rewards.iter() {
                let percentage = if total_reward_power > U256::ZERO {
                    (**power * U256::from(10000) / total_reward_power).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                println!("{:#x} | {:<19} ZKC | {:>9.2}%", address, format_ether(**power), percentage);
            }
        }

        Ok(())
    }

    async fn query_stake_positions<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
    ) -> anyhow::Result<()> {
        println!("\n--- Stake Positions ---");

        // Query StakeAdded events for total stakes
        // Event signature: StakeAdded(uint256 indexed tokenId, address indexed owner, uint256 addedAmount, uint256 newTotal)
        let stake_event_sig = B256::from(alloy::primitives::keccak256(
            "StakeAdded(uint256,address,uint256,uint256)"
        ));
        let stake_filter = Filter::new()
            .address(deployment.vezkc_address)
            .event_signature(stake_event_sig)
            .from_block(BlockNumberOrTag::Earliest);

        let stake_logs = provider.get_logs(&stake_filter).await?;
        let mut stakes: HashMap<Address, U256> = HashMap::new();

        for log in stake_logs {
            if log.topics().len() >= 3 {
                let owner = Address::from_slice(&log.topics()[2][12..]);
                let data_bytes = &log.data().data;
                if data_bytes.len() >= 64 {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(&data_bytes[32..64]);
                    let new_total = U256::from_be_bytes(bytes);
                    stakes.insert(owner, new_total);
                }
            }
        }

        // Also check StakeCreated events
        // Event signature: StakeCreated(uint256 indexed tokenId, address indexed owner, uint256 amount)
        let create_event_sig = B256::from(alloy::primitives::keccak256(
            "StakeCreated(uint256,address,uint256)"
        ));
        let create_filter = Filter::new()
            .address(deployment.vezkc_address)
            .event_signature(create_event_sig)
            .from_block(BlockNumberOrTag::Earliest);

        let create_logs = provider.get_logs(&create_filter).await?;
        for log in create_logs {
            if log.topics().len() >= 3 {
                let owner = Address::from_slice(&log.topics()[2][12..]);
                let data_bytes = &log.data().data;
                if data_bytes.len() >= 32 {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(&data_bytes[0..32]);
                    let amount = U256::from_be_bytes(bytes);
                    stakes.entry(owner).and_modify(|e| *e = amount).or_insert(amount);
                }
            }
        }

        // Query UnstakeInitiated events to mark withdrawing positions
        // Event signature: UnstakeInitiated(uint256 indexed tokenId, address indexed owner, uint256 withdrawableAt)
        let unstake_event_sig = B256::from(alloy::primitives::keccak256(
            "UnstakeInitiated(uint256,address,uint256)"
        ));
        let unstake_filter = Filter::new()
            .address(deployment.vezkc_address)
            .event_signature(unstake_event_sig)
            .from_block(BlockNumberOrTag::Earliest);

        let unstake_logs = provider.get_logs(&unstake_filter).await?;
        let mut withdrawing: HashMap<Address, bool> = HashMap::new();

        for log in unstake_logs {
            if log.topics().len() >= 3 {
                let owner = Address::from_slice(&log.topics()[2][12..]);
                withdrawing.insert(owner, true);
            }
        }

        // Display stakes table
        if !stakes.is_empty() {
            // Calculate total staked
            let total_staked: U256 = stakes.values().sum();

            println!("\nStaked Positions (Total: {} ZKC):", format_ether(total_staked));
            println!("{:<44} | {:<20} | {:<10} | {:<12}", "Address", "Total Staked", "Percentage", "Status");
            println!("{}", "-".repeat(92));

            let mut sorted_stakes: Vec<_> = stakes.iter().collect();
            sorted_stakes.sort_by(|a, b| b.1.cmp(a.1));

            for (&address, amount) in sorted_stakes.iter() {
                let status = if *withdrawing.get(&address).unwrap_or(&false) {
                    "Withdrawing*"
                } else {
                    "Active"
                };

                let percentage = if total_staked > U256::ZERO {
                    (**amount * U256::from(10000) / total_staked).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };

                println!("{:#x} | {:<19} ZKC | {:>9.2}% | {}",
                    address, format_ether(**amount), percentage, status);
            }

            if withdrawing.values().any(|&v| v) {
                println!("* Withdrawal initiated but not yet completed");
            }
        }

        Ok(())
    }

    async fn query_povw_work<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        account: Address,
    ) -> anyhow::Result<()> {
        println!("\n--- PoVW Work Information ---");

        // Get current epoch
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;

        // Get pending epoch info
        let povw_accounting = IPovwAccounting::new(deployment.povw_accounting_address, provider);
        let pending_epoch = povw_accounting.pendingEpoch().call().await?;

        println!("Current epoch: {}", current_epoch);
        println!("Pending epoch: {} (total work: {})", pending_epoch.number, U256::from(pending_epoch.totalWork));

        // Query WorkLogUpdated events
        // Use the actual WorkLogUpdated event from IPovwAccounting
        let work_filter = Filter::new()
            .address(deployment.povw_accounting_address)
            .event_signature(IPovwAccounting::WorkLogUpdated::SIGNATURE_HASH)
            .from_block(BlockNumberOrTag::Earliest);

        let work_logs = provider.get_logs(&work_filter).await?;

        // Process work by recipient and epoch
        let mut work_by_recipient_current: HashMap<Address, U256> = HashMap::new();
        let mut work_by_recipient_all: HashMap<Address, U256> = HashMap::new();
        let mut my_work_current = U256::ZERO;

        for log in work_logs {
            let data_bytes = &log.data().data;
            if log.topics().len() >= 2 && data_bytes.len() >= 160 {
                // Parse event data
                let mut epoch_bytes = [0u8; 32];
                epoch_bytes.copy_from_slice(&data_bytes[0..32]);
                let epoch_number = U256::from_be_bytes(epoch_bytes);

                let mut value_bytes = [0u8; 32];
                value_bytes.copy_from_slice(&data_bytes[96..128]);
                let update_value = U256::from_be_bytes(value_bytes);

                let value_recipient = Address::from_slice(&data_bytes[140..160]);

                // Accumulate work across all epochs
                *work_by_recipient_all.entry(value_recipient).or_insert(U256::ZERO) += update_value;

                // Accumulate work for current epoch
                if epoch_number == current_epoch {
                    *work_by_recipient_current.entry(value_recipient).or_insert(U256::ZERO) += update_value;
                    if value_recipient == account {
                        my_work_current += update_value;
                    }
                }
            }
        }

        // Display current epoch work table
        if !work_by_recipient_current.is_empty() {
            let total_current_work: U256 = work_by_recipient_current.values().sum();

            println!("\nWork Contributors (Current Epoch {}, Total Work: {}):", current_epoch, total_current_work);
            println!("{:<44} | {:<20} | {:<10}", "Recipient", "Work", "Percentage");
            println!("{}", "-".repeat(78));

            let mut sorted_current: Vec<_> = work_by_recipient_current.iter().collect();
            sorted_current.sort_by(|a, b| b.1.cmp(a.1));

            for (recipient, work) in sorted_current.iter() {
                let percentage = if total_current_work > U256::ZERO {
                    (**work * U256::from(10000) / total_current_work).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                println!("{:#x} | {:<19} | {:>9.2}%", recipient, work, percentage);
            }
        }

        // Display all epochs work table
        if !work_by_recipient_all.is_empty() {
            let total_all_work: U256 = work_by_recipient_all.values().sum();

            println!("\nWork Contributors (All Epochs, Total Work: {}):", total_all_work);
            println!("{:<44} | {:<20} | {:<10}", "Recipient", "Total Work", "Percentage");
            println!("{}", "-".repeat(78));

            let mut sorted_all: Vec<_> = work_by_recipient_all.iter().collect();
            sorted_all.sort_by(|a, b| b.1.cmp(a.1));

            for (recipient, work) in sorted_all.iter() {
                let percentage = if total_all_work > U256::ZERO {
                    (**work * U256::from(10000) / total_all_work).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                println!("{:#x} | {:<19} | {:>9.2}%", recipient, work, percentage);
            }
        }

        // Calculate and display projected PoVW rewards with detailed calculation
        let total_work_u256 = U256::from(pending_epoch.totalWork);
        let povw_emissions = zkc.getPoVWEmissionsForEpoch(current_epoch).call().await?;

        println!("\n--- Your PoVW Projections ---");
        println!("Total PoVW emissions for epoch {}: {} ZKC", current_epoch, format_ether(povw_emissions));

        if my_work_current > U256::ZERO && total_work_u256 > U256::ZERO {
            let work_percentage = (my_work_current * U256::from(10000) / total_work_u256).to::<u64>() as f64 / 100.0;
            let projected_rewards = povw_emissions * my_work_current / total_work_u256;

            println!("Your work in current epoch: {} / {} ({:.2}%)",
                my_work_current, total_work_u256, work_percentage);
            println!("Projected PoVW rewards: {} ZKC", format_ether(projected_rewards));
            println!("Calculation: {} × {:.2}% = {} ZKC",
                format_ether(povw_emissions), work_percentage, format_ether(projected_rewards));
        } else if total_work_u256 == U256::ZERO {
            println!("No work submitted in current epoch yet");
        } else {
            println!("You have not submitted any work in the current epoch");
            println!("Your work: 0 / {} (0.00%)", total_work_u256);
        }

        Ok(())
    }
}