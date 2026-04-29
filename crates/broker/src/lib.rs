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

use std::time::SystemTime;

use alloy::primitives::U256;
use boundless_market::contracts::ProofRequest;
use chrono::{DateTime, Utc};
pub use config::Config;
pub use config::ConfigLock;
use risc0_zkvm::sha::Digest;
pub use rpc_retry_policy::CustomRetryPolicy;
use serde::{Deserialize, Serialize};
pub(crate) use storage::ConfigurableDownloader;

pub(crate) mod aggregator;
pub mod args;
pub(crate) mod block_history;
mod broker;
pub(crate) mod chain_monitor;
pub(crate) mod chain_monitor_v2;
pub(crate) mod channels;
mod db;
pub use db::{broker_sqlite_url_for_chain, DbObj, SqliteDb};
pub(crate) mod market_monitor;
pub(crate) mod offchain_market_monitor;
pub(crate) mod order_committer;
pub(crate) mod order_evaluator;
pub(crate) mod order_locker;
pub(crate) mod order_pricer;
mod order_types;
mod price_oracle;
pub mod provers;
pub(crate) mod proving;
pub(crate) mod requestor_monitor;
pub(crate) mod service_runner;
pub(crate) mod shared;
pub use shared::config;
pub(crate) use shared::{errors, prioritization, task};
pub(crate) mod submitter;
pub(crate) mod telemetry;
pub(crate) mod utils;
pub use utils::{futures_retry, rpcmetrics, sequential_fallback};
pub(crate) use utils::{reaper, rpc_retry_policy, storage};
pub mod version_check;

pub use args::{build_chain_provider, ChainArgs, ChainPipeline, CoreArgs};
pub use broker::{resolve_deployment, Broker};

pub use boundless_market::prover_utils::{
    Erc1271GasCache, FulfillmentType, MarketConfig, OrderPricingContext, OrderPricingError,
    OrderPricingOutcome, OrderRequest, PreflightCache, PreflightCacheKey, PreflightCacheValue,
    ProveLimitReason,
};
pub(crate) use order_types::{proving_order_from_request, skipped_order_from_request};
pub use order_types::{CompressionType, Order, OrderStateChange, OrderStatus};

#[derive(sqlx::Type, Default, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum BatchStatus {
    #[default]
    Aggregating,
    PendingCompression,
    Complete,
    PendingSubmission,
    Submitted,
    Failed,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AggregationState {
    pub guest_state: risc0_aggregation::GuestState,
    /// All claim digests in this aggregation.
    /// This collection can be used to construct the aggregation Merkle tree and Merkle paths.
    pub claim_digests: Vec<Digest>,
    /// Proof ID for the STARK proof that compresses the root of the aggregation tree.
    pub proof_id: String,
    /// Proof ID for the Groth16 proof that compresses the root of the aggregation tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groth16_proof_id: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Batch {
    pub status: BatchStatus,
    /// Orders from the market that are included in this batch.
    pub orders: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assessor_proof_id: Option<String>,
    /// Tuple of the current aggregation state, as committed by the set builder guest, and the
    /// proof ID for the receipt that attests to the correctness of this state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregation_state: Option<AggregationState>,
    /// When the batch was initially created.
    pub start_time: DateTime<Utc>,
    /// The deadline for the batch, which is the earliest deadline for any order in the batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u64>,
    /// The total fees for the batch, which is the sum of fees from all orders.
    pub fees: U256,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
}

/// A very small utility function to get the current unix timestamp in seconds.
// TODO(#379): Avoid drift relative to the chain's timestamps.
pub(crate) fn now_timestamp() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
}

// Utility function to format the expiries of a request in a human readable format
pub(crate) fn format_expiries(request: &ProofRequest) -> String {
    let now: i64 = now_timestamp().try_into().unwrap();
    let lock_expires_at: i64 = request.lock_expires_at().try_into().unwrap();
    let lock_expires_delta = lock_expires_at - now;
    let lock_expires_delta_str = if lock_expires_delta > 0 {
        format!("{lock_expires_delta} seconds from now")
    } else {
        format!("{} seconds ago", lock_expires_delta.abs())
    };
    let expires_at: i64 = request.expires_at().try_into().unwrap();
    let expires_at_delta = expires_at - now;
    let expires_at_delta_str = if expires_at_delta > 0 {
        format!("{expires_at_delta} seconds from now")
    } else {
        format!("{} seconds ago", expires_at_delta.abs())
    };
    format!(
        "lock expires at {lock_expires_at} ({lock_expires_delta_str}), expires at {expires_at} ({expires_at_delta_str})"
    )
}

/// Returns `true` if the dev mode environment variable is enabled.
pub(crate) fn is_dev_mode() -> bool {
    std::env::var("RISC0_DEV_MODE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

#[cfg(feature = "test-utils")]
pub mod test_utils;

#[cfg(test)]
pub mod tests;
