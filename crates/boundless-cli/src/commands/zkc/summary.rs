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
    sol,
    sol_types::SolEvent,
};
use anyhow::Context;
use boundless_market::contracts::IBoundlessMarket;
use boundless_povw::{deployments::Deployment, log_updater::IPovwAccounting};
use boundless_zkc::contracts::{IRewards, IStaking, IStakingRewards, IZKC};
use clap::Args;
use std::collections::HashMap;
use tabled::{
    builder::Builder,
    settings::Style,
};

// Block numbers for optimized event queries
const MAINNET_FROM_BLOCK: u64 = 23260070;
const SEPOLIA_FROM_BLOCK: u64 = 9110040;
const LOG_QUERY_CHUNK_SIZE: u64 = 5000;

// Define the missing getPendingRewards function from StakingRewards contract
sol! {
    #[sol(rpc)]
    interface IStakingRewardsExt {
        function getPendingRewards(address user) external returns (uint256);
    }
}

// Helper function to format ether values with 2 decimal places
fn format_zkc(value: U256) -> String {
    let formatted = format_ether(value);
    // Parse the string and format with 2 decimal places
    if let Ok(num) = formatted.parse::<f64>() {
        format!("{:.2}", num)
    } else {
        formatted
    }
}

// Helper function to print a centered section header
fn print_section_header(title: &str) {
    let width = 60;
    let padding = (width - title.len()) / 2;
    let extra = if (width - title.len()) % 2 != 0 { 1 } else { 0 };

    println!("\n{}", "=".repeat(width));
    println!("{}{}{}",
        " ".repeat(padding),
        title,
        " ".repeat(padding + extra)
    );
    println!("{}", "=".repeat(width));
}

// Helper function to query logs in chunks
async fn query_logs_chunked<P: Provider>(
    provider: &P,
    filter: Filter,
    from_block: u64,
    to_block: u64,
) -> anyhow::Result<Vec<alloy::rpc::types::Log>> {
    let mut all_logs = Vec::new();
    let mut current_from = from_block;

    while current_from <= to_block {
        let current_to = (current_from + LOG_QUERY_CHUNK_SIZE - 1).min(to_block);

        let chunk_filter = filter.clone()
            .from_block(BlockNumberOrTag::Number(current_from))
            .to_block(BlockNumberOrTag::Number(current_to));

        let logs = provider.get_logs(&chunk_filter).await?;
        all_logs.extend(logs);

        current_from = current_to + 1;
    }

    Ok(all_logs)
}

use crate::config::GlobalConfig;

/// [UNSTABLE] Summary command - queries and displays comprehensive ZKC staking, delegation, and PoVW work information.
///
/// Note: This command is unstable and its output format may change in future versions.
/// Requires an archive node with full event history for proper operation.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcSummary {
    /// Configuration for the PoVW deployment to use.
    #[clap(flatten, next_help_heading = "PoVW Deployment")]
    pub deployment: Option<Deployment>,

    /// Work log ID to query for (defaults to address of private key if set).
    #[clap(long = "work-log-id")]
    pub work_log_id: Option<Address>,
}

impl ZkcSummary {
    /// Run the [ZkcSummary] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let rpc_url = global_config.require_rpc_url()?;

        // Determine the work_log_id to use
        let work_log_id = match self.work_log_id {
            Some(addr) => addr,
            None => {
                // Try to use the address from the private key
                let private_key = global_config.require_private_key()
                    .context("No work_log_id provided. Please specify --work_log_id or set --private-key/PRIVATE_KEY")?;
                private_key.address()
            }
        };

        // Connect to the chain
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;

        // Check if the node is an archive node by trying to query an old block
        let test_block = match chain_id {
            1 => MAINNET_FROM_BLOCK,
            11155111 => SEPOLIA_FROM_BLOCK,
            _ => 1,
        };

        provider.get_block_by_number(BlockNumberOrTag::Number(test_block)).await
            .context(format!(
                "Failed to query historical block {}. This command requires an archive node with full event history. \
                Please use an RPC provider that supports archive node queries, such as Alchemy, Infura, or QuickNode with archive access enabled.",
                test_block
            ))?;

        // Ensure we're on mainnet or Sepolia
        if chain_id != 1 && chain_id != 11155111 {
            anyhow::bail!("ZKC summary is only supported on Ethereum Mainnet (chain ID 1) or Sepolia (chain ID 11155111). Current chain ID: {}", chain_id);
        }

        // Get deployment configuration
        let deployment = self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id))
            .context("could not determine PoVW deployment from chain ID; please specify deployment explicitly")?;

        print_section_header("ZKC SUMMARY");
        println!("Work Log ID: {:#x}", work_log_id);
        println!("Chain ID: {}", chain_id);

        // Query and display Personal Summary
        self.query_personal_summary(&provider, &deployment, work_log_id).await?;

        // Query staking rewards details
        self.query_staking_rewards(&provider, &deployment, work_log_id).await?;

        // Query vote and reward delegation information
        self.query_delegation_info(&provider, &deployment).await?;

        // Query stake positions
        self.query_stake_positions(&provider, &deployment).await?;

        // Query PoVW work information
        self.query_povw_work(&provider, &deployment, work_log_id).await?;

        // Query rewards claimed information
        self.query_rewards_claimed(&provider, &deployment).await?;

        Ok(())
    }

    async fn query_personal_summary<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        work_log_id: Address,
    ) -> anyhow::Result<()> {
        print_section_header("PERSONAL SUMMARY");

        // Get staked amount
        let staking = IStaking::new(deployment.vezkc_address, provider);
        let result = staking
            .getStakedAmountAndWithdrawalTime(work_log_id)
            .call()
            .await?;
        let staked_amount = result.amount;

        // Get ZKC deployment for staking rewards
        let zkc_deployment = boundless_zkc::deployments::Deployment::from_chain_id(
            provider.get_chain_id().await?
        ).context("Could not determine ZKC deployment")?;

        // Get current epoch
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;

        // Get pending staking rewards for current epoch
        let staking_rewards_ext = IStakingRewardsExt::new(zkc_deployment.staking_rewards_address, provider);
        let pending_staking_rewards = staking_rewards_ext.getPendingRewards(work_log_id).call().await?;

        // Calculate unclaimed staking rewards
        let staking_rewards = IStakingRewards::new(zkc_deployment.staking_rewards_address, provider);
        let epochs: Vec<U256> = (0..current_epoch.to::<u32>()).map(U256::from).collect();
        let unclaimed_staking = if !epochs.is_empty() {
            let unclaimed_rewards = staking_rewards.calculateUnclaimedRewards(work_log_id, epochs).call().await?;
            unclaimed_rewards.iter().sum()
        } else {
            U256::ZERO
        };

        // Get PoVW reward cap and calculate projected rewards
        let rewards = IRewards::new(deployment.vezkc_address, provider);
        let reward_cap = rewards.getPoVWRewardCap(work_log_id).call().await?;

        // Calculate projected PoVW rewards for the work_log_id
        let povw_accounting = IPovwAccounting::new(deployment.povw_accounting_address, provider);
        let pending_epoch = povw_accounting.pendingEpoch().call().await?;
        let total_work_u256 = U256::from(pending_epoch.totalWork);
        let povw_emissions = zkc.getPoVWEmissionsForEpoch(current_epoch).call().await?;

        // Get work_log_id's work for current epoch (we'll calculate this from events)
        let mut my_work_current = U256::ZERO;
        if total_work_u256 > U256::ZERO {
            // Quick query to get work for this epoch
            let chain_id = provider.get_chain_id().await?;
            let from_block_num = match chain_id {
                1 => MAINNET_FROM_BLOCK,
                11155111 => SEPOLIA_FROM_BLOCK,
                _ => 0,
            };
            let current_block = provider.get_block_number().await?;

            let work_filter = Filter::new()
                .address(deployment.povw_accounting_address)
                .event_signature(IPovwAccounting::WorkLogUpdated::SIGNATURE_HASH);

            let work_logs = if from_block_num == 0 {
                provider.get_logs(&work_filter.from_block(BlockNumberOrTag::Earliest)).await?
            } else {
                query_logs_chunked(provider, work_filter, from_block_num, current_block).await?
            };

            for log in work_logs {
                let data_bytes = &log.data().data;
                if log.topics().len() >= 2 && data_bytes.len() >= 160 {
                    let log_work_log_id = Address::from_slice(&log.topics()[1][12..]);
                    if log_work_log_id == work_log_id {
                        let mut epoch_bytes = [0u8; 32];
                        epoch_bytes.copy_from_slice(&data_bytes[0..32]);
                        let epoch_number = U256::from_be_bytes(epoch_bytes);

                        if epoch_number == current_epoch {
                            let mut value_bytes = [0u8; 32];
                            value_bytes.copy_from_slice(&data_bytes[96..128]);
                            let update_value = U256::from_be_bytes(value_bytes);
                            my_work_current += update_value;
                        }
                    }
                }
            }
        }

        let projected_povw_rewards = if my_work_current > U256::ZERO && total_work_u256 > U256::ZERO {
            let raw_rewards = povw_emissions * my_work_current / total_work_u256;
            if raw_rewards > reward_cap { reward_cap } else { raw_rewards }
        } else {
            U256::ZERO
        };

        let is_povw_capped = my_work_current > U256::ZERO && total_work_u256 > U256::ZERO &&
            (povw_emissions * my_work_current / total_work_u256) > reward_cap;

        // TODO: Query unclaimed PoVW rewards - would need to track epochs where mint happened
        let unclaimed_povw = U256::ZERO;

        // Query market balances if we have a BoundlessMarket deployment
        let market_balances = if let Some(market_deployment) = boundless_market::deployments::Deployment::from_chain_id(provider.get_chain_id().await?) {
            let market = IBoundlessMarket::new(market_deployment.boundless_market_address, provider);
            let eth_balance = market.balanceOf(work_log_id).call().await.unwrap_or(U256::ZERO);
            let collateral_balance = market.balanceOfCollateral(work_log_id).call().await.unwrap_or(U256::ZERO);
            Some((eth_balance, collateral_balance))
        } else {
            None
        };

        // Display summary
        println!("Staked ZKC: {} ZKC", format_zkc(staked_amount));

        if let Some((eth_balance, collateral_balance)) = market_balances {
            println!("ETH deposited to market: {} ETH", format_zkc(eth_balance));
            println!("ZKC deposited to market: {} ZKC", format_zkc(collateral_balance));
        }
        println!("Projected Staking Rewards for epoch {}: {} ZKC", current_epoch, format_zkc(pending_staking_rewards));

        if is_povw_capped {
            println!("Projected PoVW Rewards for epoch {}: {} ZKC ⚠️ CAPPED (uncapped: {} ZKC)",
                current_epoch,
                format_zkc(projected_povw_rewards),
                format_zkc(povw_emissions * my_work_current / total_work_u256));
            println!("                                      → Stake more ZKC to raise your reward cap");
        } else {
            println!("Projected PoVW Rewards for epoch {}: {} ZKC", current_epoch, format_zkc(projected_povw_rewards));
        }

        println!("Unclaimed Staking Rewards: {} ZKC", format_zkc(unclaimed_staking));
        println!("Unclaimed PoVW Rewards: {} ZKC", format_zkc(unclaimed_povw));

        Ok(())
    }

    async fn query_staking_rewards<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        account: Address,
    ) -> anyhow::Result<()> {
        print_section_header("STAKING REWARDS");

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

        // Sum up the unclaimed rewards
        let total_unclaimed: U256 = unclaimed_rewards.iter().sum();

        if total_unclaimed > U256::ZERO {
            println!("Total unclaimed staking rewards: {} ZKC", format_zkc(total_unclaimed));

            // Display table of unclaimed rewards by epoch
            println!("\nUnclaimed Staking Rewards by Epoch:");
            let mut builder = Builder::default();
            builder.push_record(["Epoch", "Unclaimed Amount"]);

            for (epoch_idx, amount) in unclaimed_rewards.iter().enumerate() {
                if *amount > U256::ZERO {
                    builder.push_record([epoch_idx.to_string(), format!("{} ZKC", format_zkc(*amount))]);
                }
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        } else {
            println!("No unclaimed staking rewards");
        }

        // Get expected staking rewards using getPendingRewards
        // This returns the actual pending rewards for the user in the current epoch
        let staking_rewards_ext = IStakingRewardsExt::new(zkc_deployment.staking_rewards_address, provider);
        let pending_rewards = staking_rewards_ext.getPendingRewards(account).call().await?;

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

        println!("\nExpected staking rewards for epoch {}:", current_epoch);
        println!("  Total epoch emissions: {} ZKC", format_zkc(total_staking_emissions));
        println!("  Your reward power: {} / {} ({:.2}%)",
            format_zkc(user_reward_power), format_zkc(total_reward_power), user_percentage);
        println!("  Your pending rewards: {} ZKC", format_zkc(pending_rewards));
        println!("  Calculation: {} × {:.2}% = {} ZKC",
            format_zkc(total_staking_emissions), user_percentage, format_zkc(pending_rewards));

        // Add PoVW Projections section
        self.display_povw_projections(provider, deployment, account).await?;

        Ok(())
    }

    async fn display_povw_projections<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        work_log_id: Address,
    ) -> anyhow::Result<()> {
        // Get PoVW reward cap for the work_log_id
        let rewards = IRewards::new(deployment.vezkc_address, provider);
        let reward_cap = rewards.getPoVWRewardCap(work_log_id).call().await?;

        // Get current epoch and emissions
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let povw_emissions = zkc.getPoVWEmissionsForEpoch(current_epoch).call().await?;

        // Get pending epoch info
        let povw_accounting = IPovwAccounting::new(deployment.povw_accounting_address, provider);
        let pending_epoch = povw_accounting.pendingEpoch().call().await?;
        let total_work_u256 = U256::from(pending_epoch.totalWork);

        // Get work_log_id's work for current epoch
        let mut my_work_current = U256::ZERO;
        if total_work_u256 > U256::ZERO {
            let chain_id = provider.get_chain_id().await?;
            let from_block_num = match chain_id {
                1 => MAINNET_FROM_BLOCK,
                11155111 => SEPOLIA_FROM_BLOCK,
                _ => 0,
            };
            let current_block = provider.get_block_number().await?;

            let work_filter = Filter::new()
                .address(deployment.povw_accounting_address)
                .event_signature(IPovwAccounting::WorkLogUpdated::SIGNATURE_HASH);

            let work_logs = if from_block_num == 0 {
                provider.get_logs(&work_filter.from_block(BlockNumberOrTag::Earliest)).await?
            } else {
                query_logs_chunked(provider, work_filter, from_block_num, current_block).await?
            };

            for log in work_logs {
                let data_bytes = &log.data().data;
                if log.topics().len() >= 2 && data_bytes.len() >= 160 {
                    let log_work_log_id = Address::from_slice(&log.topics()[1][12..]);
                    if log_work_log_id == work_log_id {
                        let mut epoch_bytes = [0u8; 32];
                        epoch_bytes.copy_from_slice(&data_bytes[0..32]);
                        let epoch_number = U256::from_be_bytes(epoch_bytes);

                        if epoch_number == current_epoch {
                            let mut value_bytes = [0u8; 32];
                            value_bytes.copy_from_slice(&data_bytes[96..128]);
                            let update_value = U256::from_be_bytes(value_bytes);
                            my_work_current += update_value;
                        }
                    }
                }
            }
        }

        print_section_header("YOUR POVW PROJECTIONS");

        if my_work_current > U256::ZERO && total_work_u256 > U256::ZERO {
            let work_percentage = (my_work_current * U256::from(10000) / total_work_u256).to::<u64>() as f64 / 100.0;
            let projected_rewards = povw_emissions * my_work_current / total_work_u256;

            // Check if rewards will be capped
            let is_capped = projected_rewards > reward_cap;
            let actual_rewards = if is_capped { reward_cap } else { projected_rewards };

            println!("Your work in current epoch: {} / {} ({:.2}%)",
                my_work_current, total_work_u256, work_percentage);
            println!("Total PoVW emissions for epoch {}: {} ZKC", current_epoch, format_zkc(povw_emissions));
            println!("Your PoVW reward cap: {} ZKC", format_zkc(reward_cap));

            if is_capped {
                println!("\n⚠️  REWARDS WILL BE CAPPED!");
                println!("Uncapped rewards: {} ZKC", format_zkc(projected_rewards));
                println!("Reward cap:       {} ZKC", format_zkc(reward_cap));
                println!("Actual rewards:   {} ZKC", format_zkc(actual_rewards));
                println!("→ Stake more ZKC to raise your reward cap");
            } else {
                println!("Projected PoVW rewards: {} ZKC", format_zkc(projected_rewards));
                println!("Calculation: {} × {:.2}% = {} ZKC",
                    format_zkc(povw_emissions), work_percentage, format_zkc(projected_rewards));
                println!("Status: ✅ Below reward cap ({} ZKC)", format_zkc(reward_cap));
            }
        } else {
            println!("Your work in current epoch: 0 / {} (0.00%)", total_work_u256);
            println!("Total PoVW emissions for epoch {}: {} ZKC", current_epoch, format_zkc(povw_emissions));
            println!("Your PoVW reward cap: {} ZKC", format_zkc(reward_cap));

            if total_work_u256 == U256::ZERO {
                println!("\nNo work submitted in current epoch yet");
            } else {
                println!("\nYou have not submitted any work in the current epoch");
            }
        }

        Ok(())
    }

    async fn query_delegation_info<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
    ) -> anyhow::Result<()> {
        print_section_header("POWER TABLES");

        // Get the appropriate from_block based on chain ID
        let chain_id = provider.get_chain_id().await?;
        let from_block_num = match chain_id {
            1 => MAINNET_FROM_BLOCK,  // Mainnet
            11155111 => SEPOLIA_FROM_BLOCK,  // Sepolia
            _ => 0,
        };

        // Get current block number
        let current_block = provider.get_block_number().await?;

        // Query DelegateVotesChanged events
        // Event signature: DelegateVotesChanged(address indexed delegate, uint256 previousVotes, uint256 newVotes)
        let vote_event_sig = B256::from(alloy::primitives::keccak256(
            "DelegateVotesChanged(address,uint256,uint256)"
        ));
        let vote_filter = Filter::new()
            .address(deployment.vezkc_address)
            .event_signature(vote_event_sig);

        let vote_logs = if from_block_num == 0 {
            provider.get_logs(&vote_filter.from_block(BlockNumberOrTag::Earliest)).await?
        } else {
            query_logs_chunked(provider, vote_filter, from_block_num, current_block).await?
        };
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

            println!("\nVote Powers (Total: {} ZKC):", format_zkc(total_vote_power));
            let mut builder = Builder::default();
            builder.push_record(["Address", "Vote Power", "Percentage"]);

            let mut sorted_votes: Vec<_> = vote_powers.iter().collect();
            sorted_votes.sort_by(|a, b| b.1.cmp(a.1));

            for (address, power) in sorted_votes.iter() {
                let percentage = if total_vote_power > U256::ZERO {
                    (**power * U256::from(10000) / total_vote_power).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                builder.push_record([
                    format!("{:#x}", address),
                    format!("{} ZKC", format_zkc(**power)),
                    format!("{:.2}%", percentage)
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        }

        // Query DelegateRewardsChanged events
        // Event signature: DelegateRewardsChanged(address indexed delegate, uint256 previousRewards, uint256 newRewards)
        let rewards_event_sig = B256::from(alloy::primitives::keccak256(
            "DelegateRewardsChanged(address,uint256,uint256)"
        ));
        let rewards_filter = Filter::new()
            .address(deployment.vezkc_address)
            .event_signature(rewards_event_sig);

        let rewards_logs = if from_block_num == 0 {
            provider.get_logs(&rewards_filter.from_block(BlockNumberOrTag::Earliest)).await?
        } else {
            query_logs_chunked(provider, rewards_filter, from_block_num, current_block).await?
        };
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

            println!("\nReward Powers (Total: {} ZKC):", format_zkc(total_reward_power));
            let mut builder = Builder::default();
            builder.push_record(["Address", "Reward Power", "Percentage"]);

            let mut sorted_rewards: Vec<_> = reward_powers.iter().collect();
            sorted_rewards.sort_by(|a, b| b.1.cmp(a.1));

            for (address, power) in sorted_rewards.iter() {
                let percentage = if total_reward_power > U256::ZERO {
                    (**power * U256::from(10000) / total_reward_power).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                builder.push_record([
                    format!("{:#x}", address),
                    format!("{} ZKC", format_zkc(**power)),
                    format!("{:.2}%", percentage)
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        }

        Ok(())
    }

    async fn query_stake_positions<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
    ) -> anyhow::Result<()> {
        print_section_header("STAKE POSITIONS");

        // Get the appropriate from_block based on chain ID
        let chain_id = provider.get_chain_id().await?;
        let from_block_num = match chain_id {
            1 => MAINNET_FROM_BLOCK,  // Mainnet
            11155111 => SEPOLIA_FROM_BLOCK,  // Sepolia
            _ => 0,
        };

        // Get current block number
        let current_block = provider.get_block_number().await?;

        // Query StakeAdded events for total stakes
        // Event signature: StakeAdded(uint256 indexed tokenId, address indexed owner, uint256 addedAmount, uint256 newTotal)
        let stake_event_sig = B256::from(alloy::primitives::keccak256(
            "StakeAdded(uint256,address,uint256,uint256)"
        ));
        let stake_filter = Filter::new()
            .address(deployment.vezkc_address)
            .event_signature(stake_event_sig);

        let stake_logs = if from_block_num == 0 {
            provider.get_logs(&stake_filter.from_block(BlockNumberOrTag::Earliest)).await?
        } else {
            query_logs_chunked(provider, stake_filter, from_block_num, current_block).await?
        };
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
            .event_signature(create_event_sig);

        let create_logs = if from_block_num == 0 {
            provider.get_logs(&create_filter.from_block(BlockNumberOrTag::Earliest)).await?
        } else {
            query_logs_chunked(provider, create_filter, from_block_num, current_block).await?
        };
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
            .event_signature(unstake_event_sig);

        let unstake_logs = if from_block_num == 0 {
            provider.get_logs(&unstake_filter.from_block(BlockNumberOrTag::Earliest)).await?
        } else {
            query_logs_chunked(provider, unstake_filter, from_block_num, current_block).await?
        };
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
            let num_positions = stakes.len();

            // Get current epoch staking emissions for reward calculations
            let zkc = IZKC::new(deployment.zkc_address, provider);
            let current_epoch = zkc.getCurrentEpoch().call().await?;
            let staking_emissions = zkc.getStakingEmissionsForEpoch(current_epoch).call().await?;

            println!("\nStaked Positions ({} positions, Total: {} ZKC):", num_positions, format_zkc(total_staked));
            let mut builder = Builder::default();
            builder.push_record(["Address", "Total Staked", "Percentage", "Projected Rewards", "Status"]);

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

                // Calculate projected rewards for this position
                let projected_rewards = if total_staked > U256::ZERO {
                    staking_emissions * **amount / total_staked
                } else {
                    U256::ZERO
                };

                builder.push_record([
                    format!("{:#x}", address),
                    format!("{} ZKC", format_zkc(**amount)),
                    format!("{:.2}%", percentage),
                    format!("{} ZKC", format_zkc(projected_rewards)),
                    status.to_string()
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);

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
        work_log_id: Address,
    ) -> anyhow::Result<()> {
        print_section_header("POVW WORK INFORMATION");

        // Get current epoch
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;

        // Get pending epoch info
        let povw_accounting = IPovwAccounting::new(deployment.povw_accounting_address, provider);
        let pending_epoch = povw_accounting.pendingEpoch().call().await?;

        println!("Current epoch: {}", current_epoch);
        println!("Pending epoch: {} (total work: {})", pending_epoch.number, U256::from(pending_epoch.totalWork));

        // Get the appropriate from_block based on chain ID
        let chain_id = provider.get_chain_id().await?;
        let from_block_num = match chain_id {
            1 => MAINNET_FROM_BLOCK,  // Mainnet
            11155111 => SEPOLIA_FROM_BLOCK,  // Sepolia
            _ => 0,
        };

        // Get current block number
        let current_block = provider.get_block_number().await?;

        // Query WorkLogUpdated events
        // Use the actual WorkLogUpdated event from IPovwAccounting
        let work_filter = Filter::new()
            .address(deployment.povw_accounting_address)
            .event_signature(IPovwAccounting::WorkLogUpdated::SIGNATURE_HASH);

        let work_logs = if from_block_num == 0 {
            provider.get_logs(&work_filter.from_block(BlockNumberOrTag::Earliest)).await?
        } else {
            query_logs_chunked(provider, work_filter, from_block_num, current_block).await?
        };

        // Process work by both work_log_id and recipient
        let mut work_by_work_log_id_current: HashMap<Address, U256> = HashMap::new();
        let mut work_by_work_log_id_all: HashMap<Address, U256> = HashMap::new();
        let mut work_by_recipient_current: HashMap<Address, U256> = HashMap::new();
        let mut work_by_recipient_all: HashMap<Address, U256> = HashMap::new();
        let mut my_work_current = U256::ZERO;

        for log in work_logs {
            let data_bytes = &log.data().data;
            if log.topics().len() >= 2 && data_bytes.len() >= 160 {
                // Extract work_log_id from the indexed topic
                let log_work_log_id = Address::from_slice(&log.topics()[1][12..]);

                // Parse event data
                let mut epoch_bytes = [0u8; 32];
                epoch_bytes.copy_from_slice(&data_bytes[0..32]);
                let epoch_number = U256::from_be_bytes(epoch_bytes);

                let mut value_bytes = [0u8; 32];
                value_bytes.copy_from_slice(&data_bytes[96..128]);
                let update_value = U256::from_be_bytes(value_bytes);

                let value_recipient = Address::from_slice(&data_bytes[140..160]);

                // Accumulate work by work_log_id across all epochs
                *work_by_work_log_id_all.entry(log_work_log_id).or_insert(U256::ZERO) += update_value;

                // Accumulate work by recipient across all epochs
                *work_by_recipient_all.entry(value_recipient).or_insert(U256::ZERO) += update_value;

                // Accumulate work for current epoch
                if epoch_number == current_epoch {
                    *work_by_work_log_id_current.entry(log_work_log_id).or_insert(U256::ZERO) += update_value;
                    *work_by_recipient_current.entry(value_recipient).or_insert(U256::ZERO) += update_value;
                    if log_work_log_id == work_log_id {
                        my_work_current += update_value;
                    }
                }
            }
        }

        // Display current epoch work by work_log_id
        if !work_by_work_log_id_current.is_empty() {
            let total_current_work: U256 = work_by_work_log_id_current.values().sum();

            println!("\nWork by Work Log ID (Current Epoch {}, Total Work: {}):", current_epoch, total_current_work);
            let mut builder = Builder::default();
            builder.push_record(["Work Log ID", "Work", "Percentage"]);

            let mut sorted_current: Vec<_> = work_by_work_log_id_current.iter().collect();
            sorted_current.sort_by(|a, b| b.1.cmp(a.1));

            for (wid, work) in sorted_current.iter() {
                let percentage = if total_current_work > U256::ZERO {
                    (**work * U256::from(10000) / total_current_work).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                builder.push_record([
                    format!("{:#x}", wid),
                    format!("{}", work),
                    format!("{:.2}%", percentage)
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        }

        // Display current epoch work by recipient
        if !work_by_recipient_current.is_empty() {
            let total_current_work: U256 = work_by_recipient_current.values().sum();

            println!("\nWork by Recipient (Current Epoch {}, Total Work: {}):", current_epoch, total_current_work);
            let mut builder = Builder::default();
            builder.push_record(["Recipient", "Work", "Percentage"]);

            let mut sorted_current: Vec<_> = work_by_recipient_current.iter().collect();
            sorted_current.sort_by(|a, b| b.1.cmp(a.1));

            for (recipient, work) in sorted_current.iter() {
                let percentage = if total_current_work > U256::ZERO {
                    (**work * U256::from(10000) / total_current_work).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                builder.push_record([
                    format!("{:#x}", recipient),
                    format!("{}", work),
                    format!("{:.2}%", percentage)
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        }

        // Display all epochs work table
        if !work_by_recipient_all.is_empty() {
            let total_all_work: U256 = work_by_recipient_all.values().sum();

            println!("\nWork Contributors (All Epochs, Total Work: {}):", total_all_work);
            let mut builder = Builder::default();
            builder.push_record(["Recipient", "Total Work", "Percentage"]);

            let mut sorted_all: Vec<_> = work_by_recipient_all.iter().collect();
            sorted_all.sort_by(|a, b| b.1.cmp(a.1));

            for (recipient, work) in sorted_all.iter() {
                let percentage = if total_all_work > U256::ZERO {
                    (**work * U256::from(10000) / total_all_work).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                builder.push_record([
                    format!("{:#x}", recipient),
                    format!("{}", work),
                    format!("{:.2}%", percentage)
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        }

        // Removed duplicate PoVW projections code - now handled in display_povw_projections()

        Ok(())
    }

    async fn query_rewards_claimed<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
    ) -> anyhow::Result<()> {
        print_section_header("REWARDS CLAIMED HISTORY");

        // Get the appropriate from_block based on chain ID
        let chain_id = provider.get_chain_id().await?;
        let from_block_num = match chain_id {
            1 => MAINNET_FROM_BLOCK,  // Mainnet
            11155111 => SEPOLIA_FROM_BLOCK,  // Sepolia
            _ => 0,
        };

        // Get current block number
        let current_block = provider.get_block_number().await?;

        // Query PoVWRewardsClaimed events
        // Event signature: PoVWRewardsClaimed(address indexed recipient, uint256 amount)
        let povw_claimed_sig = B256::from(alloy::primitives::keccak256(
            "PoVWRewardsClaimed(address,uint256)"
        ));
        let povw_filter = Filter::new()
            .address(deployment.zkc_address)
            .event_signature(povw_claimed_sig);

        let povw_logs = if from_block_num == 0 {
            provider.get_logs(&povw_filter.from_block(BlockNumberOrTag::Earliest)).await?
        } else {
            query_logs_chunked(provider, povw_filter, from_block_num, current_block).await?
        };

        let mut povw_claims: HashMap<Address, U256> = HashMap::new();
        for log in povw_logs {
            if log.topics().len() >= 2 {
                let recipient = Address::from_slice(&log.topics()[1][12..]);
                let data_bytes = &log.data().data;
                if data_bytes.len() >= 32 {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(&data_bytes[0..32]);
                    let amount = U256::from_be_bytes(bytes);
                    *povw_claims.entry(recipient).or_insert(U256::ZERO) += amount;
                }
            }
        }

        // Query StakingRewardsClaimed events
        // Event signature: StakingRewardsClaimed(address indexed recipient, uint256 amount)
        let staking_claimed_sig = B256::from(alloy::primitives::keccak256(
            "StakingRewardsClaimed(address,uint256)"
        ));
        let staking_filter = Filter::new()
            .address(deployment.zkc_address)
            .event_signature(staking_claimed_sig);

        let staking_logs = if from_block_num == 0 {
            provider.get_logs(&staking_filter.from_block(BlockNumberOrTag::Earliest)).await?
        } else {
            query_logs_chunked(provider, staking_filter, from_block_num, current_block).await?
        };

        let mut staking_claims: HashMap<Address, U256> = HashMap::new();
        for log in staking_logs {
            if log.topics().len() >= 2 {
                let recipient = Address::from_slice(&log.topics()[1][12..]);
                let data_bytes = &log.data().data;
                if data_bytes.len() >= 32 {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(&data_bytes[0..32]);
                    let amount = U256::from_be_bytes(bytes);
                    *staking_claims.entry(recipient).or_insert(U256::ZERO) += amount;
                }
            }
        }

        // Display PoVW rewards claimed table
        if !povw_claims.is_empty() {
            let total_povw: U256 = povw_claims.values().sum();
            let mut sorted_povw: Vec<_> = povw_claims.iter().collect();
            sorted_povw.sort_by(|a, b| b.1.cmp(a.1));

            println!("\nPoVW Rewards Claimed (Total: {} ZKC):", format_zkc(total_povw));
            let mut builder = Builder::default();
            builder.push_record(["Address", "Total Claimed", "Percentage"]);

            for (address, amount) in sorted_povw {
                let percentage = if total_povw > U256::ZERO {
                    (*amount * U256::from(10000) / total_povw).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                builder.push_record([
                    format!("{:#x}", address),
                    format!("{} ZKC", format_zkc(*amount)),
                    format!("{:.2}%", percentage)
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        } else {
            println!("\nNo PoVW rewards have been claimed yet");
        }

        // Display Staking rewards claimed table
        if !staking_claims.is_empty() {
            let total_staking: U256 = staking_claims.values().sum();
            let mut sorted_staking: Vec<_> = staking_claims.iter().collect();
            sorted_staking.sort_by(|a, b| b.1.cmp(a.1));

            println!("\nStaking Rewards Claimed (Total: {} ZKC):", format_zkc(total_staking));
            let mut builder = Builder::default();
            builder.push_record(["Address", "Total Claimed", "Percentage"]);

            for (address, amount) in sorted_staking {
                let percentage = if total_staking > U256::ZERO {
                    (*amount * U256::from(10000) / total_staking).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                builder.push_record([
                    format!("{:#x}", address),
                    format!("{} ZKC", format_zkc(*amount)),
                    format!("{:.2}%", percentage)
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        } else {
            println!("\nNo Staking rewards have been claimed yet");
        }

        Ok(())
    }
}