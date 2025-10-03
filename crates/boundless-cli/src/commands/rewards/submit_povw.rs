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
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, Context, Result};
use clap::Args;
use std::path::PathBuf;

use crate::config::GlobalConfig;

/// Submit accumulated PoVW work to earn rewards
///
/// This command submits Proof of Verifiable Work (PoVW) that has been accumulated
/// from proving activities. The work is submitted to the PoVW accounting contract
/// on Ethereum mainnet to earn ZKC token rewards.
#[derive(Args, Clone, Debug)]
pub struct RewardsSubmitPovw {
    /// Path to the PoVW state file containing work receipts
    #[arg(long, env = "POVW_STATE_PATH")]
    pub state_path: PathBuf,

    /// Private key for the work log (defaults to prover private key)
    #[arg(long, env = "POVW_PRIVATE_KEY")]
    pub povw_private_key: Option<String>,

    /// Recipient address for rewards (defaults to work log owner)
    #[arg(long)]
    pub recipient: Option<Address>,

    /// Dry run mode - simulate submission without sending transaction
    #[arg(long)]
    pub dry_run: bool,
}

impl RewardsSubmitPovw {
    /// Run the submit-povw command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        // Get RPC URL and verify chain
        let rpc_url = global_config
            .require_rpc_url()
            .context("ETH_MAINNET_RPC_URL is required for rewards commands")?;

        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        if chain_id != 1 {
            bail!("PoVW submission requires connection to Ethereum mainnet (chain ID 1), got chain ID {}", chain_id);
        }

        // Verify state file exists
        if !self.state_path.exists() {
            bail!("PoVW state file not found at {:?}. Run 'boundless povw prepare' first to create it.", self.state_path);
        }

        // Get work log signer
        let work_log_private_key = if let Some(key) = &self.povw_private_key {
            key.clone()
        } else {
            global_config
                .work_log_private_key()
                .or_else(|_| global_config.require_prover_private_key())
                .context(
                    "No PoVW private key provided. Set POVW_PRIVATE_KEY or PROVER_PRIVATE_KEY",
                )?
        };

        let work_log_signer: PrivateKeySigner =
            work_log_private_key.parse().context("Failed to parse work log private key")?;

        // Get recipient address (defaults to work log owner)
        let recipient = self.recipient.unwrap_or(work_log_signer.address());

        if self.dry_run {
            tracing::info!("DRY RUN MODE - No transaction will be sent");
            tracing::info!("Work log owner: {:#x}", work_log_signer.address());
            tracing::info!("Reward recipient: {:#x}", recipient);
            tracing::info!("State file: {:?}", self.state_path);

            // TODO: Load state file and display work summary
            tracing::info!("Would submit PoVW work from state file");
            tracing::info!("Note: Full implementation requires integration with povw crate");

            return Ok(());
        }

        // TODO: Implement actual submission once PoVW contracts are deployed
        // This would involve:
        // 1. Loading the state file with accumulated work
        // 2. Creating the log updater proof with recipient
        // 3. Submitting to the PoVW accounting contract
        // 4. Waiting for transaction confirmation

        tracing::warn!("Note: PoVW submission functionality not yet fully implemented");
        tracing::info!("This command will submit work from {:?}", self.state_path);
        tracing::info!("Work log owner: {:#x}", work_log_signer.address());
        tracing::info!("Rewards will be sent to: {:#x}", recipient);
        tracing::info!("For now, use 'boundless povw submit' for full functionality");

        Ok(())
    }
}
