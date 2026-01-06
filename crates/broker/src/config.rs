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

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use alloy::primitives::Address;
use anyhow::{Context, Result};
use boundless_market::dynamic_gas_filler::PriorityMode;
use notify::{EventKind, Watcher};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs,
    task::JoinHandle,
    time::{timeout, Duration},
};

use crate::{errors::CodedError, impl_coded_debug};

mod defaults {
    use super::PriorityMode;

    const ESTIMATED_GROTH16_TIME: u64 = 15;

    pub const fn max_journal_bytes() -> usize {
        10_000
    }

    pub const fn batch_max_journal_bytes() -> usize {
        10_000
    }

    pub const fn lockin_gas_estimate() -> u64 {
        // Observed cost of a lock transaction is ~135k gas.
        // https://sepolia.etherscan.io/tx/0xe61b5cad4a45fc0913cc966f8e3ee72027c01a949a9deca916780e1245c15964
        200_000
    }

    pub const fn fulfill_gas_estimate() -> u64 {
        // Observed cost of a basic single fulfill transaction is ~350k gas.
        // Additional padding is used to account for journals up to 10kB in size.
        // https://sepolia.etherscan.io/tx/0x14e54fbaf0c1eda20dd0828ddd64e255ffecee4562492f8c1253b0c3f20af764
        750_000
    }

    pub const fn groth16_verify_gas_estimate() -> u64 {
        250_000
    }

    pub const fn additional_proof_cycles() -> u64 {
        // 2 mcycles for assessor + 270k cycles for set builder by default
        2_000_000 + 270_000
    }

    pub fn priority_mode() -> PriorityMode {
        PriorityMode::Medium
    }

    pub const fn max_submission_attempts() -> u32 {
        2
    }

    pub const fn reaper_interval_secs() -> u32 {
        60
    }

    pub const fn reaper_grace_period_secs() -> u32 {
        10800
    }

    pub const fn max_concurrent_preflights() -> u32 {
        2
    }

    pub const fn max_file_size() -> usize {
        50_000_000
    }

    pub fn assessor_default_image_url() -> String {
        "https://signal-artifacts.beboundless.xyz/v3/assessor/assessor_guest.bin".to_string()
    }

    pub fn set_builder_default_image_url() -> String {
        "https://signal-artifacts.beboundless.xyz/v2/set-builder/guest.bin".to_string()
    }

    pub fn priority_requestor_lists() -> Option<Vec<String>> {
        Some(vec![
            "https://requestors.boundless.network/boundless-recommended-priority-list.standard.json".to_string()
        ])
    }

    pub fn allow_requestor_lists() -> Option<Vec<String>> {
        None
    }

    pub const fn max_mcycle_limit() -> u64 {
        8000
    }

    pub const fn min_mcycle_limit() -> u64 {
        0
    }

    pub const fn min_deadline() -> u64 {
        // Currently 150 seconds
        block_deadline_buffer_secs() + 30
    }

    pub const fn lookback_blocks() -> u64 {
        300
    }

    pub const fn events_poll_blocks() -> u64 {
        150
    }

    pub const fn events_poll_ms() -> u64 {
        3000
    }

    pub const fn max_concurrent_proofs() -> u32 {
        1
    }

    pub const fn status_poll_retry_count() -> u64 {
        3
    }

    pub const fn status_poll_ms() -> u64 {
        1000
    }

    pub const fn req_retry_count() -> u64 {
        3
    }

    pub const fn req_retry_sleep_ms() -> u64 {
        500
    }

    pub const fn proof_retry_count() -> u64 {
        1
    }

    pub const fn proof_retry_sleep_ms() -> u64 {
        500
    }

    pub const fn max_critical_task_retries() -> u32 {
        10
    }

    pub const fn batch_max_time() -> u64 {
        1000
    }

    pub const fn min_batch_size() -> u32 {
        1
    }

    pub const fn block_deadline_buffer_secs() -> u64 {
        // Currently 120 seconds
        txn_timeout() * max_submission_attempts() as u64 + ESTIMATED_GROTH16_TIME * 2
    }

    pub const fn txn_timeout() -> u64 {
        45
    }

    pub const fn single_txn_fulfill() -> bool {
        true
    }

    pub const fn withdraw() -> bool {
        false
    }
}

/// Order pricing priority mode for determining which orders to price first
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderPricingPriority {
    /// Process orders in random order to distribute competition among provers
    Random,
    /// Process orders in the order they were observed (FIFO)
    ObservationTime,
    /// Process orders by shortest expiry first (earliest deadline)
    ShortestExpiry,
}

impl Default for OrderPricingPriority {
    fn default() -> Self {
        Self::Random
    }
}

/// Order commitment priority mode for determining which orders to commit to first
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderCommitmentPriority {
    /// Process orders in random order to distribute competition among provers
    Random,
    /// Process orders by shortest expiry first (lock expiry for lock-and-fulfill orders, request expiry for others)
    ShortestExpiry,
    /// Process lock-and-fulfill orders by highest ETH payment, then fulfill-after-lock-expire random weighted by collateral reward
    Price,
    /// Process lock-and-fulfill orders by highest ETH price per cycle, then fulfill-after-lock-expire random weighted by collateral reward
    CyclePrice,
}

impl Default for OrderCommitmentPriority {
    fn default() -> Self {
        Self::CyclePrice
    }
}

/// All configuration related to markets mechanics
#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct MarketConf {
    /// Mega-cycle price, denominated in the native token (e.g. ETH).
    ///
    /// This price is multiplied the number of mega-cycles (i.e. million RISC-V cycles) that the requested
    /// execution took, as calculated by running the request in preflight. This is one of the inputs to
    /// decide the minimum price to accept for a request.
    #[serde(alias = "mcycle_price")]
    pub min_mcycle_price: String,
    /// Mega-cycle price, denominated in the Boundless collateral token.
    ///
    /// Similar to the mcycle_price option above. This is used to determine the minimum price to accept an
    /// order when paid in collateral tokens, as is the case for orders with an expired lock.
    #[serde(alias = "mcycle_price_collateral_token")]
    pub min_mcycle_price_collateral_token: String,
    /// Assumption price (in native token)
    ///
    /// DEPRECATED
    #[deprecated]
    pub assumption_price: Option<String>,
    /// Max cycles (in mcycles)
    ///
    /// Orders over this max_cycles will be skipped after preflight
    #[serde(default = "defaults::max_mcycle_limit")]
    pub max_mcycle_limit: u64,
    /// Min cycles (in mcycles)
    ///
    /// Orders under this min_cycles will be skipped after preflight
    #[serde(default = "defaults::min_mcycle_limit")]
    pub min_mcycle_limit: u64,
    /// Optional priority requestor addresses that can bypass the mcycle limit and max input size limit.
    ///
    /// If enabled, the order will be preflighted without constraints.
    #[serde(alias = "priority_requestor_addresses")]
    pub priority_requestor_addresses: Option<Vec<Address>>,
    /// Optional URLs to fetch requestor priority lists from.
    ///
    /// These lists will be periodically refreshed and merged with priority_requestor_addresses.
    #[serde(default = "defaults::priority_requestor_lists")]
    pub priority_requestor_lists: Option<Vec<String>>,
    /// Max journal size in bytes
    ///
    /// Orders that produce a journal larger than this size in preflight will be skipped. Since journals
    /// must be posted onchain to complete an order, an excessively large journal may prevent completion
    /// of a request.
    #[serde(default = "defaults::max_journal_bytes")]
    pub max_journal_bytes: usize,
    /// Estimated peak performance of the proving cluster, in kHz.
    ///
    /// Used to estimate proving capacity and accept only as much work as the prover can handle. Estimates
    /// can be derived from benchmarking using Bento CLI or from data based on fulfilling market orders.
    pub peak_prove_khz: Option<u64>,
    /// Min seconds left before the deadline to consider bidding on a request.
    ///
    /// If there is not enough time left before the deadline, the prover may not be able to complete
    /// proving of the request and finalize the batch for publishing before expiration.
    #[serde(default = "defaults::min_deadline")]
    pub min_deadline: u64,
    /// On startup, the number of blocks to look back for possible open orders.
    #[serde(default = "defaults::lookback_blocks")]
    pub lookback_blocks: u64,
    /// Number of blocks to query per batch when polling for new market events.
    ///
    /// After the initial lookback completes, the market monitor will query this many blocks
    /// at a time when polling for new events. A higher value reduces RPC calls but may increase
    /// response latency. A lower value provides more real-time updates but increases RPC load.
    #[serde(default = "defaults::events_poll_blocks")]
    pub events_poll_blocks: u64,
    /// Polling interval for market events (in milliseconds).
    ///
    /// The time between polls for new market events. A lower value provides more real-time updates
    /// but increases RPC load. A higher value reduces RPC calls but may increase response latency.
    #[serde(default = "defaults::events_poll_ms")]
    pub events_poll_ms: u64,
    /// Max collateral amount, denominated in the Boundless collateral token.
    ///
    /// Requests that require a higher collateral amount than this will not be considered.
    #[serde(alias = "max_stake")]
    pub max_collateral: String,
    /// Optional allow list for customer address.
    ///
    /// If enabled, all requests from clients not in the allow list are skipped.
    pub allow_client_addresses: Option<Vec<Address>>,
    /// Optional URLs to fetch requestor allow lists from.
    ///
    /// These lists will be periodically refreshed and merged with allow_client_addresses.
    #[serde(default = "defaults::allow_requestor_lists")]
    pub allow_requestor_lists: Option<Vec<String>>,
    /// Optional deny list for requestor address.
    ///
    /// If enabled, all requests from clients in the deny list are skipped.
    pub deny_requestor_addresses: Option<HashSet<Address>>,
    /// Transaction priority mode (low, medium, high, or custom)
    ///
    /// Controls the multipliers applied to the provider's baseline EIP-1559
    /// estimate. The baseline uses the median 20th-percentile tip from the last
    /// 10 blocks as `max_priority_fee_per_gas` and doubles the current base fee
    /// for `max_fee_per_gas`; `PriorityMode` then scales up that value based on the priority mode.
    /// Defaults to "medium". Use the `custom` variant to explicitly set the base-fee multiplier
    /// percentage (100 = 1x), priority-fee multiplier percentage, the fee-history percentile, and the
    /// dynamic multiplier, e.g.:
    /// `gas_priority_mode = { custom = { base_fee_multiplier_percentage = 300, priority_fee_multiplier_percentage = 150, priority_fee_percentile = 15.0, dynamic_multiplier_percentage = 5 } }`.
    #[serde(default = "defaults::priority_mode")]
    pub gas_priority_mode: PriorityMode,

    /// DEPRECATED: lockRequest priority gas
    ///
    /// Optional additional gas to add to the transaction for lockinRequest, good
    /// for increasing the priority if competing with multiple provers during the
    /// same block
    pub lockin_priority_gas: Option<u64>,
    /// Max input / image file size allowed for downloading from request URLs.
    #[serde(default = "defaults::max_file_size")]
    pub max_file_size: usize,
    /// Max retries for fetching input / image contents from URLs
    pub max_fetch_retries: Option<u8>,
    /// Gas estimate for lockin call
    ///
    /// Used for estimating the gas costs associated with an order during pricing. If not set a
    /// conservative default will be used.
    #[serde(default = "defaults::lockin_gas_estimate")]
    pub lockin_gas_estimate: u64,
    /// Gas estimate for fulfill call
    ///
    /// Used for estimating the gas costs associated with an order during pricing. If not set a
    /// conservative default will be used.
    #[serde(default = "defaults::fulfill_gas_estimate")]
    pub fulfill_gas_estimate: u64,
    /// Gas estimate for proof verification using the RiscZeroGroth16Verifier
    ///
    /// Used for estimating the gas costs associated with an order during pricing. If not set a
    /// conservative default will be used.
    #[serde(default = "defaults::groth16_verify_gas_estimate")]
    pub groth16_verify_gas_estimate: u64,
    /// Additional cycles to be proven for each order.
    ///
    /// This is currently the sum of the cycles for the assessor and set builder.
    #[serde(default = "defaults::additional_proof_cycles")]
    pub additional_proof_cycles: u64,
    /// Optional balance warning threshold (in native token)
    ///
    /// If the submitter balance drops below this the broker will issue warning logs
    pub balance_warn_threshold: Option<String>,
    /// Optional balance error threshold (in native token)
    ///
    /// If the submitter balance drops below this the broker will issue error logs
    pub balance_error_threshold: Option<String>,
    /// Optional collateral balance warning threshold (in collateral tokens)
    ///
    /// If the collateral balance drops below this the broker will issue warning logs
    #[serde(alias = "stake_balance_warn_threshold")]
    pub collateral_balance_warn_threshold: Option<String>,
    /// Optional collateral balance error threshold (in collateral tokens)
    ///
    /// If the collateral balance drops below this the broker will issue error logs
    #[serde(alias = "stake_balance_error_threshold")]
    pub collateral_balance_error_threshold: Option<String>,
    /// Max concurrent proofs
    ///
    /// Maximum number of concurrent proofs that can be processed at once
    #[serde(alias = "max_concurrent_locks", default = "defaults::max_concurrent_proofs")]
    pub max_concurrent_proofs: u32,
    /// Optional cache directory for storing downloaded images and inputs
    ///
    /// If not set, files will be re-downloaded every time
    pub cache_dir: Option<PathBuf>,
    /// Default URL for assessor image
    ///
    /// This URL will be tried first before falling back to the contract URL
    #[serde(default = "defaults::assessor_default_image_url")]
    pub assessor_default_image_url: String,
    /// Default URL for set builder image
    ///
    /// This URL will be tried first before falling back to the contract URL
    #[serde(default = "defaults::set_builder_default_image_url")]
    pub set_builder_default_image_url: String,
    /// Maximum number of orders to concurrently work on pricing
    ///
    /// Used to limit pricing tasks spawned to prevent overwhelming the system
    #[serde(default = "defaults::max_concurrent_preflights")]
    pub max_concurrent_preflights: u32,
    /// Order pricing priority mode
    ///
    /// Determines how orders are prioritized for pricing. Options:
    /// - "random": Process orders in random order to distribute competition among provers (default)
    /// - "observation_time": Process orders in the order they were observed (FIFO)
    /// - "shortest_expiry": Process orders by shortest expiry first (earliest deadline)
    #[serde(default)]
    pub order_pricing_priority: OrderPricingPriority,
    /// Order commitment priority mode
    ///
    /// Determines how orders are prioritized when committing to prove them. Options:
    /// - "random": Process orders in random order to distribute competition among provers (default)
    /// - "shortest_expiry": Process orders by shortest expiry first (lock expiry for lock-and-fulfill orders, request expiry for others)
    /// - "lock_price": Process lock-and-fulfill orders by highest ETH payment, then fulfill-after-lock-expire randomly
    /// - "lock_cycle_price": Process lock-and-fulfill orders by highest ETH price per cycle, then fulfill-after-lock-expire randomly
    #[serde(default, alias = "expired_order_fulfillment_priority")]
    pub order_commitment_priority: OrderCommitmentPriority,
    /// Whether to cancel Bento proving sessions when the order is no longer actionable
    /// If false (default), Bento proving continues even if the order cannot be fulfilled in the
    /// market. This should remain false to avoid losing partial PoVW jobs.
    #[serde(default)]
    pub cancel_proving_expired_orders: bool,
}

impl Default for MarketConf {
    fn default() -> Self {
        // Allow use of assumption_price until it is removed.
        #[allow(deprecated)]
        Self {
            min_mcycle_price: "0.00001".to_string(),
            min_mcycle_price_collateral_token: "0.001".to_string(),
            assumption_price: None,
            max_mcycle_limit: defaults::max_mcycle_limit(),
            min_mcycle_limit: defaults::min_mcycle_limit(),
            priority_requestor_addresses: None,
            priority_requestor_lists: defaults::priority_requestor_lists(),
            allow_requestor_lists: defaults::allow_requestor_lists(),
            max_journal_bytes: defaults::max_journal_bytes(),
            peak_prove_khz: None,
            min_deadline: defaults::min_deadline(),
            lookback_blocks: defaults::lookback_blocks(),
            events_poll_blocks: defaults::events_poll_blocks(),
            events_poll_ms: defaults::events_poll_ms(),
            max_collateral: "0.1".to_string(),
            allow_client_addresses: None,
            deny_requestor_addresses: None,
            gas_priority_mode: defaults::priority_mode(),
            lockin_priority_gas: None,
            max_file_size: defaults::max_file_size(),
            max_fetch_retries: Some(2),
            lockin_gas_estimate: defaults::lockin_gas_estimate(),
            fulfill_gas_estimate: defaults::fulfill_gas_estimate(),
            groth16_verify_gas_estimate: defaults::groth16_verify_gas_estimate(),
            additional_proof_cycles: defaults::additional_proof_cycles(),
            balance_warn_threshold: None,
            balance_error_threshold: None,
            collateral_balance_warn_threshold: None,
            collateral_balance_error_threshold: None,
            max_concurrent_proofs: defaults::max_concurrent_proofs(),
            cache_dir: None,
            assessor_default_image_url: defaults::assessor_default_image_url(),
            set_builder_default_image_url: defaults::set_builder_default_image_url(),
            max_concurrent_preflights: defaults::max_concurrent_preflights(),
            order_pricing_priority: OrderPricingPriority::default(),
            order_commitment_priority: OrderCommitmentPriority::default(),
            cancel_proving_expired_orders: false,
        }
    }
}

/// All configuration related to prover (bonsai / Bento) mechanics
#[derive(Debug, Deserialize, Serialize)]
pub struct ProverConf {
    /// Number of retries to poll for proving status.
    ///
    /// Provides a little durability for transient failures.
    #[serde(default = "defaults::status_poll_retry_count")]
    pub status_poll_retry_count: u64,
    /// Polling interval to monitor proving status (in millisecs)
    #[serde(default = "defaults::status_poll_ms")]
    pub status_poll_ms: u64,
    /// Optional config, if using bonsai set the zkVM version here
    pub bonsai_r0_zkvm_ver: Option<String>,
    /// Number of retries to query a prover backend for on failures.
    ///
    /// Used for API requests to a prover backend, creating sessions, preflighting, uploading images, etc.
    /// Provides a little durability for transient failures.
    #[serde(default = "defaults::req_retry_count")]
    pub req_retry_count: u64,
    /// Number of milliseconds to sleep between retries.
    #[serde(default = "defaults::req_retry_sleep_ms")]
    pub req_retry_sleep_ms: u64,
    /// Number of retries to for running the entire proof generation process
    ///
    /// This is separate from the request retry count, as the proving process
    /// is a multi-step process involving multiple API calls to create a proof
    /// job and then polling for the proof job to complete.
    #[serde(default = "defaults::proof_retry_count")]
    pub proof_retry_count: u64,
    /// Number of milliseconds to sleep between proof retries.
    #[serde(default = "defaults::proof_retry_sleep_ms")]
    pub proof_retry_sleep_ms: u64,
    /// Set builder guest program binary path
    ///
    /// When using a durable deploy, set this to the published current SOT guest program binary
    /// path on the system
    pub set_builder_guest_path: Option<PathBuf>,
    /// Assessor program path
    pub assessor_set_guest_path: Option<PathBuf>,
    /// Max critical task retries on recoverable failures.
    ///
    /// The broker service has a number of subtasks. Some are considered critical. If a task fails, it
    /// will be retried, but after this number of retries, the process will exit.
    #[serde(default = "defaults::max_critical_task_retries")]
    pub max_critical_task_retries: u32,
    /// Interval for checking expired committed orders (in seconds)
    ///
    /// This is the interval at which the ReaperTask will check for expired orders and mark them as failed.
    /// If not set, it defaults to 60 seconds.
    #[serde(default = "defaults::reaper_interval_secs")]
    pub reaper_interval_secs: u32,
    /// Grace period before marking expired orders as failed (in seconds)
    ///
    /// This provides a buffer time after an order expires before the reaper marks it as failed.
    /// This helps prevent race conditions with the aggregator that might be processing the order.
    /// If not set, it defaults to 30 seconds.
    #[serde(default = "defaults::reaper_grace_period_secs")]
    pub reaper_grace_period_secs: u32,
}

impl Default for ProverConf {
    fn default() -> Self {
        Self {
            status_poll_retry_count: defaults::status_poll_retry_count(),
            status_poll_ms: defaults::status_poll_ms(),
            bonsai_r0_zkvm_ver: None,
            req_retry_count: defaults::req_retry_count(),
            req_retry_sleep_ms: defaults::req_retry_sleep_ms(),
            proof_retry_count: defaults::proof_retry_count(),
            proof_retry_sleep_ms: defaults::proof_retry_sleep_ms(),
            set_builder_guest_path: None,
            assessor_set_guest_path: None,
            max_critical_task_retries: defaults::max_critical_task_retries(),
            reaper_interval_secs: defaults::reaper_interval_secs(),
            reaper_grace_period_secs: defaults::reaper_grace_period_secs(),
        }
    }
}

/// All configuration related to batching / aggregation
#[derive(Debug, Deserialize, Serialize)]
pub struct BatcherConfig {
    /// Max batch duration before publishing (in seconds)
    #[serde(default = "defaults::batch_max_time")]
    pub batch_max_time: u64,
    /// Batch size (in proofs) before publishing
    #[serde(alias = "batch_size", default = "defaults::min_batch_size")]
    pub min_batch_size: u32,
    /// Max combined journal size (in bytes) that once exceeded will trigger a publish
    #[serde(default = "defaults::batch_max_journal_bytes")]
    pub batch_max_journal_bytes: usize,
    /// max batch fees (in ETH) before publishing
    pub batch_max_fees: Option<String>,
    /// Batch blocktime buffer
    ///
    /// Number of seconds before the lowest block deadline in the order batch
    /// to flush the batch. This should be approximately snark_proving_time * 2
    #[serde(default = "defaults::block_deadline_buffer_secs")]
    pub block_deadline_buffer_secs: u64,
    /// Timeout, in seconds for transaction confirmations
    #[serde(default = "defaults::txn_timeout")]
    pub txn_timeout: u64,
    /// Polling time, in milliseconds
    ///
    /// The time between polls for new orders to aggregate and how often to check for batch finalize
    /// conditions
    pub batch_poll_time_ms: Option<u64>,
    /// Use the single TXN submission that batches submit_merkle / fulfill_batch into
    ///
    /// A single transaction. Requires the `submitRootAndFulfill` method
    /// be present on the deployed contract
    #[serde(default = "defaults::single_txn_fulfill")]
    pub single_txn_fulfill: bool,
    /// Whether to withdraw from the prover balance when fulfilling
    #[serde(default = "defaults::withdraw")]
    pub withdraw: bool,
    /// Number of attempts to make to submit a batch before abandoning
    #[serde(default = "defaults::max_submission_attempts")]
    pub max_submission_attempts: u32,
}

impl Default for BatcherConfig {
    fn default() -> Self {
        Self {
            batch_max_time: defaults::batch_max_time(),
            min_batch_size: defaults::min_batch_size(),
            batch_max_journal_bytes: defaults::batch_max_journal_bytes(),
            batch_max_fees: None,
            block_deadline_buffer_secs: defaults::block_deadline_buffer_secs(),
            txn_timeout: defaults::txn_timeout(),
            batch_poll_time_ms: Some(1000),
            single_txn_fulfill: defaults::single_txn_fulfill(),
            withdraw: defaults::withdraw(),
            max_submission_attempts: defaults::max_submission_attempts(),
        }
    }
}

/// Top level config for the broker service
#[derive(Deserialize, Serialize, Default, Debug)]
pub struct Config {
    /// Market / bidding configurations
    #[serde(default)]
    pub market: MarketConf,
    /// Prover backend configs
    #[serde(default)]
    pub prover: ProverConf,
    /// Aggregation batch configs
    #[serde(default)]
    pub batcher: BatcherConfig,
}

impl Config {
    /// Load the config from disk
    pub async fn load(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)
            .await
            .context(format!("Failed to read config file from {path:?}"))?;
        toml::from_str(&data).context(format!("Failed to parse toml file from {path:?}"))
    }

    /// Write the config to disk
    #[cfg(feature = "test-utils")]
    pub async fn write(&self, path: &Path) -> Result<()> {
        let data = toml::to_string(&self).context("Failed to serialize config")?;
        fs::write(path, data).await.context("Failed to write Config to disk")
    }
}

#[derive(Error)]
pub enum ConfigErr {
    #[error("Failed to lock internal config structure")]
    LockFailed,

    #[error("Invalid configuration")]
    InvalidConfig,
}

impl_coded_debug!(ConfigErr);

impl CodedError for ConfigErr {
    fn code(&self) -> &str {
        match self {
            ConfigErr::LockFailed => "[B-CON-3012]",
            ConfigErr::InvalidConfig => "[B-CON-3013]",
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct ConfigLock {
    config: Arc<RwLock<Config>>,
}

impl ConfigLock {
    fn new(config: Arc<RwLock<Config>>) -> Self {
        Self { config }
    }

    pub fn lock_all(&self) -> Result<std::sync::RwLockReadGuard<'_, Config>, ConfigErr> {
        self.config.read().map_err(|_| ConfigErr::LockFailed)
    }

    #[cfg(test)]
    pub fn load_write(&self) -> Result<std::sync::RwLockWriteGuard<'_, Config>, ConfigErr> {
        self.config.write().map_err(|_| ConfigErr::LockFailed)
    }
}

/// Max number of pending filesystem events from the config file
const FILE_MONITOR_EVENT_BUFFER: usize = 32;

/// Monitor service for watching config files for changes
pub struct ConfigWatcher {
    /// Current config data
    pub config: ConfigLock,
    /// monitor task handle
    // TODO: Need to join and monitor this handle
    _monitor: JoinHandle<Result<()>>,
}

impl ConfigWatcher {
    /// Initialize a new config watcher and handle
    pub async fn new(config_path: &Path) -> Result<Self> {
        let initial_config = Config::load(config_path).await?;
        log_deprecated_config_usage(&initial_config);
        let config = Arc::new(RwLock::new(initial_config));
        let config_copy = config.clone();
        let config_path_copy = config_path.to_path_buf();

        let startup_notification = Arc::new(tokio::sync::Notify::new());
        let startup_notification_copy = startup_notification.clone();

        let monitor = tokio::spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::channel(FILE_MONITOR_EVENT_BUFFER);

            let mut watcher = notify::recommended_watcher(move |res| match res {
                Ok(event) => {
                    // tracing::debug!("watch event: {event:?}");
                    if let Err(err) = tx.try_send(event) {
                        // TODO we hit TrySendError::Closed if the ConfigWatcher is dropped
                        // it would be nice to auto un-watch the file and shutdown in a cleaner
                        // order
                        tracing::debug!("Failed to send filesystem event to channel: {err:?}");
                    }
                }
                Err(err) => tracing::error!("Failed to watch config file: {err:?}"),
            })
            .context("Failed to construct watcher")?;

            watcher
                .watch(&config_path_copy, notify::RecursiveMode::NonRecursive)
                .context("Failed to start watcher")?;
            startup_notification_copy.notify_one();

            while let Some(event) = rx.recv().await {
                // tracing::debug!("Got event: {event:?}");
                match event.kind {
                    EventKind::Modify(_) => {
                        tracing::debug!("Reloading modified config file");
                        let new_config = match Config::load(&config_path_copy).await {
                            Ok(val) => val,
                            Err(err) => {
                                tracing::error!("Failed to load modified config: {err:?}");
                                continue;
                            }
                        };
                        let mut config = match config_copy.write() {
                            Ok(val) => val,
                            Err(err) => {
                                tracing::error!(
                                    "Failed to lock config, previously poisoned? {err:?}"
                                );
                                continue;
                            }
                        };
                        log_deprecated_config_usage(&new_config);
                        *config = new_config;
                    }
                    _ => {
                        tracing::debug!("unsupported config file event: {event:?}");
                    }
                }
            }

            watcher.unwatch(&config_path_copy).context("Failed to stop watching config")?;

            Ok(())
        });

        // Wait for successful start up, if failed return the Result
        if let Err(err) = timeout(Duration::from_secs(1), startup_notification.notified()).await {
            tracing::error!("Failed to get notification from config monitor startup in: {err}");
            let task_res = monitor.await.context("Config watcher startup failed")?;
            match task_res {
                Ok(_) => unreachable!("Startup failed to notify in timeout but exited cleanly"),
                Err(err) => return Err(err),
            }
        }
        tracing::debug!("Successful startup");

        Ok(Self { config: ConfigLock::new(config), _monitor: monitor })
    }
}

fn log_deprecated_config_usage(config: &Config) {
    if config.market.lockin_priority_gas.is_some() {
        tracing::warn!(
            "market.lockin_priority_gas is deprecated and ignored; configure market.gas_priority_mode instead"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        io::{Seek, Write},
    };
    use tempfile::NamedTempFile;
    use tracing_test::traced_test;

    const CONFIG_TEMPL: &str = r#"
[market]
mcycle_price = "0.1"
mcycle_price_collateral_token = "0.1"
peak_prove_khz = 500
min_deadline = 300
lookback_blocks = 100
max_stake = "0.1"
max_file_size = 50_000_000
min_mcycle_limit = 5

[prover]
bonsai_r0_zkvm_ver = "1.0.1"
status_poll_retry_count = 3
status_poll_ms = 1000
req_retry_count = 3
req_retry_sleep_ms = 500
proof_retry_count = 1
proof_retry_sleep_ms = 500

[batcher]
batch_max_time = 300
min_batch_size = 2
batch_max_fees = "0.1"
block_deadline_buffer_secs = 120"#;

    const CONFIG_TEMPL_2: &str = r#"
[market]
mcycle_price = "0.1"
mcycle_price_collateral_token = "0.1"
assumption_price = "0.1"
peak_prove_khz = 10000
min_deadline = 300
lookback_blocks = 100
max_stake = "0.1"
max_file_size = 50_000_000
max_fetch_retries = 10
allow_client_addresses = ["0x0000000000000000000000000000000000000000"]
deny_requestor_addresses = ["0x0000000000000000000000000000000000000000"]
gas_priority_mode = "high"
max_mcycle_limit = 10
min_mcycle_limit = 5

[prover]
status_poll_retry_count = 2
status_poll_ms = 1000
req_retry_count = 1
req_retry_sleep_ms = 200
proof_retry_count = 1
proof_retry_sleep_ms = 500


[batcher]
batch_max_time = 300
batch_size = 3
block_deadline_buffer_secs = 120
txn_timeout = 45
batch_poll_time_ms = 1200
single_txn_fulfill = true
withdraw = true"#;

    const CONFIG_CUSTOM_PRIORITY_MODE: &str = r#"
[market]
mcycle_price = "0.2"
mcycle_price_collateral_token = "0.2"
max_stake = "0.1"
gas_priority_mode = { custom = { priority_fee_multiplier_percentage = 150, priority_fee_percentile = 15.0, dynamic_multiplier_percentage = 9 } }
"#;

    const BAD_CONFIG: &str = r#"
[market]
error = ?"#;

    fn write_config(data: &str, file: &mut File) {
        file.seek(std::io::SeekFrom::Start(0)).unwrap();
        file.write_all(data.as_bytes()).unwrap();
        file.set_len(data.len() as u64).unwrap();
    }

    #[allow(deprecated)]
    #[tokio::test]
    async fn config_parser() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(CONFIG_TEMPL, config_temp.as_file_mut());
        let config = Config::load(config_temp.path()).await.unwrap();

        assert_eq!(config.market.min_mcycle_price, "0.1");
        assert_eq!(config.market.assumption_price, None);
        assert_eq!(config.market.peak_prove_khz, Some(500));
        assert_eq!(config.market.min_deadline, 300);
        assert_eq!(config.market.lookback_blocks, 100);
        assert_eq!(config.market.max_collateral, "0.1");
        assert_eq!(config.market.max_file_size, 50_000_000);
        assert_eq!(config.market.min_mcycle_limit, 5);
        assert_eq!(config.market.gas_priority_mode, PriorityMode::Medium);

        assert_eq!(config.prover.status_poll_ms, 1000);
        assert_eq!(config.prover.status_poll_retry_count, 3);
        assert_eq!(config.prover.bonsai_r0_zkvm_ver.unwrap(), "1.0.1");
        assert_eq!(config.prover.req_retry_count, 3);
        assert_eq!(config.prover.req_retry_sleep_ms, 500);
        assert_eq!(config.prover.proof_retry_count, 1);
        assert_eq!(config.prover.proof_retry_sleep_ms, 500);
        assert_eq!(config.prover.set_builder_guest_path, None);
        assert_eq!(config.prover.assessor_set_guest_path, None);

        assert_eq!(config.batcher.batch_max_time, 300);
        assert_eq!(config.batcher.min_batch_size, 2);
        assert_eq!(config.batcher.batch_max_fees, Some("0.1".into()));
        assert_eq!(config.batcher.block_deadline_buffer_secs, 120);
        assert_eq!(config.batcher.txn_timeout, 45);
        assert_eq!(config.batcher.batch_poll_time_ms, None);
    }

    #[tokio::test]
    async fn config_parser_custom_priority_mode() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(CONFIG_CUSTOM_PRIORITY_MODE, config_temp.as_file_mut());
        let config = Config::load(config_temp.path()).await.unwrap();

        assert_eq!(
            config.market.gas_priority_mode,
            PriorityMode::Custom {
                base_fee_multiplier_percentage: 200,
                priority_fee_multiplier_percentage: 150,
                priority_fee_percentile: 15.0,
                dynamic_multiplier_percentage: 9,
            }
        );
    }

    #[tokio::test]
    #[should_panic(expected = "TOML parse error")]
    async fn bad_config() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(BAD_CONFIG, config_temp.as_file_mut());
        Config::load(config_temp.path()).await.unwrap();
    }

    #[allow(deprecated)]
    #[tokio::test]
    #[traced_test]
    async fn config_watcher() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(CONFIG_TEMPL, config_temp.as_file_mut());
        let config_mgnr = ConfigWatcher::new(config_temp.path()).await.unwrap();

        {
            let config = config_mgnr.config.lock_all().unwrap();
            assert_eq!(config.market.min_mcycle_price, "0.1");
            assert_eq!(config.market.assumption_price, None);
            assert_eq!(config.market.peak_prove_khz, Some(500));
            assert_eq!(config.market.min_deadline, 300);
            assert_eq!(config.market.lookback_blocks, 100);
            assert_eq!(config.market.max_mcycle_limit, 8000);
            assert_eq!(config.market.min_mcycle_limit, 5);
            assert_eq!(config.prover.status_poll_ms, 1000);
        }

        write_config(CONFIG_TEMPL_2, config_temp.as_file_mut());
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        {
            tracing::debug!("Locking config for reading...");
            let config = config_mgnr.config.lock_all().unwrap();
            assert_eq!(config.market.min_mcycle_price, "0.1");
            assert_eq!(config.market.assumption_price, Some("0.1".into()));
            assert_eq!(config.market.peak_prove_khz, Some(10000));
            assert_eq!(config.market.min_deadline, 300);
            assert_eq!(config.market.lookback_blocks, 100);
            assert_eq!(config.market.allow_client_addresses, Some(vec![Address::ZERO]));
            assert_eq!(
                config.market.deny_requestor_addresses,
                Some([Address::ZERO].into_iter().collect())
            );
            assert_eq!(config.market.gas_priority_mode, PriorityMode::High);
            assert_eq!(config.market.max_fetch_retries, Some(10));
            assert_eq!(config.market.max_mcycle_limit, 10);
            assert_eq!(config.market.min_mcycle_limit, 5);
            assert_eq!(config.prover.status_poll_ms, 1000);
            assert_eq!(config.prover.status_poll_retry_count, 2);
            assert_eq!(config.prover.req_retry_count, 1);
            assert_eq!(config.prover.req_retry_sleep_ms, 200);
            assert_eq!(config.prover.proof_retry_count, 1);
            assert_eq!(config.prover.proof_retry_sleep_ms, 500);
            assert!(config.prover.bonsai_r0_zkvm_ver.is_none());
            assert_eq!(config.batcher.txn_timeout, 45);
            assert_eq!(config.batcher.batch_poll_time_ms, Some(1200));
            assert_eq!(config.batcher.min_batch_size, 3);
            assert!(config.batcher.single_txn_fulfill);
            assert!(config.batcher.withdraw);
        }
        tracing::debug!("closing...");
    }

    #[tokio::test]
    #[traced_test]
    #[should_panic(expected = "Failed to parse toml file")]
    async fn watcher_fail_startup() {
        let mut config_temp = NamedTempFile::new().unwrap();
        write_config(BAD_CONFIG, config_temp.as_file_mut());
        ConfigWatcher::new(config_temp.path()).await.unwrap();
    }
}
