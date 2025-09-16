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
    rpc::types::{BlockNumberOrTag, Filter, Log},
    sol,
    sol_types::SolEvent,
};
use anyhow::Context;
use boundless_povw::{deployments::Deployment, log_updater::IPovwAccounting};
use boundless_zkc::contracts::{IRewards, IStaking, IStakingRewards, IZKC};
use clap::Args;
use std::collections::HashMap;
use tabled::{
    builder::Builder,
    settings::Style,
};

// Block numbers from before contract creation.
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

fn format_zkc(value: U256) -> String {
    let formatted = format_ether(value);
    // Parse the string and format with 2 decimal places
    if let Ok(num) = formatted.parse::<f64>() {
        format!("{:.2}", num)
    } else {
        formatted
    }
}

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

// Query logs in chunks
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

/// Struct to hold all fetched event logs for processing
struct AllEventLogs {
    work_logs: Vec<Log>,
    stake_created_logs: Vec<Log>,
    stake_added_logs: Vec<Log>,
    unstake_initiated_logs: Vec<Log>,
    unstake_completed_logs: Vec<Log>,
    vote_delegation_logs: Vec<Log>,
    reward_delegation_logs: Vec<Log>,
    povw_claims_logs: Vec<Log>,
    staking_claims_logs: Vec<Log>,
}

/// Fetch all event logs in batches to avoid rate limiting
async fn fetch_all_event_logs<P: Provider>(
    provider: &P,
    deployment: &Deployment,
    zkc_deployment: &boundless_zkc::deployments::Deployment,
    from_block_num: u64,
    current_block: u64,
) -> anyhow::Result<AllEventLogs> {
    println!("Fetching blockchain event data ({} blocks)...", current_block - from_block_num);

    // Batch 1: Core stake and work data (4 parallel queries)
    println!("[1/3] Querying stake and work events...");

    let work_filter = Filter::new()
        .address(deployment.povw_accounting_address)
        .event_signature(IPovwAccounting::WorkLogUpdated::SIGNATURE_HASH);

    let stake_created_filter = Filter::new()
        .address(deployment.vezkc_address)
        .event_signature(B256::from(alloy::primitives::keccak256(
            "StakeCreated(uint256,address,uint256)"
        )));

    let stake_added_filter = Filter::new()
        .address(deployment.vezkc_address)
        .event_signature(B256::from(alloy::primitives::keccak256(
            "StakeAdded(uint256,address,uint256,uint256)"
        )));

    let unstake_initiated_filter = Filter::new()
        .address(deployment.vezkc_address)
        .event_signature(B256::from(alloy::primitives::keccak256(
            "UnstakeInitiated(uint256,address,uint256)"
        )));

    let (work_logs, stake_created_logs, stake_added_logs, unstake_initiated_logs) = tokio::join!(
        async {
            if from_block_num == 0 {
                provider.get_logs(&work_filter.from_block(BlockNumberOrTag::Earliest)).await
                    .map_err(|e| anyhow::anyhow!("Failed to get work logs: {}", e))
            } else {
                query_logs_chunked(provider, work_filter, from_block_num, current_block).await
            }
        },
        async {
            if from_block_num == 0 {
                provider.get_logs(&stake_created_filter.from_block(BlockNumberOrTag::Earliest)).await
                    .map_err(|e| anyhow::anyhow!("Failed to get stake created logs: {}", e))
            } else {
                query_logs_chunked(provider, stake_created_filter, from_block_num, current_block).await
            }
        },
        async {
            if from_block_num == 0 {
                provider.get_logs(&stake_added_filter.from_block(BlockNumberOrTag::Earliest)).await
                    .map_err(|e| anyhow::anyhow!("Failed to get stake added logs: {}", e))
            } else {
                query_logs_chunked(provider, stake_added_filter, from_block_num, current_block).await
            }
        },
        async {
            if from_block_num == 0 {
                provider.get_logs(&unstake_initiated_filter.from_block(BlockNumberOrTag::Earliest)).await
                    .map_err(|e| anyhow::anyhow!("Failed to get unstake initiated logs: {}", e))
            } else {
                query_logs_chunked(provider, unstake_initiated_filter, from_block_num, current_block).await
            }
        }
    );

    // Batch 2: Delegation and completion (3 parallel queries)
    println!("[2/3] Querying delegation and unstake completion events...");

    let unstake_completed_filter = Filter::new()
        .address(deployment.vezkc_address)
        .event_signature(B256::from(alloy::primitives::keccak256(
            "UnstakeCompleted(uint256,address,uint256)"
        )));

    let vote_delegation_filter = Filter::new()
        .address(deployment.vezkc_address)
        .event_signature(B256::from(alloy::primitives::keccak256(
            "DelegateVotesChanged(address,uint256,uint256)"
        )));

    let reward_delegation_filter = Filter::new()
        .address(deployment.vezkc_address)
        .event_signature(B256::from(alloy::primitives::keccak256(
            "DelegateRewardsChanged(address,uint256,uint256)"
        )));

    let (unstake_completed_logs, vote_delegation_logs, reward_delegation_logs) = tokio::join!(
        async {
            if from_block_num == 0 {
                provider.get_logs(&unstake_completed_filter.from_block(BlockNumberOrTag::Earliest)).await
                    .map_err(|e| anyhow::anyhow!("Failed to get unstake completed logs: {}", e))
            } else {
                query_logs_chunked(provider, unstake_completed_filter, from_block_num, current_block).await
            }
        },
        async {
            if from_block_num == 0 {
                provider.get_logs(&vote_delegation_filter.from_block(BlockNumberOrTag::Earliest)).await
                    .map_err(|e| anyhow::anyhow!("Failed to get vote delegation logs: {}", e))
            } else {
                query_logs_chunked(provider, vote_delegation_filter, from_block_num, current_block).await
            }
        },
        async {
            if from_block_num == 0 {
                provider.get_logs(&reward_delegation_filter.from_block(BlockNumberOrTag::Earliest)).await
                    .map_err(|e| anyhow::anyhow!("Failed to get reward delegation logs: {}", e))
            } else {
                query_logs_chunked(provider, reward_delegation_filter, from_block_num, current_block).await
            }
        }
    );

    // Batch 3: Reward claims (2 parallel queries)
    println!("[3/3] Querying reward claim events...");

    let povw_claims_filter = Filter::new()
        .address(zkc_deployment.zkc_address)
        .event_signature(B256::from(alloy::primitives::keccak256(
            "POVWRewardsClaimed(address,uint256)"
        )));

    let staking_claims_filter = Filter::new()
        .address(zkc_deployment.zkc_address)
        .event_signature(B256::from(alloy::primitives::keccak256(
            "StakingRewardsClaimed(address,uint256)"
        )));

    let (povw_claims_logs, staking_claims_logs) = tokio::join!(
        async {
            if from_block_num == 0 {
                provider.get_logs(&povw_claims_filter.from_block(BlockNumberOrTag::Earliest)).await
                    .map_err(|e| anyhow::anyhow!("Failed to get povw claims logs: {}", e))
            } else {
                query_logs_chunked(provider, povw_claims_filter, from_block_num, current_block).await
            }
        },
        async {
            if from_block_num == 0 {
                provider.get_logs(&staking_claims_filter.from_block(BlockNumberOrTag::Earliest)).await
                    .map_err(|e| anyhow::anyhow!("Failed to get staking claims logs: {}", e))
            } else {
                query_logs_chunked(provider, staking_claims_filter, from_block_num, current_block).await
            }
        }
    );

    println!("✓ Event data fetched successfully");

    Ok(AllEventLogs {
        work_logs: work_logs?,
        stake_created_logs: stake_created_logs?,
        stake_added_logs: stake_added_logs?,
        unstake_initiated_logs: unstake_initiated_logs?,
        unstake_completed_logs: unstake_completed_logs?,
        vote_delegation_logs: vote_delegation_logs?,
        reward_delegation_logs: reward_delegation_logs?,
        povw_claims_logs: povw_claims_logs?,
        staking_claims_logs: staking_claims_logs?,
    })
}

/// [UNSTABLE] Summary command - queries and displays comprehensive ZKC staking, delegation, and PoVW work information.
///
/// Note: This command is unstable and its output format may change in future versions.
/// Requires an archive node with full event history for proper operation.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcSummary {
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
        let deployment = Deployment::from_chain_id(chain_id)
            .context("could not determine PoVW deployment from chain ID")?;

        // Get ZKC deployment for reward claims
        let zkc_deployment = boundless_zkc::deployments::Deployment::from_chain_id(chain_id)
            .context("could not determine ZKC deployment from chain ID")?;

        // Get the appropriate from_block based on chain ID
        let from_block_num = match chain_id {
            1 => MAINNET_FROM_BLOCK,  // Mainnet
            11155111 => SEPOLIA_FROM_BLOCK,  // Sepolia
            _ => 0,
        };

        // Get current block number
        let current_block = provider.get_block_number().await?;

        // Fetch all event logs in parallel batches
        let all_logs = fetch_all_event_logs(
            &provider,
            &deployment,
            &zkc_deployment,
            from_block_num,
            current_block,
        ).await?;

        print_section_header("ZKC SUMMARY");
        println!("Work Log ID: {:#x}", work_log_id);
        println!("Chain ID: {}", chain_id);

        // Get and display epoch end time
        let zkc = IZKC::new(zkc_deployment.zkc_address, &provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let epoch_end_timestamp = zkc.getCurrentEpochEndTime().call().await?;

        // Convert timestamp to human readable format
        let epoch_end_secs = epoch_end_timestamp.to::<u64>();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let time_remaining_secs = if epoch_end_secs > now {
            epoch_end_secs - now
        } else {
            0
        };

        // Format time remaining
        let time_remaining_str = if time_remaining_secs == 0 {
            "ended".to_string()
        } else {
            let days = time_remaining_secs / 86400;
            let hours = (time_remaining_secs % 86400) / 3600;
            let minutes = (time_remaining_secs % 3600) / 60;

            let format_unit = |value: u64, unit: &str| -> String {
                if value == 1 {
                    format!("{} {}", value, unit)
                } else {
                    format!("{} {}s", value, unit)
                }
            };

            if days > 0 {
                format!("{}, {}, {} from now",
                    format_unit(days, "day"),
                    format_unit(hours, "hour"),
                    format_unit(minutes, "minute"))
            } else if hours > 0 {
                format!("{}, {} from now",
                    format_unit(hours, "hour"),
                    format_unit(minutes, "minute"))
            } else if minutes > 0 {
                format!("{} from now", format_unit(minutes, "minute"))
            } else {
                format!("{} seconds from now", time_remaining_secs)
            }
        };

        // Convert to UTC datetime string
        use chrono::{DateTime, Utc};
        let epoch_end_datetime = DateTime::<Utc>::from_timestamp(epoch_end_secs as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid timestamp".to_string());

        println!("Current Epoch: {}", current_epoch);
        println!("Epoch Ends: {} ({})", epoch_end_datetime, time_remaining_str);

        // Process and display Personal Summary
        self.process_personal_summary(&provider, &deployment, work_log_id, &all_logs).await?;

        // Process staking rewards details
        self.process_staking_rewards(&provider, &deployment, work_log_id, &all_logs).await?;

        // Process vote and reward delegation information
        let reward_powers = self.process_delegation_info(&all_logs.vote_delegation_logs, &all_logs.reward_delegation_logs)?;

        // Process stake positions
        self.process_stake_positions(
            &provider,
            &deployment,
            &reward_powers,
            &all_logs.stake_created_logs,
            &all_logs.stake_added_logs,
            &all_logs.unstake_initiated_logs,
            &all_logs.unstake_completed_logs,
        ).await?;

        // Process PoVW work information
        self.process_povw_work(&provider, &deployment, work_log_id, &all_logs.work_logs).await?;

        // Process rewards claimed information
        self.process_rewards_claimed(&all_logs.povw_claims_logs, &all_logs.staking_claims_logs)?;

        Ok(())
    }

    async fn process_personal_summary<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        work_log_id: Address,
        logs: &AllEventLogs,
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

        // Get work_log_id's work for current epoch from pre-fetched logs
        let mut my_work_current = U256::ZERO;
        if total_work_u256 > U256::ZERO {
            for log in &logs.work_logs {
                if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
                    if decoded.inner.data.workLogId == work_log_id {
                        if U256::from(decoded.inner.data.epochNumber) == current_epoch {
                            my_work_current += U256::from(decoded.inner.data.updateValue);
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

        // Display summary
        println!("Staked ZKC: {} ZKC", format_zkc(staked_amount));
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

        // TODO: Query unclaimed PoVW rewards - would need to track work log IDs, commits, and epochs where updates happened,
        // and then reconcile to mint txs.

        Ok(())
    }

    async fn process_staking_rewards<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        account: Address,
        _logs: &AllEventLogs,
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
                if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
                    if decoded.inner.data.workLogId == work_log_id {
                        if U256::from(decoded.inner.data.epochNumber) == current_epoch {
                            my_work_current += U256::from(decoded.inner.data.updateValue);
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

    fn process_delegation_info(
        &self,
        vote_logs: &[Log],
        rewards_logs: &[Log],
    ) -> anyhow::Result<HashMap<Address, U256>> {
        print_section_header("DELEGATION TABLES");

        let mut vote_powers: HashMap<Address, U256> = HashMap::new();

        for log in vote_logs {
            // DelegateVotesChanged is part of veZKC/IStaking, use manual parsing for now
            if log.topics().len() >= 2 {
                let delegate = Address::from_slice(&log.topics()[1][12..]);
                let data_bytes = &log.data().data;
                if data_bytes.len() >= 64 {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(&data_bytes[32..64]);
                    let new_votes = U256::from_be_bytes(bytes);
                    // Only track non-zero powers (zero means delegation removed)
                    if new_votes > U256::ZERO {
                        vote_powers.insert(delegate, new_votes);
                    } else {
                        vote_powers.remove(&delegate);
                    }
                }
            }
            /*if let Ok(decoded) = log.log_decode::<IStaking::DelegateVotesChanged>() {
            }*/
        }

        let mut reward_powers: HashMap<Address, U256> = HashMap::new();

        for log in rewards_logs {
            if let Ok(decoded) = log.log_decode::<IRewards::DelegateRewardsChanged>() {
                let delegate = decoded.inner.data.delegate;
                let new_rewards = decoded.inner.data.newRewards;
                // Only track non-zero powers (zero means delegation removed)
                if new_rewards > U256::ZERO {
                    reward_powers.insert(delegate, new_rewards);
                } else {
                    reward_powers.remove(&delegate);
                }
            }
        }


        // Display vote power table
        if !vote_powers.is_empty() {
            let total_vote_power: U256 = vote_powers.values().sum();

            println!("\nDelegated Vote Power (Total: {} ZKC):", format_zkc(total_vote_power));
            println!("Shows who can vote on governance proposals");

            let mut builder = Builder::default();
            builder.push_record(["Address", "Vote Power", "Percentage"]);

            let mut sorted_votes: Vec<_> = vote_powers.iter().collect();
            sorted_votes.sort_by(|a, b| b.1.cmp(a.1));

            // Show top 20 for brevity
            for (address, power) in sorted_votes.iter().take(20) {
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

            if sorted_votes.len() > 20 {
                builder.push_record([
                    format!("... and {} more", sorted_votes.len() - 20),
                    "".to_string(),
                    "".to_string()
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        }

        // Display reward power table
        if !reward_powers.is_empty() {
            let total_reward_power: U256 = reward_powers.values().sum();

            println!("\nDelegated Reward Power (Total: {} ZKC):", format_zkc(total_reward_power));
            println!("Shows who can claim staking rewards");

            let mut builder = Builder::default();
            builder.push_record(["Address", "Reward Power", "Percentage"]);

            let mut sorted_rewards: Vec<_> = reward_powers.iter().collect();
            sorted_rewards.sort_by(|a, b| b.1.cmp(a.1));

            // Show top 20 for brevity
            for (address, power) in sorted_rewards.iter().take(20) {
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

            if sorted_rewards.len() > 20 {
                builder.push_record([
                    format!("... and {} more", sorted_rewards.len() - 20),
                    "".to_string(),
                    "".to_string()
                ]);
            }

            let table = builder.build()
                .with(Style::modern())
                .to_string();
            println!("{}", table);
        }

        // Return reward powers for use in stake positions calculation
        Ok(reward_powers)
    }

    async fn process_stake_positions<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        reward_powers: &HashMap<Address, U256>,
        stake_created_logs: &[Log],
        stake_added_logs: &[Log],
        unstake_initiated_logs: &[Log],
        unstake_completed_logs: &[Log],
    ) -> anyhow::Result<()> {
        print_section_header("STAKE POSITIONS");

        let mut stakes: HashMap<Address, U256> = HashMap::new();

        // FIRST: Process StakeCreated events (initial stakes)
        for log in stake_created_logs {
            if let Ok(decoded) = log.log_decode::<IStaking::StakeCreated>() {
                let owner = decoded.inner.data.owner;
                let amount = decoded.inner.data.amount;
                stakes.entry(owner).or_insert(amount);
            }
        }

        // SECOND: Process StakeAdded events (updates to stakes)
        for log in stake_added_logs {
            if let Ok(decoded) = log.log_decode::<IStaking::StakeAdded>() {
                let owner = decoded.inner.data.owner;
                let new_total = decoded.inner.data.newTotal;
                // StakeAdded always overwrites with the new total
                stakes.insert(owner, new_total);
            }
        }

        // Process UnstakeInitiated events to mark withdrawing positions
        let mut withdrawing: HashMap<Address, bool> = HashMap::new();
        for log in unstake_initiated_logs {
            if let Ok(decoded) = log.log_decode::<IStaking::UnstakeInitiated>() {
                let owner = decoded.inner.data.owner;
                withdrawing.insert(owner, true);
            }
        }

        // Process UnstakeCompleted events to remove completed unstakes from total
        for log in unstake_completed_logs {
            if let Ok(decoded) = log.log_decode::<IStaking::UnstakeCompleted>() {
                let owner = decoded.inner.data.owner;
                // Remove from stakes map when unstake is completed
                stakes.remove(&owner);
                // Also remove from withdrawing status
                withdrawing.remove(&owner);
            }
        }

        // Display stakes table
        if !stakes.is_empty() {
            // Calculate total staked
            let total_staked: U256 = stakes.values().sum();
            let num_positions = stakes.len();

            // Validate against contract's getTotalStakingRewards()
            let rewards_contract = IRewards::new(deployment.vezkc_address, provider);
            let contract_total = rewards_contract.getTotalStakingRewards().call().await?;

            if contract_total != total_staked {
                println!("⚠️  WARNING: Stake total mismatch!");
                println!("  Contract reports: {} ZKC", format_zkc(contract_total));
                println!("  Events sum to: {} ZKC", format_zkc(total_staked));
                println!("  Difference: {} ZKC",
                    if contract_total > total_staked {
                        format!("+{}", format_zkc(contract_total - total_staked))
                    } else {
                        format!("-{}", format_zkc(total_staked - contract_total))
                    }
                );
            }

            // Get current epoch staking emissions for reward calculations
            let zkc = IZKC::new(deployment.zkc_address, provider);
            let current_epoch = zkc.getCurrentEpoch().call().await?;
            let staking_emissions = zkc.getStakingEmissionsForEpoch(current_epoch).call().await?;

            println!("\nStaked Positions ({} positions, Total: {} ZKC):", num_positions, format_zkc(total_staked));
            println!("Shows actual token ownership");
            let mut builder = Builder::default();
            builder.push_record(["Address", "Total Staked", "%", "Projected Rewards", "Status"]);

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

            // Show totals comparison
            let total_reward_power: U256 = reward_powers.values().sum();

            // Get the contract's total for validation
            let rewards_contract = IRewards::new(deployment.vezkc_address, provider);
            let contract_total = rewards_contract.getTotalStakingRewards().call().await?;

            println!("\nTotals Summary:");
            println!("Contract Total (getTotalStakingRewards): {} ZKC", format_zkc(contract_total));
            println!("Total from Stake Events: {} ZKC", format_zkc(total_staked));
            println!("Total Reward Power: {} ZKC", format_zkc(total_reward_power));

            // The contract total is the source of truth
            if total_reward_power != contract_total {
                let excess = total_reward_power.saturating_sub(contract_total);
                println!("\n⚠️  WARNING: Reward delegation events show {} ZKC MORE power than exists!", format_zkc(excess));
                println!("  This indicates that delegation events aren't properly zeroed when unstaking.");
                println!("  The contract's getTotalStakingRewards() is the accurate total.");
            } else if total_reward_power == contract_total {
                println!("\n✓ Reward delegation events match contract total perfectly!");
            }

            if total_staked != contract_total {
                let diff = if total_staked > contract_total {
                    format!("{} ZKC too high", format_zkc(total_staked - contract_total))
                } else {
                    format!("{} ZKC too low", format_zkc(contract_total - total_staked))
                };
                println!("\n📊 Stake events total is {} compared to contract", diff);
            }
        }

        Ok(())
    }

    async fn process_povw_work<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        work_log_id: Address,
        work_logs: &[Log],
    ) -> anyhow::Result<()> {
        print_section_header("POVW WORK INFORMATION");

        // Get current epoch and emissions
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let povw_emissions = zkc.getPoVWEmissionsForEpoch(current_epoch).call().await?;

        // Get pending epoch info
        let povw_accounting = IPovwAccounting::new(deployment.povw_accounting_address, provider);
        let pending_epoch = povw_accounting.pendingEpoch().call().await?;
        let total_work_u256 = U256::from(pending_epoch.totalWork);

        // Get rewards contract for cap queries
        let rewards = IRewards::new(deployment.vezkc_address, provider);

        println!("Current epoch: {}", current_epoch);
        println!("Pending epoch: {} (total work: {})", pending_epoch.number, total_work_u256);
        println!("PoVW emissions for epoch {}: {} ZKC", current_epoch, format_zkc(povw_emissions));

        // Process work by both work_log_id and recipient
        let mut work_by_work_log_id_current: HashMap<Address, U256> = HashMap::new();
        let mut work_by_work_log_id_all: HashMap<Address, U256> = HashMap::new();
        let mut work_by_recipient_current: HashMap<Address, U256> = HashMap::new();
        let mut work_by_recipient_all: HashMap<Address, U256> = HashMap::new();
        let mut work_log_id_to_recipient: HashMap<Address, Address> = HashMap::new();
        let mut my_work_current = U256::ZERO;

        for log in work_logs {
            if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
                let log_work_log_id = decoded.inner.data.workLogId;
                let epoch_number = U256::from(decoded.inner.data.epochNumber);
                let update_value = U256::from(decoded.inner.data.updateValue);
                let value_recipient = decoded.inner.data.valueRecipient;

                // Track the mapping from work log ID to recipient
                work_log_id_to_recipient.insert(log_work_log_id, value_recipient);

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
        let mut total_dao_rewards = U256::ZERO;
        if !work_by_work_log_id_current.is_empty() {
            let total_current_work: U256 = work_by_work_log_id_current.values().sum();

            println!("\nWork by Work Log ID (Current Epoch {}, Total Work: {}):", current_epoch, total_current_work);
            let mut builder = Builder::default();
            builder.push_record(["Work Log ID", "Work", "Percentage", "Projected Max Rewards", "Actual Rewards", "Cap Status"]);

            let mut sorted_current: Vec<_> = work_by_work_log_id_current.iter().collect();
            sorted_current.sort_by(|a, b| b.1.cmp(a.1));

            for (wid, work) in sorted_current.iter() {
                let percentage = if total_current_work > U256::ZERO {
                    (**work * U256::from(10000) / total_current_work).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };

                // Calculate projected rewards and check cap
                let projected_rewards = if total_current_work > U256::ZERO && total_work_u256 > U256::ZERO {
                    povw_emissions * **work / total_work_u256
                } else {
                    U256::ZERO
                };

                let reward_cap = rewards.getPoVWRewardCap(**wid).call().await.unwrap_or(U256::ZERO);
                let (actual_rewards, cap_status) = if projected_rewards > reward_cap && reward_cap > U256::ZERO {
                    total_dao_rewards += projected_rewards - reward_cap;
                    (reward_cap, "CAPPED")
                } else {
                    (projected_rewards, "OK")
                };

                builder.push_record([
                    format!("{:#x}", wid),
                    format!("{}", work),
                    format!("{:.2}%", percentage),
                    format!("{} ZKC", format_zkc(projected_rewards)),
                    format!("{} ZKC", format_zkc(actual_rewards)),
                    cap_status.to_string()
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
            builder.push_record(["Recipient", "Work", "Percentage", "Projected Max Rewards", "Actual Rewards", "Cap Status"]);

            let mut sorted_current: Vec<_> = work_by_recipient_current.iter().collect();
            sorted_current.sort_by(|a, b| b.1.cmp(a.1));

            for (recipient, work) in sorted_current.iter() {
                let percentage = if total_current_work > U256::ZERO {
                    (**work * U256::from(10000) / total_current_work).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };

                // Calculate projected rewards (recipients receive the rewards, not work log IDs)
                let projected_rewards = if total_current_work > U256::ZERO && total_work_u256 > U256::ZERO {
                    povw_emissions * **work / total_work_u256
                } else {
                    U256::ZERO
                };

                // For recipients, we need to sum up the capped rewards from all their work log IDs
                let mut actual_rewards = U256::ZERO;
                let mut is_any_capped = false;
                for (wid, wid_work) in work_by_work_log_id_current.iter() {
                    if work_log_id_to_recipient.get(wid) == Some(recipient) {
                        let wid_projected = if total_current_work > U256::ZERO && total_work_u256 > U256::ZERO {
                            povw_emissions * *wid_work / total_work_u256
                        } else {
                            U256::ZERO
                        };
                        let reward_cap = rewards.getPoVWRewardCap(*wid).call().await.unwrap_or(U256::ZERO);
                        if wid_projected > reward_cap && reward_cap > U256::ZERO {
                            actual_rewards += reward_cap;
                            is_any_capped = true;
                        } else {
                            actual_rewards += wid_projected;
                        }
                    }
                }

                let cap_status = if is_any_capped {
                    "CAPPED"
                } else if projected_rewards == actual_rewards {
                    "OK"
                } else {
                    "MIXED"
                };

                builder.push_record([
                    format!("{:#x}", recipient),
                    format!("{}", work),
                    format!("{:.2}%", percentage),
                    format!("{} ZKC", format_zkc(projected_rewards)),
                    format!("{} ZKC", format_zkc(actual_rewards)),
                    cap_status.to_string()
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

        // Display DAO rewards from capped amounts
        if total_dao_rewards > U256::ZERO {
            println!("\n============================================");
            println!("DAO REWARDS FROM CAPPED POVW");
            println!("============================================");
            println!("Total rewards redirected to DAO: {} ZKC", format_zkc(total_dao_rewards));
            println!("(These are rewards that exceeded individual caps and are now retained by the DAO)");
        }

        Ok(())
    }

    fn process_rewards_claimed(
        &self,
        povw_claims_logs: &[Log],
        staking_claims_logs: &[Log],
    ) -> anyhow::Result<()> {
        print_section_header("REWARDS CLAIMED HISTORY");

        let mut povw_claims: HashMap<Address, U256> = HashMap::new();
        for log in povw_claims_logs {
            if let Ok(decoded) = log.log_decode::<IZKC::PoVWRewardsClaimed>() {
                let recipient = decoded.inner.data.recipient;
                let amount = decoded.inner.data.amount;
                *povw_claims.entry(recipient).or_insert(U256::ZERO) += amount;
            }
        }

        let mut staking_claims: HashMap<Address, U256> = HashMap::new();
        for log in staking_claims_logs {
            if let Ok(decoded) = log.log_decode::<IZKC::StakingRewardsClaimed>() {
                let recipient = decoded.inner.data.recipient;
                let amount = decoded.inner.data.amount;
                *staking_claims.entry(recipient).or_insert(U256::ZERO) += amount;
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