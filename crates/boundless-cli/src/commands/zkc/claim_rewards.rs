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
};
use anyhow::{ensure, Context};
use boundless_zkc::{
    contracts::{extract_tx_logs, IStakingRewards, IZKC},
    deployments::Deployment,
};
use clap::Args;

use crate::config::GlobalConfig;

/// Command to claim rewards for ZKC.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcClaimRewards {
    /// Address to claim rewards for.
    pub account: Address,
    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,
}

impl ZkcClaimRewards {
    /// Run the [ZkcClaimRewards] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let tx_signer = global_config.require_private_key()?;
        let rpc_url = global_config.require_rpc_url()?;

        // Connect to the chain.
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;
        let deployment = self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id))
            .context("could not determine ZKC deployment from chain ID; please specify deployment explicitly")?;

        let total = claim_rewards(
            provider,
            deployment.staking_rewards_address,
            self.account,
            global_config,
        )
        .await?;
        tracing::info!("Claimed rewards: {} ZKC", format_ether(total));

        Ok(())
    }
}

/// Claim rewards for a specified address.
pub async fn claim_rewards(
    provider: impl Provider,
    staking_rewards_address: Address,
    account: Address,
    global_config: &GlobalConfig,
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
    let tx_result = staking
        .claimRewards(unclaimed_epochs)
        .send()
        .await
        .context("Failed to send claimRewards transaction")?;

    let tx_hash = tx_result.tx_hash();
    tracing::info!(%tx_hash, "Sent transaction for claimRewards");

    let timeout = global_config.tx_timeout.or(tx_result.timeout());
    tracing::debug!(?timeout, %tx_hash, "Waiting for transaction receipt");
    let tx_receipt = tx_result
        .with_timeout(timeout)
        .get_receipt()
        .await
        .context("Failed to receive receipt claimRewards transaction")?;

    ensure!(
        tx_receipt.status(),
        "claimRewards transaction failed: tx_hash = {}",
        tx_receipt.transaction_hash
    );

    let logs = extract_tx_logs::<IZKC::StakingRewardsClaimed>(&tx_receipt)?;
    let total = logs.into_iter().map(|log| (U256::from(log.data().amount))).sum::<U256>();

    Ok(total)
}
