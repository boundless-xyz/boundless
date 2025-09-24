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

use std::io::{self, Write};

use alloy::{
    consensus::BlockHeader,
    eips::BlockNumberOrTag,
    network::Ethereum,
    primitives::{utils::format_ether, Address, U256},
    providers::{PendingTransactionBuilder, Provider, ProviderBuilder},
    sol_types::SolCall,
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
    /// Whether to only print the calldata without sending the transaction.
    #[clap(long)]
    pub calldata: bool,
    /// The account address to unstake from.
    ///
    /// Only valid when used with `--calldata`.
    #[clap(long, requires = "calldata")]
    pub from: Option<Address>,
    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,
}

impl ZkcUnstake {
    /// Run the [ZkcUnstake] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let rpc_url = global_config.require_rpc_url()?;

        // Connect to the chain.
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;
        let deployment = self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id))
            .context("could not determine ZKC deployment from chain ID; please specify deployment explicitly")?;

        let account = match &self.from {
            Some(addr) => *addr,
            None => global_config.require_private_key()?.address(),
        };

        let token_id =
            get_active_token_id(provider.clone(), deployment.vezkc_address, account).await?;
        if token_id.is_zero() {
            anyhow::bail!("No active staking found");
        }

        let (amount, withdrawable_at) =
            get_staked_amount(provider.clone(), deployment.vezkc_address, account).await?;

        if amount.is_zero() {
            anyhow::bail!("No staked amount found");
        }

        if self.calldata {
            return self.print_calldata(provider, deployment, withdrawable_at).await;
        }

        let tx_signer = global_config.require_private_key()?;
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;
        let deployment = self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id))
            .context("could not determine ZKC deployment from chain ID; please specify deployment explicitly")?;

        let send_result = if withdrawable_at.is_zero() {
            // Explain what initiating an unstake does and get explicit confirmation.
            println!(
                "You're about to initiate unstaking of your active ZKC position ({} ZKC).",
                format_ether(amount)
            );
            println!(
                "- This starts a 30-day cooldown. After it ends, you can complete the unstake process and withdraw your tokens."
            );
            println!("- Your staking position will close immediately: you'll lose rewards and voting power and stop earning rewards until you open a new position.");
            print!("Type 'yes' to confirm and continue: ");
            io::stdout().flush().ok();
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map_err(|e| anyhow::anyhow!("failed to read confirmation: {}", e))?;
            if input.trim().to_lowercase() != "yes" {
                anyhow::bail!("Unstake cancelled by user");
            }
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

    async fn print_calldata(
        &self,
        provider: impl Provider + Clone,
        deployment: Deployment,
        withdrawable_at: U256,
    ) -> anyhow::Result<()> {
        if withdrawable_at.is_zero() {
            let initiate_call = IStaking::initiateUnstakeCall {};
            println!("========= InitiateUnstake Call =========");
            println!("Contract: {}", deployment.vezkc_address);
            println!("Calldata: 0x{}", hex::encode(initiate_call.abi_encode()));
            println!("========================================");
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
            let complete_call = IStaking::completeUnstakeCall {};
            println!("========= CompleteUnstake Call =========");
            println!("Contract: {}", deployment.vezkc_address);
            println!("Calldata: 0x{}", hex::encode(complete_call.abi_encode()));
            println!("========================================");
        }
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
