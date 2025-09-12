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
use boundless_povw::{
    deployments::Deployment,
    log_updater::{prover::LogUpdaterProver, IPovwAccounting},
};
use clap::Args;
use risc0_povw::guest::Journal as LogBuilderJournal;
use risc0_zkvm::{default_prover, ProverOpts};

use super::State;
use crate::config::{GlobalConfig, ProverConfig};

/// Submit a work log update to the PoVW accounting contract.
///
/// To prepare the update, this command creates a Groth16 proof, compressing the updates to be sent
/// and proving that they are authorized by the signing key for the work log.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct PovwSubmit {
    /// State of the work log, including proven updates produces by the prepare command.
    #[arg(short, long, env = "POVW_STATE_PATH")]
    pub state: PathBuf,

    /// Private key used to sign work log updates. Should have an address equal to the work log ID.
    ///
    /// If this option is not set, the value of the private key from global config will be used.
    #[clap(long, env = "POVW_PRIVATE_KEY", hide_env_values = true)]
    pub povw_private_key: Option<PrivateKeySigner>,

    /// The address to assign any PoVW rewards to. If not provided, defaults to the work log ID.
    #[clap(short, long, env = "POVW_VALUE_RECIPIENT")]
    pub value_recipient: Option<Address>,

    /// Deployment configuration for the PoVW and ZKC contracts.
    #[clap(flatten, next_help_heading = "Deployment")]
    pub deployment: Option<Deployment>,

    #[clap(flatten, next_help_heading = "Prover")]
    prover_config: ProverConfig,
}

impl PovwSubmit {
    /// Run the [PovwSubmit] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let tx_signer = global_config.require_private_key()?;
        let work_log_signer = self.povw_private_key.as_ref().unwrap_or(&tx_signer);
        let rpc_url = global_config.require_rpc_url()?;

        // Load the state and check to make sure the private key matches.
        let mut state = State::load(&self.state)
            .await
            .with_context(|| format!("Failed to load state from {}", self.state.display()))?;
        tracing::info!("Submitting work log update for log ID: {:x}", state.log_id);

        ensure!(
            Address::from(state.log_id) == work_log_signer.address(),
            "Signer does not match the state log ID: signer: {}, state: {}",
            work_log_signer.address(),
            state.log_id
        );

        // Connect to the chain.
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
            "could not determine deployment from chain ID; please specify deployment explicitly",
        )?;
        let povw_accounting =
            IPovwAccounting::new(deployment.povw_accounting_address, provider.clone());

        // Get the current work log commit, to determine which update(s) should be applied.
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

        self.prover_config.configure_proving_backend_with_health_check().await?;
        for receipt in receipts_for_update {
            let prover = LogUpdaterProver::builder()
                .prover(default_prover())
                .chain_id(chain_id)
                .value_recipient(self.value_recipient)
                .contract_address(deployment.povw_accounting_address)
                .prover_opts(ProverOpts::groth16())
                .build()
                .context("Failed to build prover for Log Updater")?;

            // Sign and prove the authorized work log update.
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

            // Save the pending transaction to state.
            state
                .add_pending_update_tx(*tx_hash)?
                .save(&self.state)
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

            // Confirm the transaction in the state.
            state
                .confirm_update_tx(&tx_receipt)
                .context("Failed to add transaction receipt to state")?
                .save(&self.state)
                .context("Failed to save state")?;
        }

        // TODO: Display to the user the current epoch and when it will end (e.g. in "2h 25m (2025-09-04 16:23:45 PDT)")

        Ok(())
    }
}
