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

use alloy::{primitives::Address, providers::ProviderBuilder};
use anyhow::{ensure, Context};
use boundless_zkc::contracts::IRewards;
use clap::Args;

use crate::config::GlobalConfig;

/// Command to delegate rewards for ZKC.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct ZkcDelegateRewards {
    /// Address to delegate rewards to.
    pub to: Address,
    /// Address of the [IRewards] contract.
    #[clap(long, env = "VEZKC_ADDRESS")]
    pub vezkc_address: Address,
}

impl ZkcDelegateRewards {
    /// Run the [DelegateRewards] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let tx_signer = global_config.require_private_key()?;
        let rpc_url = global_config.require_rpc_url()?;

        // Connect to the chain.
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;

        let rewards = IRewards::new(self.vezkc_address, provider.clone());

        let tx_result = rewards
            .delegateRewards(self.to)
            .send()
            .await
            .context("Failed to send delegateRewards transaction")?;
        let tx_hash = tx_result.tx_hash();
        tracing::info!(%tx_hash, "Sent transaction for delegating rewards");

        let timeout = global_config.tx_timeout.or(tx_result.timeout());
        tracing::debug!(?timeout, %tx_hash, "Waiting for transaction receipt");
        let tx_receipt = tx_result
            .with_timeout(timeout)
            .get_receipt()
            .await
            .context("Failed to receive receipt staking transaction")?;

        ensure!(
            tx_receipt.status(),
            "Delegating rewards transaction failed: tx_hash = {}",
            tx_receipt.transaction_hash
        );

        // TODO(povw): Display some info
        tracing::info!("Delegating rewards completed");
        Ok(())
    }
}
