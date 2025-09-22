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
    primitives::{utils::format_ether, Address, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::{BlockNumberOrTag, Log},
    sol,
};
use anyhow::Context;
use boundless_povw::{deployments::Deployment, log_updater::IPovwAccounting};
use boundless_rewards::{
    build_block_timestamp_cache, build_epoch_start_end_time_cache, build_rewards_cache,
    compute_delegation_powers_by_address, compute_povw_rewards_by_work_log_id,
    compute_staking_positions_by_address, create_block_lookup, create_emissions_lookup,
    create_epoch_lookup, create_reward_cap_lookup, create_staking_amount_lookup,
    fetch_all_event_logs, get_current_staking_aggregate, AllEventLogs, EpochPoVWRewards,
    MAINNET_FROM_BLOCK, SEPOLIA_FROM_BLOCK,
};
use boundless_zkc::contracts::{IRewards, IStaking, IStakingRewards, IZKC};
use clap::Args;
use std::collections::{HashMap, HashSet};
use tabled::{builder::Builder, settings::Style};

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
    tracing::info!("");
    tracing::info!("{}", "=".repeat(width));
    tracing::info!("{}{}{}", " ".repeat(padding), title, " ".repeat(padding + extra));
    tracing::info!("{}", "=".repeat(width));
}

use crate::config::GlobalConfig;

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
            tracing::warn!(
                "ZKC summary is optimized for Ethereum Mainnet (chain ID 1) or Sepolia (chain ID 11155111). Current chain ID: {}. If this is unexpected, verify the RPC URL you are using.",
                chain_id
            );
        }

        // Get deployment configuration
        let deployment = Deployment::from_chain_id(chain_id)
            .context("could not determine PoVW deployment from chain ID")?;

        // Get ZKC deployment for reward claims
        let zkc_deployment = boundless_zkc::deployments::Deployment::from_chain_id(chain_id)
            .context("could not determine ZKC deployment from chain ID")?;

        // Get the appropriate from_block based on chain ID
        let from_block_num = match chain_id {
            1 => MAINNET_FROM_BLOCK,        // Mainnet
            11155111 => SEPOLIA_FROM_BLOCK, // Sepolia
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
        )
        .await?;

        print_section_header("ZKC SUMMARY");
        tracing::info!("Work Log ID: {:#x}", work_log_id);
        tracing::info!("Chain ID: {}", chain_id);

        // Get and display epoch end time
        let zkc = IZKC::new(zkc_deployment.zkc_address, &provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let epoch_end_timestamp = zkc.getCurrentEpochEndTime().call().await?;

        // Convert timestamp to human readable format
        let epoch_end_secs = epoch_end_timestamp.to::<u64>();
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        let time_remaining_secs = epoch_end_secs.saturating_sub(now);

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
                format!(
                    "{}, {}, {} from now",
                    format_unit(days, "day"),
                    format_unit(hours, "hour"),
                    format_unit(minutes, "minute")
                )
            } else if hours > 0 {
                format!(
                    "{}, {} from now",
                    format_unit(hours, "hour"),
                    format_unit(minutes, "minute")
                )
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

        tracing::info!("Current Epoch: {}", current_epoch);
        tracing::info!("Epoch Ends: {} ({})", epoch_end_datetime, time_remaining_str);

        // Process and display Personal Summary
        self.process_personal_summary(&provider, &deployment, work_log_id, &all_logs).await?;

        // Process staking rewards details
        self.process_staking_rewards(&provider, &deployment, work_log_id, &all_logs).await?;

        // Display PoVW projections
        self.display_povw_projections(&provider, &deployment, work_log_id, &all_logs).await?;

        // Process vote and reward delegation information
        let reward_powers = self
            .process_delegation_info(
                &provider,
                &deployment,
                &zkc_deployment,
                &all_logs.vote_delegation_change_logs,
                &all_logs.reward_delegation_change_logs,
                &all_logs.vote_power_logs,
                &all_logs.reward_power_logs,
            )
            .await?;

        // Process stake positions
        self.process_stake_positions(
            &provider,
            &deployment,
            &zkc_deployment,
            &reward_powers,
            &all_logs,
        )
        .await?;

        // Process PoVW work information
        self.process_povw_work(
            &provider,
            &deployment,
            work_log_id,
            &all_logs.work_logs,
            &all_logs.epoch_finalized_logs,
        )
        .await?;

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
        let result = staking.getStakedAmountAndWithdrawalTime(work_log_id).call().await?;
        let staked_amount = result.amount;

        // Get ZKC deployment for staking rewards
        let zkc_deployment =
            boundless_zkc::deployments::Deployment::from_chain_id(provider.get_chain_id().await?)
                .context("Could not determine ZKC deployment")?;

        // Get current epoch
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let previous_epoch =
            if current_epoch > U256::from(1) { current_epoch - U256::from(1) } else { U256::ZERO };

        // Get pending staking rewards for current epoch
        let staking_rewards_ext =
            IStakingRewardsExt::new(zkc_deployment.staking_rewards_address, provider);
        let pending_staking_rewards =
            staking_rewards_ext.getPendingRewards(work_log_id).call().await?;

        // Calculate unclaimed staking rewards
        let staking_rewards =
            IStakingRewards::new(zkc_deployment.staking_rewards_address, provider);
        let epochs: Vec<U256> = (0..current_epoch.to::<u32>()).map(U256::from).collect();
        let unclaimed_staking = if !epochs.is_empty() {
            let unclaimed_rewards =
                staking_rewards.calculateUnclaimedRewards(work_log_id, epochs).call().await?;
            unclaimed_rewards.iter().sum()
        } else {
            U256::ZERO
        };

        // Build cache for PoVW rewards computation
        let epochs_to_process = vec![current_epoch.to::<u64>()];
        let work_log_ids = vec![work_log_id];
        let povw_cache = build_rewards_cache(
            provider,
            deployment,
            *zkc.address(),
            &epochs_to_process,
            &work_log_ids,
            current_epoch.to::<u64>(),
        )
        .await?;

        // Create lookup closures from cache
        let get_emissions = create_emissions_lookup(&povw_cache);
        let get_reward_cap = create_reward_cap_lookup(&povw_cache);
        // Wrap the deprecated staking lookup to add epoch parameter (ignored)
        let old_lookup = create_staking_amount_lookup(&povw_cache);
        let get_staking_amount = move |address: Address, _epoch: u64| old_lookup(address);

        // Calculate PoVW rewards for current epoch using the cached function
        let current_epoch_rewards = compute_povw_rewards_by_work_log_id(
            provider,
            deployment,
            current_epoch,
            current_epoch,
            &logs.work_logs,
            &logs.epoch_finalized_logs,
            get_emissions,
            get_reward_cap,
            get_staking_amount,
        )
        .await?;

        // Get rewards for this specific work_log_id
        let my_current_epoch_info = current_epoch_rewards.rewards_by_work_log_id.get(&work_log_id);
        let _my_work_current = my_current_epoch_info.map_or(U256::ZERO, |info| info.work);
        let projected_povw_rewards =
            my_current_epoch_info.map_or(U256::ZERO, |info| info.capped_rewards);
        let raw_povw_rewards_value =
            my_current_epoch_info.map_or(U256::ZERO, |info| info.proportional_rewards);
        let is_povw_capped = my_current_epoch_info.is_some_and(|info| info.is_capped);
        let _reward_cap = my_current_epoch_info.map_or(U256::ZERO, |info| info.reward_cap);

        // Calculate previous epoch PoVW rewards if available
        let mut my_work_previous = U256::ZERO;
        let mut previous_epoch_povw_rewards = U256::ZERO;

        if previous_epoch > U256::ZERO {
            // Build cache for previous epoch
            let prev_epochs_to_process = vec![previous_epoch.to::<u64>()];
            let prev_povw_cache = build_rewards_cache(
                provider,
                deployment,
                *zkc.address(),
                &prev_epochs_to_process,
                &work_log_ids,
                current_epoch.to::<u64>(),
            )
            .await?;

            // Create lookup closures from cache
            let prev_get_emissions = create_emissions_lookup(&prev_povw_cache);
            let prev_get_reward_cap = create_reward_cap_lookup(&prev_povw_cache);
            // Wrap the deprecated staking lookup to add epoch parameter (ignored)
            let old_lookup = create_staking_amount_lookup(&prev_povw_cache);
            let prev_get_staking_amount = move |address: Address, _epoch: u64| old_lookup(address);

            let previous_epoch_rewards = compute_povw_rewards_by_work_log_id(
                provider,
                deployment,
                previous_epoch,
                current_epoch,
                &logs.work_logs,
                &logs.epoch_finalized_logs,
                prev_get_emissions,
                prev_get_reward_cap,
                prev_get_staking_amount,
            )
            .await?;

            // Get rewards for this specific work_log_id
            if let Some(my_prev_epoch_info) =
                previous_epoch_rewards.rewards_by_work_log_id.get(&work_log_id)
            {
                my_work_previous = my_prev_epoch_info.work;
                previous_epoch_povw_rewards = my_prev_epoch_info.capped_rewards;
            }
        }

        // Display summary
        tracing::info!("Staked ZKC: {} ZKC", format_zkc(staked_amount));
        tracing::info!(
            "Projected Staking Rewards for epoch {}: {} ZKC",
            current_epoch,
            format_zkc(pending_staking_rewards)
        );

        if is_povw_capped {
            tracing::warn!(
                "Projected PoVW Rewards for epoch {}: {} ZKC ⚠️ CAPPED (uncapped: {} ZKC)",
                current_epoch,
                format_zkc(projected_povw_rewards),
                format_zkc(raw_povw_rewards_value)
            );
            tracing::info!(
                "                                      → Stake more ZKC to raise your reward cap"
            );
        } else {
            tracing::info!(
                "Projected PoVW Rewards for epoch {}: {} ZKC",
                current_epoch,
                format_zkc(projected_povw_rewards)
            );
        }

        // Display previous epoch PoVW rewards if available
        if previous_epoch > U256::ZERO && my_work_previous > U256::ZERO {
            tracing::info!(
                "Previous Epoch {} PoVW Rewards (claimable): {} ZKC",
                previous_epoch,
                format_zkc(previous_epoch_povw_rewards)
            );
        } else if previous_epoch > U256::ZERO {
            tracing::info!("Previous Epoch {} PoVW Rewards: No work submitted", previous_epoch);
        }

        tracing::info!("Unclaimed Staking Rewards: {} ZKC", format_zkc(unclaimed_staking));

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
        let zkc_deployment =
            boundless_zkc::deployments::Deployment::from_chain_id(provider.get_chain_id().await?)
                .context("Could not determine ZKC deployment")?;

        let staking_rewards =
            IStakingRewards::new(zkc_deployment.staking_rewards_address, provider);

        // Get current epoch
        let current_epoch: u32 = staking_rewards.getCurrentEpoch().call().await?.try_into()?;

        // Calculate unclaimed rewards for all epochs
        let epochs: Vec<U256> = (0..current_epoch).map(U256::from).collect();
        let unclaimed_rewards =
            staking_rewards.calculateUnclaimedRewards(account, epochs.clone()).call().await?;

        // Sum up the unclaimed rewards
        let total_unclaimed: U256 = unclaimed_rewards.iter().sum();

        if total_unclaimed > U256::ZERO {
            tracing::info!("Total unclaimed staking rewards: {} ZKC", format_zkc(total_unclaimed));

            // Display table of unclaimed rewards by epoch
            tracing::info!("Unclaimed Staking Rewards by Epoch:");
            let mut builder = Builder::default();
            builder.push_record(["Epoch", "Unclaimed Amount"]);

            for (epoch_idx, amount) in unclaimed_rewards.iter().enumerate() {
                if *amount > U256::ZERO {
                    builder.push_record([
                        epoch_idx.to_string(),
                        format!("{} ZKC", format_zkc(*amount)),
                    ]);
                }
            }

            let table = builder.build().with(Style::modern()).to_string();
            tracing::info!("\n{}", table);
        } else {
            tracing::info!("No unclaimed staking rewards");
        }

        // Get expected staking rewards using getPendingRewards
        // This returns the actual pending rewards for the user in the current epoch
        let staking_rewards_ext =
            IStakingRewardsExt::new(zkc_deployment.staking_rewards_address, provider);
        let pending_rewards = staking_rewards_ext.getPendingRewards(account).call().await?;

        // Get total staking emissions for context
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let total_staking_emissions =
            zkc.getStakingEmissionsForEpoch(U256::from(current_epoch)).call().await?;

        // Get total staking rewards to calculate percentage
        let rewards = IRewards::new(deployment.vezkc_address, provider);
        let user_reward_power = rewards.getStakingRewards(account).call().await?;
        let total_reward_power = rewards.getTotalStakingRewards().call().await?;

        let user_percentage = if total_reward_power > U256::ZERO {
            (user_reward_power * U256::from(10000) / total_reward_power).to::<u64>() as f64 / 100.0
        } else {
            0.0
        };

        tracing::info!("\nExpected staking rewards for epoch {}:", current_epoch);
        tracing::info!("  Total epoch emissions: {} ZKC", format_zkc(total_staking_emissions));
        tracing::info!(
            "  Your reward power: {} / {} ({:.8}%)",
            format_zkc(user_reward_power),
            format_zkc(total_reward_power),
            user_percentage
        );
        tracing::info!("  Your pending rewards: {} ZKC", format_zkc(pending_rewards));
        tracing::info!(
            "  Calculation: {} × {:.8}% = {} ZKC",
            format_zkc(total_staking_emissions),
            user_percentage,
            format_zkc(pending_rewards)
        );

        Ok(())
    }

    async fn display_povw_projections<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        work_log_id: Address,
        all_logs: &AllEventLogs,
    ) -> anyhow::Result<()> {
        // Get current epoch
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;

        // Build cache for PoVW rewards computation
        let epochs_to_process = vec![current_epoch.to::<u64>()];
        let work_log_ids = vec![work_log_id];
        let povw_cache = build_rewards_cache(
            provider,
            deployment,
            *zkc.address(),
            &epochs_to_process,
            &work_log_ids,
            current_epoch.to::<u64>(),
        )
        .await?;

        // Create lookup closures from cache
        let get_emissions = create_emissions_lookup(&povw_cache);
        let get_reward_cap = create_reward_cap_lookup(&povw_cache);
        // Wrap the deprecated staking lookup to add epoch parameter (ignored)
        let old_lookup = create_staking_amount_lookup(&povw_cache);
        let get_staking_amount = move |address: Address, _epoch: u64| old_lookup(address);

        // Calculate rewards for current epoch using the cached function
        let current_epoch_rewards = compute_povw_rewards_by_work_log_id(
            provider,
            deployment,
            current_epoch,
            current_epoch,
            &all_logs.work_logs,
            &all_logs.epoch_finalized_logs,
            get_emissions,
            get_reward_cap,
            get_staking_amount,
        )
        .await?;

        print_section_header("YOUR POVW PROJECTIONS");

        // Get info for this specific work_log_id
        if let Some(my_info) = current_epoch_rewards.rewards_by_work_log_id.get(&work_log_id) {
            let work_percentage = if current_epoch_rewards.total_work > U256::ZERO {
                (my_info.work * U256::from(10000) / current_epoch_rewards.total_work).to::<u64>()
                    as f64
                    / 100.0
            } else {
                0.0
            };

            tracing::info!(
                "Your work in current epoch: {} / {} ({:.2}%)",
                my_info.work,
                current_epoch_rewards.total_work,
                work_percentage
            );
            tracing::info!(
                "Total PoVW emissions for epoch {}: {} ZKC",
                current_epoch,
                format_zkc(current_epoch_rewards.total_emissions)
            );
            tracing::info!("Your PoVW reward cap: {} ZKC", format_zkc(my_info.reward_cap));

            if my_info.is_capped {
                tracing::warn!("\n⚠️  REWARDS WILL BE CAPPED!");
                tracing::info!(
                    "Uncapped rewards: {} ZKC",
                    format_zkc(my_info.proportional_rewards)
                );
                tracing::info!("Reward cap:       {} ZKC", format_zkc(my_info.reward_cap));
                tracing::info!("Actual rewards:   {} ZKC", format_zkc(my_info.capped_rewards));
                tracing::info!("→ Stake more ZKC to raise your reward cap");
            } else {
                tracing::info!(
                    "Projected PoVW rewards: {} ZKC",
                    format_zkc(my_info.capped_rewards)
                );
                tracing::info!(
                    "Calculation: {} × {:.2}% = {} ZKC",
                    format_zkc(current_epoch_rewards.total_emissions),
                    work_percentage,
                    format_zkc(my_info.capped_rewards)
                );
                tracing::info!(
                    "Status: ✅ Below reward cap ({} ZKC)",
                    format_zkc(my_info.reward_cap)
                );
            }
        } else {
            // No work submitted for this work_log_id
            // Still need to get the reward cap for display
            let rewards = IRewards::new(deployment.vezkc_address, provider);
            let reward_cap = rewards.getPoVWRewardCap(work_log_id).call().await?;

            tracing::info!(
                "Your work in current epoch: 0 / {} (0.00%)",
                current_epoch_rewards.total_work
            );
            tracing::info!(
                "Total PoVW emissions for epoch {}: {} ZKC",
                current_epoch,
                format_zkc(current_epoch_rewards.total_emissions)
            );
            tracing::info!("Your PoVW reward cap: {} ZKC", format_zkc(reward_cap));

            if current_epoch_rewards.total_work == U256::ZERO {
                tracing::info!("\nNo work submitted in current epoch yet");
            } else {
                tracing::info!("\nYou have not submitted any work in the current epoch");
            }
        }

        Ok(())
    }

    async fn process_delegation_info<P: Provider>(
        &self,
        provider: &P,
        _deployment: &Deployment,
        zkc_deployment: &boundless_zkc::deployments::Deployment,
        vote_delegation_change_logs: &[Log],
        reward_delegation_change_logs: &[Log],
        vote_power_logs: &[Log],
        reward_power_logs: &[Log],
    ) -> anyhow::Result<HashMap<Address, U256>> {
        print_section_header("DELEGATION TABLES");

        // Get current epoch
        let zkc = IZKC::new(zkc_deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?.to::<u64>();

        // Build caches for epoch and block timestamps
        // Collect all unique epochs from the logs first
        let mut epochs_to_process = HashSet::new();
        epochs_to_process.insert(current_epoch);
        if current_epoch > 0 {
            epochs_to_process.insert(current_epoch - 1);
        }
        let epochs_vec: Vec<u64> = epochs_to_process.into_iter().collect();
        let epoch_cache = build_epoch_start_end_time_cache(
            provider,
            zkc_deployment.zkc_address,
            &epochs_vec,
            current_epoch,
        )
        .await?;

        let all_delegation_logs: Vec<&Log> = vote_delegation_change_logs
            .iter()
            .chain(reward_delegation_change_logs.iter())
            .chain(vote_power_logs.iter())
            .chain(reward_power_logs.iter())
            .collect();

        let mut block_cache = HashMap::new();
        build_block_timestamp_cache(provider, &all_delegation_logs, &mut block_cache).await?;

        // Create lookup closures
        let get_epoch = create_epoch_lookup(&epoch_cache);
        let get_timestamp = create_block_lookup(&block_cache);

        // Use the refactored function to compute delegation powers by epoch
        let epoch_delegation_powers = compute_delegation_powers_by_address(
            vote_delegation_change_logs,
            reward_delegation_change_logs,
            vote_power_logs,
            reward_power_logs,
            get_epoch,
            get_timestamp,
            current_epoch,
        )?;

        // Get current state from latest epoch for display
        let (vote_powers, reward_powers) = if let Some(latest) = epoch_delegation_powers.last() {
            let mut vote_powers = HashMap::new();
            let mut reward_powers = HashMap::new();

            for (address, powers) in &latest.powers {
                if powers.vote_power > U256::ZERO {
                    vote_powers.insert(*address, powers.vote_power);
                }
                if powers.reward_power > U256::ZERO {
                    reward_powers.insert(*address, powers.reward_power);
                }
            }

            (vote_powers, reward_powers)
        } else {
            (HashMap::new(), HashMap::new())
        };

        // Helper function to display delegation power tables
        fn display_delegation_table(
            powers: &HashMap<Address, U256>,
            title: &str,
            column_name: &str,
            description: &str,
        ) {
            if powers.is_empty() {
                return;
            }

            let total_power: U256 = powers.values().sum();

            tracing::info!("{} (Total: {} ZKC):", title, format_zkc(total_power));
            tracing::info!("{}", description);

            let mut builder = Builder::default();
            builder.push_record(["Address", column_name, "Percentage"]);

            let mut sorted_powers: Vec<_> = powers.iter().collect();
            sorted_powers.sort_by(|a, b| b.1.cmp(a.1));

            // Show top 20 for brevity
            for (address, power) in sorted_powers.iter().take(20) {
                let percentage = if total_power > U256::ZERO {
                    (**power * U256::from(10000) / total_power).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };
                builder.push_record([
                    format!("{:#x}", address),
                    format!("{} ZKC", format_zkc(**power)),
                    format!("{:.2}%", percentage),
                ]);
            }

            if sorted_powers.len() > 20 {
                builder.push_record([
                    format!("... and {} more", sorted_powers.len() - 20),
                    "".to_string(),
                    "".to_string(),
                ]);
            }

            let table = builder.build().with(Style::modern()).to_string();
            tracing::info!("\n{}", table);
        }

        // Display vote power table
        display_delegation_table(
            &vote_powers,
            "Delegated Vote Power",
            "Vote Power",
            "Shows who can vote on governance proposals",
        );

        // Display reward power table
        display_delegation_table(
            &reward_powers,
            "Delegated Reward Power",
            "Reward Power",
            "Shows who can claim staking rewards",
        );

        // Return reward powers for use in stake positions calculation
        Ok(reward_powers)
    }

    async fn process_stake_positions<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        zkc_deployment: &boundless_zkc::deployments::Deployment,
        reward_powers: &HashMap<Address, U256>,
        all_logs: &AllEventLogs,
    ) -> anyhow::Result<()> {
        print_section_header("STAKE POSITIONS");

        // Get current epoch
        let zkc = IZKC::new(zkc_deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?.to::<u64>();

        // Build caches for epoch and block timestamps
        // Collect all unique epochs from the logs first
        let mut epochs_to_process = HashSet::new();
        epochs_to_process.insert(current_epoch);
        if current_epoch > 0 {
            epochs_to_process.insert(current_epoch - 1);
        }
        let epochs_vec: Vec<u64> = epochs_to_process.into_iter().collect();
        let epoch_cache = build_epoch_start_end_time_cache(
            provider,
            zkc_deployment.zkc_address,
            &epochs_vec,
            current_epoch,
        )
        .await?;

        let all_staking_logs: Vec<&Log> = all_logs
            .stake_created_logs
            .iter()
            .chain(all_logs.stake_added_logs.iter())
            .chain(all_logs.unstake_initiated_logs.iter())
            .chain(all_logs.unstake_completed_logs.iter())
            .chain(all_logs.reward_delegation_change_logs.iter())
            .collect();

        let mut block_cache = HashMap::new();
        build_block_timestamp_cache(provider, &all_staking_logs, &mut block_cache).await?;

        // Create lookup closures
        let get_epoch = create_epoch_lookup(&epoch_cache);
        let get_timestamp = create_block_lookup(&block_cache);

        // Compute staking positions synchronously
        let epoch_positions = compute_staking_positions_by_address(
            &all_logs.stake_created_logs,
            &all_logs.stake_added_logs,
            &all_logs.unstake_initiated_logs,
            &all_logs.unstake_completed_logs,
            &all_logs.vote_delegation_change_logs,
            &all_logs.reward_delegation_change_logs,
            get_epoch,
            get_timestamp,
            current_epoch,
        )?;

        // Get current aggregate state for display
        let (stakes, withdrawing) = get_current_staking_aggregate(&epoch_positions);

        // Display stakes table
        if !stakes.is_empty() {
            // Calculate total staked
            let total_staked: U256 = stakes.values().sum();
            let num_positions = stakes.len();

            // Validate against contract's getTotalStakingRewards()
            let rewards_contract = IRewards::new(deployment.vezkc_address, provider);
            let contract_total = rewards_contract.getTotalStakingRewards().call().await?;

            if contract_total != total_staked {
                tracing::warn!("⚠️  WARNING: Stake total mismatch!");
                tracing::info!("  Contract reports: {} ZKC", format_zkc(contract_total));
                tracing::info!("  Events sum to: {} ZKC", format_zkc(total_staked));
                tracing::info!(
                    "  Difference: {} ZKC",
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

            tracing::info!(
                "\nStaked Positions ({} positions, Total: {} ZKC):",
                num_positions,
                format_zkc(total_staked)
            );
            tracing::info!("Shows actual token ownership");
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
                    status.to_string(),
                ]);
            }

            let table = builder.build().with(Style::modern()).to_string();
            tracing::info!("\n{}", table);

            if withdrawing.values().any(|&v| v) {
                tracing::info!("* Withdrawal initiated but not yet completed");
            }

            // Show totals comparison
            let total_reward_power: U256 = reward_powers.values().sum();

            // Get the contract's total for validation
            let rewards_contract = IRewards::new(deployment.vezkc_address, provider);
            let contract_total = rewards_contract.getTotalStakingRewards().call().await?;

            tracing::info!("Totals Summary:");
            tracing::info!(
                "Contract Total (getTotalStakingRewards): {} ZKC",
                format_zkc(contract_total)
            );
            tracing::info!("Total from Stake Events: {} ZKC", format_zkc(total_staked));
            tracing::info!("Total Reward Power: {} ZKC", format_zkc(total_reward_power));

            // The contract total is the source of truth
            if total_reward_power != contract_total {
                let excess = total_reward_power.saturating_sub(contract_total);
                tracing::warn!(
                    "⚠️  WARNING: Reward delegation events show {} ZKC MORE power than exists!",
                    format_zkc(excess)
                );
            }

            if total_staked != contract_total {
                let diff = if total_staked > contract_total {
                    format!("{} ZKC too high", format_zkc(total_staked - contract_total))
                } else {
                    format!("{} ZKC too low", format_zkc(contract_total - total_staked))
                };
                tracing::info!("📊 Stake events total is {} compared to contract", diff);
            }
        }

        Ok(())
    }

    async fn process_povw_work<P: Provider>(
        &self,
        provider: &P,
        deployment: &Deployment,
        _work_log_id: Address,
        work_logs: &[Log],
        epoch_finalized_logs: &[Log],
    ) -> anyhow::Result<()> {
        print_section_header("POVW WORK INFORMATION");

        // Get current epoch and emissions
        let zkc = IZKC::new(deployment.zkc_address, provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let povw_emissions = zkc.getPoVWEmissionsForEpoch(current_epoch).call().await?;

        // Get previous epoch emissions if we have a previous epoch
        let previous_epoch =
            if current_epoch > U256::from(1) { current_epoch - U256::from(1) } else { U256::ZERO };
        let previous_epoch_emissions = if previous_epoch > U256::ZERO {
            zkc.getPoVWEmissionsForEpoch(previous_epoch).call().await?
        } else {
            U256::ZERO
        };

        // First, collect all unique epochs and work log IDs from work logs
        let mut epochs_with_work: HashSet<U256> = HashSet::new();
        let mut all_work_log_ids: HashSet<Address> = HashSet::new();
        for log in work_logs {
            if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
                let epoch_number = U256::from(decoded.inner.data.epochNumber);
                epochs_with_work.insert(epoch_number);
                all_work_log_ids.insert(decoded.inner.data.workLogId);
            }
        }

        // Convert to vectors for cache building
        let epochs_to_process: Vec<u64> = epochs_with_work.iter().map(|e| e.to::<u64>()).collect();
        let work_log_ids: Vec<Address> = all_work_log_ids.into_iter().collect();

        // Build cache for all epochs and work log IDs at once
        let povw_cache = build_rewards_cache(
            provider,
            deployment,
            *zkc.address(),
            &epochs_to_process,
            &work_log_ids,
            current_epoch.to::<u64>(),
        )
        .await?;

        // Create lookup closures from cache
        let get_emissions = create_emissions_lookup(&povw_cache);
        let get_reward_cap = create_reward_cap_lookup(&povw_cache);
        // Wrap the deprecated staking lookup to add epoch parameter (ignored)
        let old_lookup = create_staking_amount_lookup(&povw_cache);
        let get_staking_amount = move |address: Address, _epoch: u64| old_lookup(address);

        // Compute rewards for each epoch that has work
        let mut all_epoch_rewards: HashMap<U256, EpochPoVWRewards> = HashMap::new();
        for epoch in epochs_with_work {
            let epoch_rewards = compute_povw_rewards_by_work_log_id(
                provider,
                deployment,
                epoch,
                current_epoch,
                work_logs,
                epoch_finalized_logs,
                &get_emissions,
                &get_reward_cap,
                &get_staking_amount,
            )
            .await?;
            all_epoch_rewards.insert(epoch, epoch_rewards);
        }

        // Get current and previous epoch rewards from our computed results
        let current_epoch_rewards =
            all_epoch_rewards.get(&current_epoch).cloned().unwrap_or(EpochPoVWRewards {
                epoch: current_epoch,
                total_work: U256::ZERO,
                total_emissions: povw_emissions,
                rewards_by_work_log_id: HashMap::new(),
            });

        let previous_epoch_rewards = if previous_epoch > U256::ZERO {
            all_epoch_rewards.get(&previous_epoch).cloned()
        } else {
            None
        };

        // Get pending epoch info for display
        let povw_accounting = IPovwAccounting::new(deployment.povw_accounting_address, provider);
        let pending_epoch = povw_accounting.pendingEpoch().call().await?;

        tracing::info!("Current epoch: {}", current_epoch);
        tracing::info!(
            "Pending epoch: {} (total work: {})",
            pending_epoch.number,
            current_epoch_rewards.total_work
        );
        tracing::info!(
            "PoVW emissions for epoch {}: {} ZKC",
            current_epoch,
            format_zkc(povw_emissions)
        );
        if let Some(ref _prev_rewards) = previous_epoch_rewards {
            tracing::info!(
                "PoVW emissions for epoch {}: {} ZKC",
                previous_epoch,
                format_zkc(previous_epoch_emissions)
            );
        }

        // Build aggregate work data across all epochs
        let mut work_by_work_log_id_all: HashMap<Address, U256> = HashMap::new();
        for epoch_rewards in all_epoch_rewards.values() {
            for (wid, info) in &epoch_rewards.rewards_by_work_log_id {
                *work_by_work_log_id_all.entry(*wid).or_insert(U256::ZERO) += info.work;
            }
        }

        // Display current epoch work by work_log_id
        if !current_epoch_rewards.rewards_by_work_log_id.is_empty() {
            tracing::info!(
                "Work by Work Log ID (Num work logs {}, Current Epoch {}, Total Work: {}):",
                current_epoch_rewards.rewards_by_work_log_id.len(),
                current_epoch,
                current_epoch_rewards.total_work
            );
            let mut builder = Builder::default();
            builder.push_record([
                "Work Log ID",
                "Work",
                "Percentage",
                "Projected Max Rewards",
                "Actual Rewards",
                "Cap Status",
            ]);

            let mut sorted_current: Vec<_> =
                current_epoch_rewards.rewards_by_work_log_id.iter().collect();
            sorted_current.sort_by(|a, b| b.1.work.cmp(&a.1.work));

            for (wid, info) in sorted_current.iter() {
                let cap_status = if info.is_capped { "CAPPED" } else { "OK" };
                let percentage = if current_epoch_rewards.total_work > U256::ZERO {
                    (info.work * U256::from(10000) / current_epoch_rewards.total_work).to::<u64>()
                        as f64
                        / 100.0
                } else {
                    0.0
                };

                builder.push_record([
                    format!("{:#x}", wid),
                    format!("{}", info.work),
                    format!("{:.2}%", percentage),
                    format!("{} ZKC", format_zkc(info.proportional_rewards)),
                    format!("{} ZKC", format_zkc(info.capped_rewards)),
                    cap_status.to_string(),
                ]);
            }

            let table = builder.build().with(Style::modern()).to_string();
            tracing::info!("\n{}", table);
        }

        // Display previous epoch work by work_log_id if available
        if let Some(ref prev_rewards) = previous_epoch_rewards {
            if !prev_rewards.rewards_by_work_log_id.is_empty() {
                tracing::info!(
                    "🕐 Previous Epoch Work by Work Log ID (Num work logs {}, Epoch {}, Total Work: {}):",
                    prev_rewards.rewards_by_work_log_id.len(),
                    previous_epoch,
                    prev_rewards.total_work
                );
                tracing::info!("These rewards are finalized and can be claimed");
                let mut builder = Builder::default();
                builder.push_record([
                    "Work Log ID",
                    "Work",
                    "Percentage",
                    "Rewards Earned",
                    "Cap Status",
                ]);

                let mut sorted_previous: Vec<_> =
                    prev_rewards.rewards_by_work_log_id.iter().collect();
                sorted_previous.sort_by(|a, b| b.1.work.cmp(&a.1.work));

                for (wid, info) in sorted_previous.iter() {
                    let cap_status = if info.is_capped { "CAPPED" } else { "OK" };
                    let percentage = if prev_rewards.total_work > U256::ZERO {
                        (info.work * U256::from(10000) / prev_rewards.total_work).to::<u64>() as f64
                            / 100.0
                    } else {
                        0.0
                    };

                    builder.push_record([
                        format!("{:#x}", wid),
                        format!("{}", info.work),
                        format!("{:.2}%", percentage),
                        format!("{} ZKC", format_zkc(info.capped_rewards)),
                        cap_status.to_string(),
                    ]);
                }

                let table = builder.build().with(Style::modern()).to_string();
                tracing::info!("\n{}", table);
            }
        }

        // Calculate DAO rewards from all epochs
        let mut total_dao_rewards = U256::ZERO;
        let mut dao_rewards_by_epoch: Vec<(U256, U256)> = Vec::new();

        // Process each epoch we already computed rewards for
        for (epoch, epoch_rewards) in &all_epoch_rewards {
            // Calculate DAO rewards (sum of all capped amounts)
            let mut epoch_dao_rewards = U256::ZERO;
            for info in epoch_rewards.rewards_by_work_log_id.values() {
                if info.is_capped {
                    let dao_amount = info.proportional_rewards - info.capped_rewards;
                    epoch_dao_rewards += dao_amount;
                }
            }

            if epoch_dao_rewards > U256::ZERO {
                dao_rewards_by_epoch.push((*epoch, epoch_dao_rewards));
                total_dao_rewards += epoch_dao_rewards;
            }
        }

        // Sort by epoch for display
        dao_rewards_by_epoch.sort_by_key(|(epoch, _)| *epoch);

        // Display DAO rewards from capped amounts
        if total_dao_rewards > U256::ZERO {
            tracing::info!("============================================");
            tracing::info!("DAO REWARDS FROM CAPPED POVW (ALL EPOCHS)");
            tracing::info!("============================================");
            tracing::info!(
                "Total rewards redirected to DAO: {} ZKC",
                format_zkc(total_dao_rewards)
            );
            tracing::info!("Breakdown by epoch:");

            for (epoch, amount) in dao_rewards_by_epoch.iter() {
                tracing::info!("  Epoch {}: {} ZKC", epoch, format_zkc(*amount));
            }

            tracing::info!(
                "(These are rewards that exceeded individual caps and are retained by the DAO)"
            );
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

            tracing::info!("PoVW Rewards Claimed (Total: {} ZKC):", format_zkc(total_povw));
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
                    format!("{:.2}%", percentage),
                ]);
            }

            let table = builder.build().with(Style::modern()).to_string();
            tracing::info!("\n{}", table);
        } else {
            tracing::info!("\nNo PoVW rewards have been claimed yet");
        }

        // Display Staking rewards claimed table
        if !staking_claims.is_empty() {
            let total_staking: U256 = staking_claims.values().sum();
            let mut sorted_staking: Vec<_> = staking_claims.iter().collect();
            sorted_staking.sort_by(|a, b| b.1.cmp(a.1));

            tracing::info!("Staking Rewards Claimed (Total: {} ZKC):", format_zkc(total_staking));
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
                    format!("{:.2}%", percentage),
                ]);
            }

            let table = builder.build().with(Style::modern()).to_string();
            tracing::info!("\n{}", table);
        } else {
            tracing::info!("\nNo Staking rewards have been claimed yet");
        }

        Ok(())
    }
}
