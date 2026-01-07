// Copyright 2026 Boundless Foundation, Inc.
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

use std::{io::Write, path::PathBuf};

use alloy::{
    primitives::{
        utils::{format_ether, parse_ether},
        Address, U256,
    },
    providers::{Provider, ProviderBuilder},
};
use anyhow::{bail, ensure, Context, Result};
use boundless_povw::{
    deployments::Deployment,
    log_updater::{prover::LogUpdaterProver, IPovwAccounting},
};
use clap::Args;
use colored::Colorize;
use risc0_povw::guest::Journal as LogBuilderJournal;
use risc0_zkvm::{default_prover, ProverOpts};

use crate::{
    commands::rewards::State,
    config::{GlobalConfig, ProvingBackendConfig, RewardsConfig},
    display::{format_amount, DisplayManager},
    indexer_client::{parse_amount, IndexerClient},
};

/// Submit a work log update to the mining accounting contract
///
/// To prepare the update, this command creates a Groth16 proof, compressing the updates to be sent
/// and proving that they are authorized by the signing key for the work log.
#[derive(Args, Clone, Debug)]
pub struct RewardsSubmitMining {
    /// Path to the mining state file (defaults to configured state file from setup)
    #[arg(long = "state-file", env = "MINING_STATE_FILE")]
    state_file: Option<PathBuf>,

    /// The address to assign any mining rewards to (defaults to the reward address from config)
    #[arg(long = "recipient", env = "MINING_RECIPIENT")]
    recipient: Option<Address>,

    /// Deployment configuration for the PoVW and ZKC contracts (defaults to deployment from chain ID)
    #[clap(flatten, next_help_heading = "Deployment")]
    deployment: Option<Deployment>,

    /// Simulate submission and show projected rewards without sending transaction
    #[arg(long)]
    dry_run: bool,

    /// Override reward cap for dry-run (only valid with --dry-run)
    #[arg(long, requires = "dry_run", conflicts_with = "dry_run_staked_zkc")]
    dry_run_reward_cap: Option<String>,

    /// Override staked ZKC amount for dry-run (reward cap = staked / 15) (only valid with --dry-run)
    #[arg(long, requires = "dry_run", conflicts_with = "dry_run_reward_cap")]
    dry_run_staked_zkc: Option<String>,

    /// Override work already submitted for dry-run (only valid with --dry-run)
    #[arg(long, requires = "dry_run")]
    dry_run_work_submitted: Option<String>,

    /// Override total epoch work for dry-run (only valid with --dry-run)
    #[arg(long, requires = "dry_run")]
    dry_run_total_work: Option<String>,

    /// Skip creating a backup of the state file before updating
    #[arg(long)]
    skip_backup: bool,

    #[clap(flatten)]
    rewards_config: RewardsConfig,

    #[clap(flatten, next_help_heading = "Proving Backend")]
    proving_backend: ProvingBackendConfig,
}

impl RewardsSubmitMining {
    /// Run the submit-mining command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let display = DisplayManager::new();
        display.header("Submitting Mining Work Log Update");

        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Determine state file path (param > config > error)
        let state_path = self.state_file.clone()
            .or_else(|| rewards_config.mining_state_file.clone().map(PathBuf::from))
            .context("No mining state file configured.\n\nTo configure: run 'boundless rewards setup' and enable mining\nOr set MINING_STATE_FILE env var")?;

        display.item_colored("State file", state_path.display(), "cyan");

        // Get the transaction signer and work log signer (must be the reward private key)
        let tx_signer = rewards_config.require_reward_private_key()?;
        let work_log_signer = &tx_signer;
        let rpc_url = rewards_config.require_rpc_url()?;

        // Load the state and check to make sure the private key matches
        let mut state = State::load(&state_path)
            .await
            .with_context(|| format!("Failed to load state from {}", state_path.display()))?;

        ensure!(
            Address::from(state.log_id) == work_log_signer.address(),
            "Signer does not match the state log ID: signer: {}, state: {:x}",
            work_log_signer.address(),
            state.log_id,
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

        // Get the current epoch from contract
        let current_epoch = povw_accounting.pendingEpoch().call().await?.number;
        display.item_colored("Work will be submitted during Epoch", current_epoch, "cyan");

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
        let latest_local_commit =
            bytemuck::cast::<_, [u8; 32]>(latest_receipt_journal.updated_commit);

        if latest_local_commit == *onchain_commit {
            display.status("Status", "Already up to date (no submission needed)", "green");
            display.success("On-chain state is already current");
            return Ok(());
        }

        display.status(
            "Status",
            "On-chain work log is not up to date (submission required)",
            "yellow",
        );

        // Find the index of the receipt in the state that has an initial commit equal to the
        // commit current onchain. We will send all updates after that point.
        std::io::stdout().flush()?;

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

        display.item_colored(
            "Updates to submit",
            format!(
                "{} updates to the mining state file need to be submitted on-chain",
                receipts_for_update.len()
            ),
            "cyan",
        );

        if receipts_for_update.len() > 1 {
            display.note("Note: Multiple mining state file updates will be submitted. This happens when prepare-mining was run multiple times before submission. For future submissions, consider running prepare-mining only once before submitting to save gas costs");
        }

        // Execute dry-run if requested
        if self.dry_run {
            // Calculate total work from all receipts to be submitted
            let total_new_work = receipts_for_update
                .iter()
                .map(|receipt| {
                    let journal = LogBuilderJournal::decode(&receipt.journal.bytes)?;
                    Ok(journal.update_value)
                })
                .collect::<Result<Vec<_>>>()?
                .iter()
                .sum::<u64>();

            return self
                .execute_dry_run(
                    &rewards_config,
                    &provider,
                    &deployment,
                    &state,
                    total_new_work,
                    chain_id,
                )
                .await;
        }

        // Determine recipient (param > reward address > error)
        let recipient = self.recipient.or(rewards_config.reward_address);

        if let Some(recipient_addr) = recipient {
            display.address("Recipient", recipient_addr);
        }

        display.subsection("Configuring prover for submitting mining work");

        self.proving_backend.configure_proving_backend_with_health_check().await?;
        display.status("Status", "Ready", "green");

        let total_receipts = receipts_for_update.len();
        for (idx, receipt) in receipts_for_update.into_iter().enumerate() {
            let receipt_num = idx + 1;

            display.subsection(&format!(
                "Processing mining state file update {}/{}",
                receipt_num, total_receipts
            ));

            let prover = LogUpdaterProver::builder()
                .prover(default_prover())
                .chain_id(chain_id)
                .value_recipient(recipient)
                .contract_address(deployment.povw_accounting_address)
                .prover_opts(ProverOpts::groth16())
                .build()
                .context("Failed to build prover for Log Updater")?;

            // Sign and prove the authorized work log update
            display.step(1, 3, "Generating proof...");
            display.note("  (This may take several minutes)");

            let prove_info = prover
                .prove_update(receipt, work_log_signer)
                .await
                .context("Failed to prove authorized log update")?;

            display.status("Status", "Proof complete", "green");

            display.step(2, 3, "Submitting transaction...");
            let tx_result = povw_accounting
                .update_work_log(&prove_info.receipt)
                .context("Failed to construct update transaction")?
                .send()
                .await
                .context("Failed to send update transaction")?;
            let tx_hash = tx_result.tx_hash();

            display.tx_hash(*tx_hash);

            // Save the pending transaction to state
            state
                .add_pending_update_tx(*tx_hash)?
                .save(&state_path)
                .context("Failed to save state")?;

            display.step(3, 3, "Waiting for confirmation...");
            let timeout = global_config.tx_timeout.or(tx_result.timeout());
            let tx_receipt = tx_result
                .with_timeout(timeout)
                .get_receipt()
                .await
                .context("Failed to receive receipt for update transaction")?;

            ensure!(
                tx_receipt.status(),
                "Submit mining state file update transaction failed: tx_hash = {}",
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
                display.status("Status", "Confirmed", "green");
                display.item_colored("Epoch", data.epochNumber, "cyan");
                display.item_colored("Work Value", data.updateValue.to::<u64>(), "cyan");
            } else {
                display.status("Status", "Confirmed", "green");
            }

            // Confirm the transaction in the state
            state
                .confirm_update_tx(&tx_receipt)
                .context("Failed to add transaction receipt to state")?
                .save(&state_path)
                .context("Failed to save state")?;

            // Backup the updated state after each receipt is processed
            if !self.skip_backup {
                let backup_path = state.save_backup(&state_path)?;
                display.note(&format!("Backup saved: {}", backup_path.display()));
            }
        }

        display.separator();
        display.success(&format!("Successfully submitted {} mining update(s)", total_receipts));
        display.separator();

        Ok(())
    }

    /// Execute dry-run to show projected rewards
    async fn execute_dry_run(
        &self,
        _rewards_config: &RewardsConfig,
        provider: &impl Provider,
        deployment: &Deployment,
        state: &State,
        new_work_value: u64,
        chain_id: u64,
    ) -> Result<()> {
        // Get current epoch from contract
        let povw_accounting = IPovwAccounting::new(deployment.povw_accounting_address, provider);
        let current_epoch = povw_accounting.pendingEpoch().call().await?.number;

        // Determine reward address (log_id → address)
        let reward_address = Address::from(state.log_id);

        // Get indexer client (use chain_id to determine indexer)
        let indexer = IndexerClient::new_from_chain_id(chain_id)?;

        // Fetch PoVW metadata for last updated timestamp
        let metadata = indexer.get_povw_metadata().await.ok();

        // Query indexer APIs (may fail if overrides are provided)
        let epoch_data = indexer.get_epoch_povw(current_epoch.to::<u64>()).await.ok();
        let address_data = indexer
            .get_epoch_povw_for_address(current_epoch.to::<u64>(), reward_address)
            .await
            .ok();

        let display = DisplayManager::new();
        display.header("Dry Run: Mining Submission Projection");

        // Parse values with overrides
        let current_work_submitted = if let Some(ref override_val) = self.dry_run_work_submitted {
            parse_amount(override_val).context("Failed to parse --dry-run-work-submitted")?
        } else if let Some(ref data) = address_data {
            parse_amount(&data.work_submitted)?
        } else {
            display.warning("Did not find any work already submitted for this epoch, and --dry-run-work-submitted not set. Defaulting to 0.");
            U256::ZERO
        };

        let total_work = if let Some(ref override_val) = self.dry_run_total_work {
            parse_amount(override_val).context("Failed to parse --dry-run-total-work")?
        } else if let Some(ref data) = epoch_data {
            parse_amount(&data.total_work)?
        } else {
            bail!("No epoch work data available and no override provided. Use --dry-run-total-work")
        };

        let total_emissions = if let Some(ref data) = epoch_data {
            parse_amount(&data.total_emissions)?
        } else {
            bail!("No epoch emissions data available from indexer")
        };

        let reward_cap = if let Some(ref override_val) = self.dry_run_reward_cap {
            parse_ether(override_val).context("Failed to parse --dry-run-reward-cap")?
        } else if let Some(ref staked_zkc) = self.dry_run_staked_zkc {
            let staked = parse_ether(staked_zkc).context("Failed to parse --dry-run-staked-zkc")?;
            staked / U256::from(15)
        } else if let Some(ref data) = address_data {
            parse_amount(&data.reward_cap)?
        } else {
            bail!("No reward cap data available and no override provided. Use --dry-run-reward-cap or --dry-run-staked-zkc")
        };

        // Calculate projected work
        let projected_total_work = current_work_submitted + U256::from(new_work_value);
        let new_total_work = total_work + U256::from(new_work_value);

        // Calculate uncapped projected rewards
        let uncapped_rewards = if !new_total_work.is_zero() {
            (projected_total_work * total_emissions) / new_total_work
        } else {
            U256::ZERO
        };

        // Apply reward cap
        let capped_rewards = std::cmp::min(uncapped_rewards, reward_cap);

        // Display results (work is in cycles, ZKC amounts are in wei)
        let current_work_formatted = format_work_cycles(&current_work_submitted);
        let total_work_formatted = format_work_cycles(&total_work);
        let new_work_formatted = format_work_cycles(&U256::from(new_work_value));
        let projected_total_formatted = format_work_cycles(&projected_total_work);
        let new_total_work_formatted = format_work_cycles(&new_total_work);

        let total_emissions_formatted = format_amount(&format_ether(total_emissions));
        let uncapped_rewards_formatted = format_amount(&format_ether(uncapped_rewards));
        let reward_cap_formatted = format_amount(&format_ether(reward_cap));
        let capped_rewards_formatted = format_amount(&format_ether(capped_rewards));

        if let Some(ref meta) = metadata {
            let formatted_time = crate::indexer_client::format_timestamp(&meta.last_updated_at);
            display.note(&format!("Data last updated: {}", formatted_time));
        }

        display.note(&format!(
            "Dry-run for submitting work during Epoch {}",
            current_epoch.to_string().cyan().bold()
        ));
        display.address("Reward Address", reward_address);
        display.balance("Total Epoch Emissions", &total_emissions_formatted, "ZKC", "cyan");

        display.subsection("Before Work Submission State");
        display.subitem("Your total work submitted:", &current_work_formatted.cyan().to_string());
        display.subitem(
            "Total work submitted by all participants:",
            &total_work_formatted.cyan().to_string(),
        );

        display.subsection("Projected After Submission");
        display.subitem("Your new work being submitted:", &new_work_formatted.yellow().to_string());
        display
            .subitem("Your total work submitted:", &projected_total_formatted.cyan().to_string());
        display.subitem(
            "Total work submitted by all participants:",
            &new_total_work_formatted.cyan().to_string(),
        );

        display.subsection("Reward Calculations");
        let percentage = if !new_total_work.is_zero() {
            (projected_total_work * U256::from(10000u64)) / new_total_work
        } else {
            U256::ZERO
        };
        let percentage_float = percentage.to::<u64>() as f64 / 100.0;
        display.subitem(
            "Your Share:",
            &format!(
                "{:.2}% {}",
                percentage_float,
                format!("({} / {})", projected_total_formatted, new_total_work_formatted).dimmed()
            ),
        );
        display.subitem(
            "Potential Rewards:",
            &format!("{} {}", uncapped_rewards_formatted.yellow(), "ZKC".yellow()),
        );
        display.subitem(
            "Reward Cap:",
            &format!("{} {}", reward_cap_formatted.yellow(), "ZKC".yellow()),
        );
        display.subitem(
            "Actual Rewards:",
            &format!(
                "{} {} {}",
                capped_rewards_formatted.green().bold(),
                "ZKC".green(),
                "(min(reward cap, potential rewards))".to_string().dimmed()
            ),
        );

        if uncapped_rewards > reward_cap {
            display.warning("Your rewards are CAPPED");
            let lost = uncapped_rewards - reward_cap;
            let lost_formatted = format_amount(&format_ether(lost));
            display
                .subitem("", &format!("You would lose {} ZKC due to reward cap", lost_formatted));
        }

        // Display override information if any were used
        if self.dry_run_reward_cap.is_some()
            || self.dry_run_staked_zkc.is_some()
            || self.dry_run_work_submitted.is_some()
            || self.dry_run_total_work.is_some()
        {
            display.subsection(&"Override Parameters Used".yellow().to_string());
            if let Some(ref cap) = self.dry_run_reward_cap {
                display.subitem("Reward Cap:", &format!("{} ZKC (override)", cap));
            }
            if let Some(ref staked) = self.dry_run_staked_zkc {
                display.subitem(
                    "Staked ZKC:",
                    &format!("{} ZKC (override, reward cap = staked / 15)", staked),
                );
            }
            if let Some(ref work) = self.dry_run_work_submitted {
                display.subitem("Work Submitted:", &format!("{} (override)", work));
            }
            if let Some(ref total) = self.dry_run_total_work {
                display.subitem("Total Epoch Work:", &format!("{} (override)", total));
            }
        }

        display.note("Note: This is a projection based on current epoch data.");
        display.note("Actual rewards may vary due to other participants submitting more work.");

        Ok(())
    }
}

/// Helper to format work cycles with commas and " cycles" suffix
fn format_work_cycles(amount: &U256) -> String {
    let value = amount.to::<u128>();
    let formatted = format_number_with_commas(value);
    format!("{} cycles", formatted)
}

/// Helper to format numbers with comma separators
fn format_number_with_commas(n: u128) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let mut count = 0;

    for c in s.chars().rev() {
        if count == 3 {
            result.push(',');
            count = 0;
        }
        result.push(c);
        count += 1;
    }

    result.chars().rev().collect()
}
