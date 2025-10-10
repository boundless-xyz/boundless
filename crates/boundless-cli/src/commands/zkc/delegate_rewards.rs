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
    primitives::Address,
    providers::{Provider, ProviderBuilder},
    sol_types::SolCall,
};
use anyhow::Context;
use boundless_zkc::{contracts::IRewards, deployments::Deployment};
use clap::Args;

use crate::{
    config::{GlobalConfig, RewardsConfig},
    config_ext::RewardsConfigExt,
    contracts::confirm_transaction,
    display::DisplayManager,
};

/// Command to delegate rewards for ZKC.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcDelegateRewards {
    /// Address to delegate rewards to.
    pub to: Address,
    /// Whether to only print the calldata without sending the transaction.
    #[clap(long)]
    pub calldata: bool,
    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,

    /// Rewards configuration (RPC URL, private key, etc.)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl ZkcDelegateRewards {
    /// Run the [DelegateRewards] command.
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

        if self.calldata {
            print_calldata(&deployment, self.to);
            return Ok(());
        }

        let provider_with_wallet = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;

        let display = DisplayManager::new();
        let rewards = IRewards::new(deployment.vezkc_address, provider_with_wallet);

        let pending_tx = rewards
            .delegateRewards(self.to)
            .send()
            .await
            .context("Failed to send delegateRewards transaction")?;

        let tx_hash = *pending_tx.tx_hash();
        display.tx_hash(tx_hash);
        display.status("Status", "Waiting for confirmation", "yellow");

        confirm_transaction(pending_tx, global_config.tx_timeout, 1).await?;

        display.success("Rewards delegation completed");
        display.header("Delegation Details");
        display.address("Delegated to", self.to);
        Ok(())
    }
}

fn print_calldata(deployment: &Deployment, delegatee: Address) {
    let delegate_call = IRewards::delegateRewardsCall { delegatee };
    println!("========= DelegateRewards Call =========");
    println!("target address: {}", deployment.vezkc_address);
    println!("calldata: 0x{}", hex::encode(delegate_call.abi_encode()));
}
