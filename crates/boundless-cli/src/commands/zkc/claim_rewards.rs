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
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder},
    sol_types::SolCall,
};
use anyhow::{ensure, Context};
use boundless_zkc::{
    contracts::{extract_tx_logs, IStakingRewards, IZKC},
    deployments::Deployment,
};
use clap::Args;

use crate::{
    config::{GlobalConfig, RewardsConfig},
    config_ext::RewardsConfigExt,
    contracts::confirm_transaction,
    display::{format_eth, DisplayManager},
};

/// Command to claim rewards for ZKC.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcClaimRewards {
    /// Whether to only print the calldata without sending the transaction.
    #[clap(long)]
    pub calldata: bool,
    /// The account address to claim rewards from.
    ///
    /// Only valid when used with `--calldata`.
    #[clap(long, requires = "calldata")]
    pub from: Option<Address>,
    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,

    /// Rewards configuration (RPC URL, private key, etc.)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl ZkcClaimRewards {
    /// Run the [ZkcClaimRewards] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url_with_help()?;
        let tx_signer = rewards_config.require_staking_key_with_help()?;

        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;
        let deployment = rewards_config.get_zkc_deployment(chain_id)?;

        let account = self.from.unwrap_or_else(|| tx_signer.address());

        if self.calldata {
            return print_calldata(provider, deployment.clone(), account).await;
        }

        let provider_with_wallet = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;

        let display = DisplayManager::new();
        let total =
            claim_rewards(provider_with_wallet, deployment.staking_rewards_address, account, global_config, &display)
                .await?;

        display.success("Rewards claimed successfully");
        display.header("Claim Details");
        display.balance("Total Claimed", &format_eth(total), "ZKC", "green");

        Ok(())
    }
}

async fn print_calldata(
    provider: impl Provider + Clone,
    deployment: Deployment,
    from: Address,
) -> anyhow::Result<()> {
    let staking = IStakingRewards::new(deployment.staking_rewards_address, provider);
    let current_epoch: u32 = staking.getCurrentEpoch().call().await?.try_into()?;
    let epochs: Vec<U256> = (0..current_epoch).map(U256::from).collect();
    let unclaimed_rewards = staking.calculateUnclaimedRewards(from, epochs).call().await?;
    let mut unclaimed_epochs = vec![];
    for (i, unclaimed_reward) in unclaimed_rewards.iter().enumerate() {
        if *unclaimed_reward > U256::ZERO {
            unclaimed_epochs.push(U256::from(i));
        }
    }
    ensure!(!unclaimed_epochs.is_empty(), "No unclaimed rewards for account {}", from);

    let claim_call = IStakingRewards::claimRewardsCall { epochs: unclaimed_epochs };
    println!("========= ClaimRewards Call =========");
    println!("Contract: {}", deployment.staking_rewards_address);
    println!("From: {}", from);
    println!("Calldata: 0x{}", hex::encode(claim_call.abi_encode()));
    println!("=====================================");
    Ok(())
}

/// Claim rewards for a specified address.
pub async fn claim_rewards(
    provider: impl Provider,
    staking_rewards_address: Address,
    account: Address,
    global_config: &GlobalConfig,
    display: &DisplayManager,
) -> anyhow::Result<U256> {
    let staking = IStakingRewards::new(staking_rewards_address, provider);
    let current_epoch: u32 = staking.getCurrentEpoch().call().await?.try_into()?;
    let epochs: Vec<U256> = (0..current_epoch).map(U256::from).collect();
    let unclaimed_rewards = staking.calculateUnclaimedRewards(account, epochs).call().await?;
    let mut unclaimed_epochs = vec![];
    for (i, unclaimed_reward) in unclaimed_rewards.iter().enumerate() {
        if *unclaimed_reward > U256::ZERO {
            unclaimed_epochs.push(U256::from(i));
        }
    }
    ensure!(!unclaimed_epochs.is_empty(), "No unclaimed rewards for account {}", account);

    let pending_tx = staking
        .claimRewards(unclaimed_epochs)
        .send()
        .await
        .context("Failed to send claimRewards transaction")?;

    let tx_hash = *pending_tx.tx_hash();
    display.tx_hash(tx_hash);
    display.status("Status", "Waiting for confirmation", "yellow");

    let tx_receipt = confirm_transaction(pending_tx, global_config.tx_timeout, 1).await?;

    let logs = extract_tx_logs::<IZKC::StakingRewardsClaimed>(&tx_receipt)?;
    let total = logs.into_iter().map(|log| (U256::from(log.data().amount))).sum::<U256>();

    Ok(total)
}
