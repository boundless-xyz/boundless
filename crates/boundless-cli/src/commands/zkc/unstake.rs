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
    consensus::BlockHeader,
    eips::BlockNumberOrTag,
    network::Ethereum,
    providers::{PendingTransactionBuilder, Provider, ProviderBuilder},
};
use anyhow::{ensure, Context};
use boundless_zkc::{
    contracts::{DecodeRevert, IStaking},
    deployments::Deployment,
};
use chrono::DateTime;
use clap::Args;

use crate::{
    commands::zkc::{get_active_token_id, get_staked_amount},
    config::GlobalConfig,
};

/// Command to unstake ZKC.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcUnstake {
    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,
}

impl ZkcUnstake {
    /// Run the [ZkcUnstake] command.
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

        let token_id =
            get_active_token_id(provider.clone(), deployment.vezkc_address, tx_signer.address())
                .await?;
        if token_id.is_zero() {
            anyhow::bail!("No active staking found");
        }

        let (amount, withdrawable_at) =
            get_staked_amount(provider.clone(), deployment.vezkc_address, tx_signer.address())
                .await?;

        if amount.is_zero() {
            anyhow::bail!("No staked amount found");
        }

        let send_result = if withdrawable_at.is_zero() {
            self.initiate_unstake(provider.clone(), deployment).await
        } else {
            let block_timestamp = get_block_timestamp(provider.clone()).await?;
            let withdrawable_at = u64::try_from(withdrawable_at)?;
            if block_timestamp < withdrawable_at {
                let datetime = DateTime::from_timestamp(withdrawable_at as i64, 0)
                    .context("failed to create DateTime")?;
                anyhow::bail!(
                    "Unstaking initiated. Withdrawal period ends at UTC: {}",
                    datetime.format("%Y-%m-%d %H:%M:%S")
                );
            }
            self.complete_unstake(provider.clone(), deployment).await
        };
        let pending_tx = send_result.maybe_decode_revert::<IStaking::IStakingErrors>()?;

        tracing::debug!("Broadcasting unstake deposit tx {}", pending_tx.tx_hash());
        let tx_hash = pending_tx.tx_hash();
        tracing::info!(%tx_hash, "Sent transaction for unstaking");

        let timeout = global_config.tx_timeout.or(pending_tx.timeout());

        tracing::debug!(?timeout, %tx_hash, "Waiting for transaction receipt");
        let tx_receipt = pending_tx
            .with_timeout(timeout)
            .get_receipt()
            .await
            .context("Failed to receive receipt unstaking transaction")?;

        ensure!(
            tx_receipt.status(),
            "Unstaking transaction failed: tx_hash = {}",
            tx_receipt.transaction_hash
        );

        tracing::info!("Unstaking completed");
        Ok(())
    }

    async fn initiate_unstake(
        &self,
        provider: impl Provider + Clone,
        deployment: Deployment,
    ) -> alloy::contract::Result<PendingTransactionBuilder<Ethereum>, alloy::contract::Error> {
        let staking = IStaking::new(deployment.vezkc_address, provider);
        staking.initiateUnstake().send().await
    }

    async fn complete_unstake(
        &self,
        provider: impl Provider + Clone,
        deployment: Deployment,
    ) -> alloy::contract::Result<PendingTransactionBuilder<Ethereum>, alloy::contract::Error> {
        let staking = IStaking::new(deployment.vezkc_address, provider);
        staking.completeUnstake().send().await
    }
}

async fn get_block_timestamp(provider: impl Provider + Clone) -> Result<u64, anyhow::Error> {
    let block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .context("failed to get block")?
        .context("failed to get block")?;
    Ok(block.header.timestamp())
}
