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

use alloy::primitives::{B256, U256};
use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, ProverConfig};

/// Lock a request in the market
#[derive(Args, Clone, Debug)]
pub struct ProverLock {
    /// The proof request identifier
    #[arg(long)]
    pub request_id: U256,

    /// The request digest
    #[arg(long)]
    pub request_digest: Option<B256>,

    /// The tx hash of the request submission
    #[arg(long)]
    pub tx_hash: Option<B256>,

    /// Prover configuration options
    #[clap(flatten, next_help_heading = "Prover")]
    pub prover_config: ProverConfig,
}

impl ProverLock {
    /// Run the lock command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let prover_config = self.prover_config.clone().load_from_files()?;
        let client = prover_config
            .client_builder_with_signer(global_config.tx_timeout)?
            .build()
            .await
            .context("Failed to build Boundless Client with signer")?;

        let network_name = crate::network_name_from_chain_id(client.deployment.chain_id);

        println!(
            "\n{} [{}]",
            "Locking Proof Request".bold(),
            network_name.blue().bold()
        );
        println!("  Request ID: {}", format!("{:#x}", self.request_id).cyan().bold());
        println!("  {} Fetching request details...", "→".dimmed());

        let (request, signature) =
            client.fetch_proof_request(self.request_id, self.tx_hash, self.request_digest).await?;
        tracing::debug!("Fetched order details: {request:?}");

        // If the request is smart contract signed, the preflight of the lock request
        // transaction will revert, since it includes the ERC1271 signature check.
        if !request.is_smart_contract_signed() {
            request.verify_signature(
                &signature,
                client.deployment.boundless_market_address,
                client.boundless_market.get_chain_id().await?,
            )?;
        }

        println!("  {} Locking request...", "→".dimmed());
        client.boundless_market.lock_request(&request, signature, None).await?;

        println!(
            "\n{} Successfully locked request {}",
            "✓".green().bold(),
            format!("{:#x}", self.request_id).green().bold()
        );
        Ok(())
    }
}
