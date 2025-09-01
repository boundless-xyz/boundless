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

use std::path::PathBuf;

use alloy::{
    primitives::Address,
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, ensure, Context};
use boundless_povw_guests::{
    log_updater::{prover::LogUpdaterProver, IPovwAccounting},
    mint_calculator::IPovwMint,
};
use clap::Args;
use risc0_povw::guest::Journal as LogBuilderJournal;
use risc0_zkvm::{default_prover, ProverOpts};

use super::State;
use crate::config::GlobalConfig;

#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct PovwClaimReward {
    /// Path to the work log state file.
    #[arg(short, long)]
    pub state: PathBuf,

    // TODO(povw): Provide a default here, similar to the Deployment struct in boundless-market.
    /// Address of the [IPovwAccounting] contract.
    #[clap(long, env = "POVW_ACCOUNTING_ADDRESS")]
    pub povw_accounting_address: Address,
    /// Address of the [IPovwMint] contract.
    #[clap(long, env = "POVW_MINT_ADDRESS")]
    pub povw_mint_address: Address,
    /// Address of the [IZKC] contract to query.
    #[clap(long, env = "ZKC_ADDRESS")]
    pub zkc_address: Address,
    // TODO: Rename this field to veZKC or something?
    /// Address of the [IZKCRewards] contract to query.
    #[clap(long, env = "ZKC_REWARDS_ADDRESS")]
    pub zkc_rewards_address: Address,
}

impl PovwClaimReward {
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let tx_signer = global_config.require_private_key()?;
        let rpc_url = global_config.require_rpc_url()?;

        // Load the state and check to make sure the private key matches.
        let mut state = State::load(&self.state)
            .with_context(|| format!("Failed to load state from {}", self.state.display()))?;

        // Connect to the chain.
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;

        let chain_id = provider
            .get_chain_id()
            .await
            .with_context(|| format!("Failed to get chain ID from {rpc_url}"))?;

        // Collect the list of block numbers that contain work log update transactions.
        for (tx_hash, update_transaction) in state.update_transactions.iter() {
            // TODO: If the update_transaction has a block_number of None
        }

        todo!()
    }
}
