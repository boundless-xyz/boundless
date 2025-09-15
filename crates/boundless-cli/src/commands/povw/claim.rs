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

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use alloy::{
    contract::Event,
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::{Filter, Log},
    sol_types::SolEvent,
};
use anyhow::{bail, ensure, Context};
use boundless_povw::{
    deployments::Deployment,
    log_updater::IPovwAccounting::{self, EpochFinalized, IPovwAccountingInstance, WorkLogUpdated},
    mint_calculator::{prover::MintCalculatorProver, IPovwMint, CHAIN_SPECS},
};
use clap::Args;
use risc0_povw::PovwLogId;
use risc0_zkvm::{default_prover, Digest, ProverOpts};
use url::Url;

use crate::config::{GlobalConfig, ProverConfig};

const HOUR: Duration = Duration::from_secs(60 * 60);

// TODO: Figure out what rewards the user is eligible for and warn them if they are receiving less
// than their cycles could get them.

/// Command to claim PoVW rewards associated with submitted work log updates.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct PovwClaim {
    // TODO(povw): Support providing multiple log IDs.
    /// Work log ID for the reward claim.
    ///
    /// State for submitted updates is retrieved from the chain using the ID. Note that initiating
    /// the claim can be done for any log ID and does not require authorization.
    /// Work log ID for the reward claim.
    ///
    /// State for submitted updates is retrieved from the chain using the ID. Note that initiating
    /// the claim can be done for any log ID and does not require authorization.
    #[arg(short, long)]
    pub log_id: PovwLogId,

    // TODO: Deprecate and/or remove this when history support works without the Beacon API.
    /// URL for an Ethereum Beacon chain (i.e. consensus chain) API.
    ///
    /// Providing a Beacon API is required when claiming rewards on Ethereum in order to get the
    /// chain data required to prove your allocated rewards. A provider such as Quicknode can
    /// supply a Beacon API.
    #[arg(long, env)]
    pub beacon_api_url: Option<Url>,

    /// Deployment configuration for the PoVW and ZKC contracts.
    #[clap(flatten, next_help_heading = "Deployment")]
    pub deployment: Option<Deployment>,

    /// Maximum number of days to consider for the reward claim.
    ///
    /// This effects how far back in history this command will search for log update events for the
    /// rewards claim. If all log update events to claim occured in fewer than the specified number
    /// of days, this command will not scan for events in the full range.
    #[clap(long, default_value_t = 30)]
    pub days: u32,
    /// Chunk size to use when querying the RPC node for events using `eth_getLogs`.
    ///
    /// If using a free-tier RPC provider, you may need to set this to a lower value. You may also
    /// try raising this value to improve search time.
    #[clap(long, default_value_t = 10000)]
    pub event_query_chunk_size: u64,

    #[clap(flatten, next_help_heading = "Prover")]
    prover_config: ProverConfig,
}

impl PovwClaim {
    /// Run the [PovwClaim] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        if self.beacon_api_url.is_none() {
            tracing::warn!("No Beacon API URL provided; claiming rewards may fail the multi-block continuity check.");
            tracing::warn!("You can provide it using the --beacon-api-url flag.");
        }
        let tx_signer = global_config.require_private_key()?;
        let rpc_url = global_config.require_rpc_url()?;

        // Connect to the chain.
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;

        let chain_id = provider.get_chain_id().await.context("Failed to query the chain ID")?;
        let chain_spec = CHAIN_SPECS.get(&chain_id).with_context(|| {
            format!("No known Steel chain specification for chain ID {chain_id}")
        })?;
        let deployment = self
            .deployment
            .clone()
            .or_else(|| Deployment::from_chain_id(chain_id))
            .context(
            "could not determine deployment from chain ID; please specify deployment explicitly",
        )?;

        // Determine the limits on the blocks that will be searched for events.
        let latest_block_number =
            provider.get_block_number().await.context("Failed to query the block number")?;
        let search_limit_time = SystemTime::now()
            .checked_sub(self.days * 24 * HOUR)
            .context("Invalid number of days")?;
        let lower_limit_block_number = block_number_near_timestamp(
            &provider,
            latest_block_number,
            search_limit_time,
            Some(HOUR),
        )
        .await
        .context("Failed to determine the block number for the event search limit")?;
        tracing::debug!("Event search will use a lower limit of block {lower_limit_block_number}");

        let povw_accounting =
            IPovwAccounting::new(deployment.povw_accounting_address, provider.clone());
        let povw_mint = IPovwMint::new(deployment.povw_mint_address, provider.clone());

        // Determine the commit range for which we can mint. This is the difference between the
        // recoreded work log commit on the accounting contract and on the mint contract.
        let initial_commit = Digest::from(
            *povw_mint.workLogCommit(self.log_id.into()).call().await.with_context(|| {
                format!(
                    "Failed to call IPovwMint.workLogCommit on {}",
                    deployment.povw_mint_address
                )
            })?,
        );
        let final_commit = Digest::from(
            *povw_accounting.workLogCommit(self.log_id.into()).call().await.with_context(|| {
                format!(
                    "Failed to call IPovwAccounting.workLogCommit on {}",
                    deployment.povw_accounting_address
                )
            })?,
        );
        tracing::debug!(%initial_commit, %final_commit, "Commit range for mint");

        if initial_commit == final_commit {
            tracing::info!("All rewards for submitted work log updates have been claimed");
            return Ok(());
        }

        // Search for the WorkLogUpdated events, and the the EpochFinalized events.
        tracing::info!("Searching for work log update events in the past {} days", self.days);
        let update_events = search_work_log_updated(
            &povw_accounting,
            self.log_id,
            initial_commit,
            final_commit,
            latest_block_number,
            lower_limit_block_number,
            self.event_query_chunk_size,
        )
        .await
        .context("Search for work log update events failed")?;
        tracing::info!("Found {} work log update events", update_events.len());

        // Check to see what the current pending epoch is on the PoVW accounting contract. Filter
        // out update events with an epoch that has not finalized (with a warning).
        let pending_epoch = povw_accounting
            .pendingEpoch()
            .call()
            .await
            .context("Failed to check the pending epoch")?
            .number;
        let finalized_update_events = update_events
            .into_iter()
            .filter(|(event, _)| {
                if event.epochNumber >= pending_epoch {
                    tracing::warn!(
                        "Skipping update in epoch {}, which has not been finalized",
                        event.epochNumber
                    );
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();

        // NOTE: At least one epoch must be skipped to reach this error.
        if finalized_update_events.is_empty() {
            bail!("No update events found for finalized epochs; no rewards to claim")
        }

        // We can refine the range we search for EpochFinalized events using the first event.
        let lower_limit_block_number = finalized_update_events
            .first()
            .map(|(_, block_numer)| *block_numer)
            .unwrap_or(lower_limit_block_number);
        let epochs = finalized_update_events
            .iter()
            .map(|(event, _)| event.epochNumber)
            .collect::<BTreeSet<_>>();

        ensure!(!epochs.is_empty(), "List of epochs for claim is empty");
        let first_epoch = epochs.iter().next().unwrap();
        let last_epoch = epochs.iter().last().unwrap();
        if first_epoch == last_epoch {
            tracing::info!("Searching for epoch finalization event for epoch {first_epoch}");
        } else {
            tracing::info!("Searching for epoch finalization events, from epoch {first_epoch} to epoch {last_epoch}");
        }
        let epoch_events = search_epoch_finalized(
            &povw_accounting,
            epochs,
            latest_block_number,
            lower_limit_block_number,
            self.event_query_chunk_size,
        )
        .await
        .context("Search for epoch finalized events failed")?;
        tracing::info!("Found {} epoch finalization events", epoch_events.len());

        let event_block_numbers = BTreeSet::from_iter(
            finalized_update_events
                .iter()
                .map(|(_, block_number)| *block_number)
                .chain(epoch_events.keys().copied()),
        );

        self.prover_config.configure_proving_backend_with_health_check().await?;
        let mint_calculator_prover = MintCalculatorProver::builder()
            .prover(default_prover())
            .provider(provider.clone())
            .beacon_api(self.beacon_api_url.clone())
            .povw_accounting_address(deployment.povw_accounting_address)
            .zkc_address(deployment.zkc_address)
            .zkc_rewards_address(deployment.vezkc_address)
            .chain_spec(chain_spec)
            .prover_opts(ProverOpts::groth16())
            .build()?;

        tracing::info!("Building input data for Mint Calculator guest");
        let mint_input = mint_calculator_prover
            .build_input(event_block_numbers, [self.log_id])
            .await
            .context("Failed to build input for Mint Calculator Guest")?;

        tracing::info!("Proving Mint Calculator guest");
        let mint_prove_info = mint_calculator_prover
            .prove_mint(&mint_input)
            .await
            .context("Failed to prove Mint Calculator guest")?;

        tracing::info!("Sending reward claim transaction");
        let tx_result = povw_mint
            .mint_with_receipt(&mint_prove_info.receipt)
            .context("Failed to construct reward claim transaction")?
            .send()
            .await
            .context("Failed to send reward claim transaction")?;
        let tx_hash = tx_result.tx_hash();
        tracing::info!(%tx_hash, "Sent transaction for reward claim");

        let timeout = global_config.tx_timeout.or(tx_result.timeout());
        tracing::debug!(?timeout, %tx_hash, "Waiting for transaction receipt");
        let tx_receipt = tx_result
            .with_timeout(timeout)
            .get_receipt()
            .await
            .context("Failed to receive receipt reward claim transaction")?;

        ensure!(
            tx_receipt.status(),
            "Reward claim transaction failed: tx_hash = {}",
            tx_receipt.transaction_hash
        );

        // TODO(povw): Display some info, like how much of a reward was created.
        tracing::info!("Reward claim completed");
        Ok(())
    }
}

async fn block_number_near_timestamp(
    provider: impl Provider,
    latest_block_number: u64,
    timestamp: SystemTime,
    approx: Option<Duration>,
) -> anyhow::Result<u64> {
    tracing::debug!("Search for block with timestamp less than {timestamp:?}");

    // Phase 1: Linear search backwards in chunks until we find a block <= target_timestamp
    const LINEAR_SEARCH_CHUNK_SIZE: u64 = 100000;
    let mut probe = latest_block_number;
    loop {
        let block = provider
            .get_block_by_number(probe.into())
            .await
            .context("Failed to get block {probe}")?
            .context("Block {probe} not found")?;

        let block_timestamp = UNIX_EPOCH + Duration::from_secs(block.header.timestamp);
        tracing::debug!("Linear search at block {probe}, timestamp {block_timestamp:?}");
        if block_timestamp <= timestamp {
            break;
        }

        probe = probe.saturating_sub(LINEAR_SEARCH_CHUNK_SIZE);
        if probe == 0 {
            // We've reached the block 0. This the closest possible block to the timestamp.
            return Ok(0);
        }
    }

    // Phase 2: binary search between [low, high]
    // NOTE: If the latest block is less than the target timestamp, the binary search will not run.
    let mut high = u64::min(probe + LINEAR_SEARCH_CHUNK_SIZE, latest_block_number);
    let mut low = probe;
    while low < high {
        let mid = (low + high).div_ceil(2);
        let block = provider
            .get_block_by_number(mid.into())
            .await
            .context("Failed to get block {mid}")?
            .context("Block {mid} not found")?;

        let block_timestamp = UNIX_EPOCH + Duration::from_secs(block.header.timestamp);
        tracing::debug!("Binary search at block {mid}, timestamp {block_timestamp:?}");
        if block_timestamp <= timestamp {
            low = mid; // candidate, move up

            // If an approximation factor is provided, see if we are close enough.
            if let Some(approx) = approx {
                if block_timestamp >= timestamp.checked_sub(approx).unwrap_or(UNIX_EPOCH) {
                    break;
                }
            }
        } else {
            high = mid - 1;
        }
    }

    Ok(low)
}

/// Search for work log updated events required for the mint operation.
///
/// This function pregressively searches backwards in chunks, start at the upoer limit block, until
/// it finds all the events needed or hits the lower limit block. It returns the sorted list of
/// found [WorkLogUpdated] events along with the block number at which they were emitted.
async fn search_work_log_updated(
    povw_accounting: &IPovwAccountingInstance<impl Provider>,
    log_id: PovwLogId,
    initial_commit: Digest,
    final_commit: Digest,
    upper_limit_block_number: u64,
    lower_limit_block_number: u64,
    chunk_size: u64,
) -> anyhow::Result<Vec<(WorkLogUpdated, u64)>> {
    let mut events = HashMap::<Digest, (WorkLogUpdated, u64)>::new();
    let search_predicate = |query_logs: &[(WorkLogUpdated, Log)]| {
        for (event, log) in query_logs {
            let commit = Digest::from(*event.initialCommit);
            let block_number =
                log.block_number.context("Log from range does not have block number")?;
            events.insert(commit, (event.clone(), block_number));
            tracing::debug!(block_number, ?event, "Found WorkLogUpdated event");
        }
        let halt = events.contains_key(&initial_commit);
        if halt {
            tracing::debug!(%initial_commit, "Reached initial commit");
        }
        Ok(!halt)
    };

    // Set up the event filter for the specified block range
    let filter = Filter::new()
        .address(*povw_accounting.address())
        .event_signature(WorkLogUpdated::SIGNATURE_HASH)
        .topic1(Address::from(log_id));

    tracing::debug!(%initial_commit, %final_commit, %upper_limit_block_number, %lower_limit_block_number, "Searching for WorkLogUpdated events");
    search_events(
        povw_accounting.provider(),
        filter,
        lower_limit_block_number,
        upper_limit_block_number,
        chunk_size,
        search_predicate,
    )
    .await
    .context("Failed to search for WorkLogUpdated events")?;

    // Reconstruct the chain of WorkLogUpdated from initial_commit to final_commit.
    let mut commit = initial_commit;
    let mut sorted_events = Vec::new();
    while commit != final_commit {
        match events.remove(&commit) {
            Some((event, block_number)) => {
                sorted_events.push((event.clone(), block_number));
                commit = Digest::from(*event.updatedCommit);
            }
            None => bail!("Missing WorkLogUpdated event in chain with initial commit {commit}; did not reach final commit {final_commit}")
        }
    }

    Ok(sorted_events)
}

async fn search_epoch_finalized(
    povw_accounting: &IPovwAccountingInstance<impl Provider>,
    mut epochs: BTreeSet<U256>,
    upper_limit_block_number: u64,
    lower_limit_block_number: u64,
    chunk_size: u64,
) -> anyhow::Result<BTreeMap<u64, EpochFinalized>> {
    tracing::debug!(?epochs, "Searching for EpochFinalized events");

    let mut events = BTreeMap::<u64, EpochFinalized>::new();
    let search_predicate = |query_logs: &[(EpochFinalized, Log)]| {
        for (event, log) in query_logs {
            // Remove the epoch from the set we are searching for.
            if epochs.remove(&event.epoch) {
                let block_number =
                    log.block_number.context("Log from range does not have block number")?;
                events.insert(block_number, event.clone());
                tracing::debug!(block_number, ?event, "Found EpochFinalized event");
            }
        }
        let halt = epochs.is_empty();
        if halt {
            tracing::debug!("Found all epoch finalized events");
        }
        Ok(!halt)
    };

    // Set up the event filter for the specified block range
    let filter = Filter::new()
        .address(*povw_accounting.address())
        .event_signature(EpochFinalized::SIGNATURE_HASH);

    search_events(
        povw_accounting.provider(),
        filter,
        lower_limit_block_number,
        upper_limit_block_number,
        chunk_size,
        search_predicate,
    )
    .await
    .context("Failed to search for EpochFinalized events")?;

    Ok(events)
}

async fn search_events<P: Provider + Clone, E: SolEvent>(
    provider: P,
    filter: Filter,
    lower_limit_block_number: u64,
    upper_limit_block_number: u64,
    chunk_size: u64,
    mut f: impl FnMut(&[(E, Log)]) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    let mut upper_block = upper_limit_block_number;
    loop {
        // The scan has reach block 0. This can only really happen in tests.
        if upper_block == 0 {
            tracing::warn!("Scan for events reached block 0");
            break;
        }
        if upper_block < lower_limit_block_number {
            bail!("Search reached lower limit block number {lower_limit_block_number}");
        }

        // Calculate the block range to query: from lower_block to upper_block. Range is
        // inclusive of both lower and upper block.
        let lower_block =
            u64::max(upper_block.saturating_sub(chunk_size) + 1, lower_limit_block_number);

        // Set up the event filter for the specified block range
        tracing::debug!(range = ?(lower_block, upper_block), "Querying for events");
        let query_logs = Event::new(
            provider.clone(),
            filter.clone().from_block(lower_block).to_block(upper_block),
        )
        .query()
        .await
        .with_context(|| format!("Query for events in the range {lower_block} to {upper_block}"))?;

        // Check the predicate to see if the search should continue.
        if !f(&query_logs)? {
            break;
        }

        // Move the window down and continue.
        upper_block = lower_block.saturating_sub(1);
    }
    Ok(())
}
