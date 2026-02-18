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

//! Prover configuration structures for the Boundless market.
//!
//! This module contains the configuration types used by provers/brokers when
//! interacting with the Boundless market.

#![cfg_attr(not(feature = "prover_utils"), allow(dead_code))]

#[cfg(any(feature = "prover_utils", feature = "test-utils"))]
use std::path::Path;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use alloy::primitives::{Address, FixedBytes};
#[cfg(any(feature = "prover_utils", feature = "test-utils"))]
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
#[cfg(any(feature = "prover_utils", feature = "test-utils"))]
use tokio::fs;

use crate::dynamic_gas_filler::PriorityMode;
use crate::price_oracle::{Amount, Asset, PriceOracleConfig};

pub mod defaults {
    use url::Url;

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

    pub const fn max_fetch_retries() -> Option<u8> {
        Some(2)
    }

    pub fn ipfs_gateway() -> Option<url::Url> {
        Some(Url::parse(crate::storage::DEFAULT_IPFS_GATEWAY_URL).unwrap())
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

    /// Recommended max collateral for standard requestor list (lower risk) in USD.
    pub const MAX_COLLATERAL_STANDARD: &str = "10";

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

/// Deserialize Amount with validation that asset is USD or ETH.
/// Plain numbers without asset suffix default to ETH for backward compatibility.
fn deserialize_mcycle_price<'de, D>(deserializer: D) -> Result<Amount, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Amount::parse_with_allowed(&s, &[Asset::USD, Asset::ETH], Some(Asset::ETH))
        .map_err(serde::de::Error::custom)
}

/// Deserialize Amount with validation that asset is USD or ZKC.
/// Plain numbers without asset suffix default to ZKC for backward compatibility.
fn deserialize_max_collateral<'de, D>(deserializer: D) -> Result<Amount, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Amount::parse_with_allowed(&s, &[Asset::USD, Asset::ZKC], Some(Asset::ZKC))
        .map_err(serde::de::Error::custom)
}

/// Deserialize Amount with validation that asset is USD or ZKC.
/// Plain numbers without asset suffix default to ZKC for backward compatibility.
fn deserialize_mcycle_price_collateral_token<'de, D>(deserializer: D) -> Result<Amount, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Amount::parse_with_allowed(&s, &[Asset::USD, Asset::ZKC], Some(Asset::ZKC))
        .map_err(serde::de::Error::custom)
}

/// All configuration related to markets mechanics
#[derive(Debug, Deserialize, Serialize, Clone)]
#[non_exhaustive]
pub struct MarketConfig {
    /// Minimum price per mega-cycle. Asset can be specified: "0.00001 USD" or "0.00000001 ETH"
    /// Plain numbers default to ETH for backward compatibility.
    ///
    /// If USD, converted to target token at runtime via price oracle.
    ///
    /// This price is multiplied by the number of mega-cycles (i.e. million RISC-V cycles) that the requested
    /// execution took, as calculated by running the request in preflight. This is one of the inputs to
    /// decide the minimum price to accept for a request.
    #[serde(alias = "mcycle_price", deserialize_with = "deserialize_mcycle_price")]
    pub min_mcycle_price: Amount,
    /// Mega-cycle price, denominated in the Boundless collateral token. Asset can be specified: "0.001 ZKC" or "1 USD"
    /// Plain numbers default to ZKC for backward compatibility.
    ///
    /// If USD, converted to ZKC at runtime via price oracle.
    ///
    /// Similar to the mcycle_price option above. This is used to determine the minimum price to accept an
    /// order when paid in collateral tokens, as is the case for orders with an expired lock.
    #[serde(
        alias = "mcycle_price_collateral_token",
        deserialize_with = "deserialize_mcycle_price_collateral_token"
    )]
    pub min_mcycle_price_collateral_token: Amount,
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
    /// Max collateral amount. Asset can be specified: "50 ZKC" or "100 USD"
    /// Plain numbers default to ZKC for backward compatibility.
    ///
    /// If USD, converted to ZKC at runtime via price oracle.
    /// Requests that require a higher collateral amount than this will not be considered.
    #[serde(alias = "max_stake", deserialize_with = "deserialize_max_collateral")]
    pub max_collateral: Amount,
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
    /// Optional IPFS gateway URL for fallback when downloading IPFS content
    ///
    /// When set, if an HTTP download fails for a URL containing `/ipfs/`,
    /// the downloader will retry with this gateway.
    #[serde(default = "defaults::ipfs_gateway")]
    pub ipfs_gateway_fallback: Option<url::Url>,
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
    /// Optional path to a JSON file containing per-requestor and per-selector pricing overrides.
    ///
    /// The file is hot-reloaded automatically (every 60 s); no broker restart needed.
    /// See [`PricingOverrides`] for the file format.
    #[serde(default)]
    pub pricing_overrides_path: Option<PathBuf>,
}

impl Default for MarketConfig {
    fn default() -> Self {
        // Allow use of assumption_price until it is removed.
        #[allow(deprecated)]
        Self {
            min_mcycle_price: Amount::parse("0.00002 USD", None).expect("valid default"),
            min_mcycle_price_collateral_token: Amount::parse("0.001 ZKC", None)
                .expect("valid default"),
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
            max_collateral: Amount::parse("10 USD", None).expect("valid default"),
            allow_client_addresses: None,
            deny_requestor_addresses: None,
            gas_priority_mode: defaults::priority_mode(),
            lockin_priority_gas: None,
            max_file_size: defaults::max_file_size(),
            max_fetch_retries: defaults::max_fetch_retries(),
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
            ipfs_gateway_fallback: defaults::ipfs_gateway(),
            assessor_default_image_url: defaults::assessor_default_image_url(),
            set_builder_default_image_url: defaults::set_builder_default_image_url(),
            max_concurrent_preflights: defaults::max_concurrent_preflights(),
            order_pricing_priority: OrderPricingPriority::default(),
            order_commitment_priority: OrderCommitmentPriority::default(),
            cancel_proving_expired_orders: false,
            pricing_overrides_path: None,
        }
    }
}

/// All configuration related to prover (bonsai / Bento) mechanics
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProverConfig {
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

impl Default for ProverConfig {
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
    pub market: MarketConfig,
    /// Prover backend configs
    #[serde(default)]
    pub prover: ProverConfig,
    /// Aggregation batch configs
    #[serde(default)]
    pub batcher: BatcherConfig,
    /// Price oracle configuration
    #[serde(default)]
    pub price_oracle: PriceOracleConfig,
}

impl Config {
    /// Load the config from disk
    #[cfg(feature = "prover_utils")]
    pub async fn load(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)
            .await
            .context(format!("Failed to read config file from {path:?}"))?;
        toml::from_str(&data).context(format!("Failed to parse toml file from {path:?}"))
    }

    /// Write the config to disk
    #[cfg(feature = "test-utils")]
    #[allow(dead_code)]
    pub async fn write(&self, path: &Path) -> Result<()> {
        let data = toml::to_string(&self).context("Failed to serialize config")?;
        fs::write(path, data).await.context("Failed to write Config to disk")
    }
}

/// A single pricing override entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PricingOverrideEntry {
    /// Minimum price per mega-cycle for this override.
    pub min_mcycle_price: Amount,
}

/// Per-requestor and per-selector pricing overrides loaded from a JSON file.
///
/// Resolution priority (first match wins):
/// 1. `by_requestor_selector` -- keyed by `"<address>:<selector_hex>"`
/// 2. `by_selector` -- keyed by selector hex (e.g. `"0x12345678"`)
/// 3. `by_requestor` -- keyed by requestor address
/// 4. Fall back to global `MarketConfig::min_mcycle_price`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PricingOverrides {
    /// Overrides keyed by requestor address (checksummed or lowercase hex).
    #[serde(default)]
    pub by_requestor: HashMap<Address, PricingOverrideEntry>,
    /// Overrides keyed by selector hex (e.g. `"0x12345678"`).
    #[serde(default)]
    pub by_selector: HashMap<FixedBytes<4>, PricingOverrideEntry>,
    /// Overrides keyed by `"<requestor_address>:<selector_hex>"` for the most specific match.
    #[serde(
        default,
        deserialize_with = "deserialize_requestor_selector_map",
        serialize_with = "serialize_requestor_selector_map"
    )]
    pub by_requestor_selector: HashMap<(Address, FixedBytes<4>), PricingOverrideEntry>,
}

fn deserialize_requestor_selector_map<'de, D>(
    deserializer: D,
) -> Result<HashMap<(Address, FixedBytes<4>), PricingOverrideEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: HashMap<String, PricingOverrideEntry> = HashMap::deserialize(deserializer)?;
    let mut map = HashMap::with_capacity(raw.len());
    for (key, entry) in raw {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() != 2 {
            return Err(serde::de::Error::custom(format!(
                "by_requestor_selector key must be '<address>:<selector>', got '{key}'"
            )));
        }
        let addr: Address = parts[0].parse().map_err(serde::de::Error::custom)?;
        let sel: FixedBytes<4> = parts[1].parse().map_err(serde::de::Error::custom)?;
        map.insert((addr, sel), entry);
    }
    Ok(map)
}

fn serialize_requestor_selector_map<S>(
    map: &HashMap<(Address, FixedBytes<4>), PricingOverrideEntry>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut ser_map = serializer.serialize_map(Some(map.len()))?;
    for ((addr, sel), entry) in map {
        ser_map.serialize_entry(&format!("{addr}:{sel}"), entry)?;
    }
    ser_map.end()
}

impl PricingOverrides {
    /// Load overrides from a JSON file.
    #[cfg(any(feature = "prover_utils", feature = "test-utils"))]
    pub async fn load(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)
            .await
            .context(format!("Failed to read pricing overrides from {path:?}"))?;
        serde_json::from_str(&data)
            .context(format!("Failed to parse pricing overrides from {path:?}"))
    }

    /// Resolve the effective `min_mcycle_price` for a given requestor and selector.
    ///
    /// Returns `Some(amount)` if an override matches, `None` to use the global default.
    pub fn resolve(&self, requestor: &Address, selector: &FixedBytes<4>) -> Option<&Amount> {
        if let Some(entry) = self.by_requestor_selector.get(&(*requestor, *selector)) {
            return Some(&entry.min_mcycle_price);
        }
        if let Some(entry) = self.by_selector.get(selector) {
            return Some(&entry.min_mcycle_price);
        }
        if let Some(entry) = self.by_requestor.get(requestor) {
            return Some(&entry.min_mcycle_price);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> Address {
        s.parse().unwrap()
    }

    fn sel(s: &str) -> FixedBytes<4> {
        s.parse().unwrap()
    }

    fn amount(s: &str) -> Amount {
        Amount::parse(s, None).unwrap()
    }

    fn entry(s: &str) -> PricingOverrideEntry {
        PricingOverrideEntry { min_mcycle_price: amount(s) }
    }

    #[test]
    fn test_resolve_empty_returns_none() {
        let overrides = PricingOverrides::default();
        let result = overrides
            .resolve(&addr("0x0000000000000000000000000000000000000001"), &sel("0x12345678"));
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_by_requestor() {
        let requestor = addr("0x0000000000000000000000000000000000000001");
        let mut overrides = PricingOverrides::default();
        overrides.by_requestor.insert(requestor, entry("0.001 USD"));

        let result = overrides.resolve(&requestor, &sel("0xaabbccdd"));
        assert_eq!(result.unwrap().to_string(), "0.001 USD");

        let other = addr("0x0000000000000000000000000000000000000002");
        assert!(overrides.resolve(&other, &sel("0xaabbccdd")).is_none());
    }

    #[test]
    fn test_resolve_by_selector() {
        let selector = sel("0x12345678");
        let mut overrides = PricingOverrides::default();
        overrides.by_selector.insert(selector, entry("0.005 USD"));

        let any_addr = addr("0x0000000000000000000000000000000000000099");
        let result = overrides.resolve(&any_addr, &selector);
        assert_eq!(result.unwrap().to_string(), "0.005 USD");

        assert!(overrides.resolve(&any_addr, &sel("0x00000000")).is_none());
    }

    #[test]
    fn test_resolve_priority_requestor_selector_over_individual() {
        let requestor = addr("0x0000000000000000000000000000000000000001");
        let selector = sel("0x12345678");

        let mut overrides = PricingOverrides::default();
        overrides.by_requestor.insert(requestor, entry("0.001 USD"));
        overrides.by_selector.insert(selector, entry("0.002 USD"));
        overrides.by_requestor_selector.insert((requestor, selector), entry("0.003 USD"));

        let result = overrides.resolve(&requestor, &selector);
        assert_eq!(result.unwrap().to_string(), "0.003 USD");
    }

    #[test]
    fn test_resolve_selector_over_requestor() {
        let requestor = addr("0x0000000000000000000000000000000000000001");
        let selector = sel("0x12345678");

        let mut overrides = PricingOverrides::default();
        overrides.by_requestor.insert(requestor, entry("0.001 USD"));
        overrides.by_selector.insert(selector, entry("0.002 USD"));

        let result = overrides.resolve(&requestor, &selector);
        assert_eq!(result.unwrap().to_string(), "0.002 USD");
    }

    #[test]
    fn test_json_round_trip() {
        let json = r#"{
            "by_requestor": {
                "0x0000000000000000000000000000000000000001": { "min_mcycle_price": "0.001 USD" }
            },
            "by_selector": {
                "0x12345678": { "min_mcycle_price": "0.005 ETH" }
            },
            "by_requestor_selector": {
                "0x0000000000000000000000000000000000000001:0x12345678": { "min_mcycle_price": "0.01 USD" }
            }
        }"#;

        let overrides: PricingOverrides = serde_json::from_str(json).unwrap();

        assert_eq!(overrides.by_requestor.len(), 1);
        assert_eq!(overrides.by_selector.len(), 1);
        assert_eq!(overrides.by_requestor_selector.len(), 1);

        let requestor = addr("0x0000000000000000000000000000000000000001");
        let selector = sel("0x12345678");

        assert_eq!(overrides.resolve(&requestor, &selector).unwrap().to_string(), "0.01 USD");

        let other_sel = sel("0xdeadbeef");
        assert_eq!(overrides.resolve(&requestor, &other_sel).unwrap().to_string(), "0.001 USD");

        let other_addr = addr("0x0000000000000000000000000000000000000099");
        assert_eq!(overrides.resolve(&other_addr, &selector).unwrap().to_string(), "0.005 ETH");

        assert!(overrides.resolve(&other_addr, &other_sel).is_none());
    }

    #[test]
    fn test_json_serialize_deserialize_round_trip() {
        let requestor = addr("0x0000000000000000000000000000000000000001");
        let selector = sel("0x12345678");

        let mut overrides = PricingOverrides::default();
        overrides.by_requestor.insert(requestor, entry("0.001 USD"));
        overrides.by_selector.insert(selector, entry("0.005 ETH"));
        overrides.by_requestor_selector.insert((requestor, selector), entry("0.01 USD"));

        let json = serde_json::to_string(&overrides).unwrap();
        let deserialized: PricingOverrides = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.by_requestor.len(), 1);
        assert_eq!(deserialized.by_selector.len(), 1);
        assert_eq!(deserialized.by_requestor_selector.len(), 1);
        assert_eq!(deserialized.resolve(&requestor, &selector).unwrap().to_string(), "0.01 USD");
    }

    #[test]
    fn test_json_invalid_requestor_selector_key() {
        let json = r#"{
            "by_requestor_selector": {
                "bad-key-no-colon": { "min_mcycle_price": "0.001 USD" }
            }
        }"#;
        let result: Result<PricingOverrides, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_json_empty_maps() {
        let json = r#"{}"#;
        let overrides: PricingOverrides = serde_json::from_str(json).unwrap();
        assert!(overrides.by_requestor.is_empty());
        assert!(overrides.by_selector.is_empty());
        assert!(overrides.by_requestor_selector.is_empty());
    }

    #[test]
    fn test_market_config_default_has_no_overrides_path() {
        let config = MarketConfig::default();
        assert!(config.pricing_overrides_path.is_none());
    }
}
