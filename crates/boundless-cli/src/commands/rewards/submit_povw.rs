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
};
use anyhow::{bail, ensure, Context, Result};
use boundless_povw::{
    deployments::Deployment,
    log_updater::{prover::LogUpdaterProver, IPovwAccounting},
};
use clap::Args;
use risc0_povw::guest::Journal as LogBuilderJournal;
use risc0_zkvm::{default_prover, ProverOpts};

use crate::commands::povw::State;
use crate::config::{GlobalConfig, ProverConfig, RewardsConfig};

/// Submit a work log update to the PoVW accounting contract
///
/// To prepare the update, this command creates a Groth16 proof, compressing the updates to be sent
/// and proving that they are authorized by the signing key for the work log.
#[derive(Args, Clone, Debug)]
pub struct RewardsSubmitPovw {
    /// Path to the PoVW state file (defaults to configured state file from setup)
    #[arg(long = "state-file", env = "POVW_STATE_FILE")]
    state_file: Option<PathBuf>,

    /// The address to assign any PoVW rewards to (defaults to the reward address from config)
    #[arg(long = "recipient", env = "POVW_RECIPIENT")]
    recipient: Option<Address>,

    /// Deployment configuration for the PoVW and ZKC contracts (defaults to deployment from rewards config)
    #[clap(flatten, next_help_heading = "Deployment")]
    deployment: Option<Deployment>,

    #[clap(flatten)]
    rewards_config: RewardsConfig,

    #[clap(flatten, next_help_heading = "Prover")]
    prover_config: ProverConfig,
}

impl RewardsSubmitPovw {
    /// Run the submit-povw command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Determine state file path (param > config > error)
        let state_path = self.state_file.clone()
            .or_else(|| rewards_config.povw_state_file.clone().map(PathBuf::from))
            .context("No PoVW state file configured.\n\nTo configure: run 'boundless setup rewards' and enable PoVW\nOr set POVW_STATE_FILE env var")?;

        // Get the transaction signer and work log signer (must be the reward private key)
        let tx_signer = rewards_config.require_reward_private_key()?;
        let work_log_signer = &tx_signer;
        let rpc_url = rewards_config.require_rpc_url()?;

        // Load the state and check to make sure the private key matches
        let mut state = State::load(&state_path)
            .await
            .with_context(|| format!("Failed to load state from {}", state_path.display()))?;
        tracing::info!("Submitting work log update for log ID: {:x}", state.log_id);

        ensure!(
            Address::from(state.log_id) == work_log_signer.address(),
            "Signer does not match the state log ID: signer: {}, state: {}",
            work_log_signer.address(),
            state.log_id
        );

        // Connect to the chain
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("Failed to connect provider to {rpc_url}"))?;

        let chain_id = provider
            .get_chain_id()
            .await
            .with_context(|| format!("Failed to get chain ID from {rpc_url}"))?;

        let deployment = self
            .deployment
            .clone()
            .or_else(|| Deployment::from_chain_id(chain_id))
            .context(
                "Could not determine deployment from chain ID; please specify deployment explicitly",
            )?;

        let povw_accounting =
            IPovwAccounting::new(deployment.povw_accounting_address, provider.clone());

        // Get the current work log commit to determine which update(s) should be applied
        let onchain_commit =
            povw_accounting.workLogCommit(state.log_id.into()).call().await.with_context(|| {
                format!(
                    "Failed to get work log commit for {:x} from {:x}",
                    state.log_id, deployment.povw_accounting_address
                )
            })?;

        // Check if the latest log builder receipt has an updated_commit value equal to what is
        // onchain. If so, the onchain work log is already up to date.
        let Some(latest_receipt) = state.log_builder_receipts.last() else {
            bail!("Loaded state has no log builder receipts")
        };
        let latest_receipt_journal = LogBuilderJournal::decode(&latest_receipt.journal.bytes)
            .context("Failed to decode journal from latest receipt")?;
        if bytemuck::cast::<_, [u8; 32]>(latest_receipt_journal.updated_commit) == *onchain_commit {
            tracing::info!("Onchain PoVW accounting contract is already up to date with the latest commit in state");
            return Ok(());
        }

        // Find the index of the receipt in the state that has an initial commit equal to the
        // commit current onchain. We will send all updates after that point.
        let matching_receipt_index = state
            .log_builder_receipts
            .iter()
            .enumerate()
            .rev()
            .map(|(i, receipt)| {
                let journal =
                    LogBuilderJournal::decode(&receipt.journal.bytes).with_context(|| {
                        format!("Failed to decode journal from receipt in state at index {i}")
                    })?;
                anyhow::Ok(
                    (bytemuck::cast::<_, [u8; 32]>(journal.initial_commit) == *onchain_commit)
                        .then_some(i),
                )
            })
            .find_map(|x| x.transpose())
            .with_context(|| {
                format!("Failed to find receipt with initial commit matching {onchain_commit}")
            })??;

        // Iterate over all the log builder receipts that should be sent to the chain.
        // NOTE: In most cases, this will be one receipt. It may be more if the prover previously
        // built a work log update but it failed to send (e.g. network instability or high gas
        // fees caused the transaction not to go through).
        let receipts_for_update = state.log_builder_receipts[matching_receipt_index..].to_vec();
        if receipts_for_update.len() > 1 {
            tracing::info!(
                "Updating onchain work log {:x} with {} update receipts",
                state.log_id,
                receipts_for_update.len()
            )
        }

        // Determine recipient (param > reward address > error)
        let recipient = self.recipient.or(rewards_config.reward_address);

        self.prover_config.configure_proving_backend_with_health_check().await?;
        for receipt in receipts_for_update {
            let prover = LogUpdaterProver::builder()
                .prover(default_prover())
                .chain_id(chain_id)
                .value_recipient(recipient)
                .contract_address(deployment.povw_accounting_address)
                .prover_opts(ProverOpts::groth16())
                .build()
                .context("Failed to build prover for Log Updater")?;

            // Sign and prove the authorized work log update
            tracing::info!("Proving work log update");
            let prove_info = prover
                .prove_update(receipt, work_log_signer)
                .await
                .context("Failed to prove authorized log update")?;

            tracing::info!("Sending work log update transaction");
            let tx_result = povw_accounting
                .update_work_log(&prove_info.receipt)
                .context("Failed to construct update transaction")?
                .send()
                .await
                .context("Failed to send update transaction")?;
            let tx_hash = tx_result.tx_hash();
            tracing::info!(%tx_hash, "Sent transaction for work log update");

            // Save the pending transaction to state
            state
                .add_pending_update_tx(*tx_hash)?
                .save(&state_path)
                .context("Failed to save state")?;

            let timeout = global_config.tx_timeout.or(tx_result.timeout());
            tracing::debug!(?timeout, %tx_hash, "Waiting for transaction receipt");
            let tx_receipt = tx_result
                .with_timeout(timeout)
                .get_receipt()
                .await
                .context("Failed to receive receipt for update transaction")?;

            ensure!(
                tx_receipt.status(),
                "Work log update transaction failed: tx_hash = {}",
                tx_receipt.transaction_hash
            );

            // Extract the WorkLogUpdated event
            let work_log_updated_event = tx_receipt
                .logs()
                .iter()
                .filter_map(|log| log.log_decode::<IPovwAccounting::WorkLogUpdated>().ok())
                .next();

            if let Some(event) = work_log_updated_event {
                let data = event.inner.data;
                tracing::info!(
                    "Work log update confirmed in epoch {} with work value {}",
                    data.epochNumber,
                    data.updateValue.to::<u64>()
                );
                tracing::debug!(updated_commit = %data.updatedCommit, "Updated work log commitment")
            }

            // Confirm the transaction in the state
            state
                .confirm_update_tx(&tx_receipt)
                .context("Failed to add transaction receipt to state")?
                .save(&state_path)
                .context("Failed to save state")?;
        }

        Ok(())
    }
}
