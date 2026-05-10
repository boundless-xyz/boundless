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

//! Public batch/aggregation types and a small internal helper used by the
//! aggregator service.

use alloy::primitives::U256;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use boundless_market::aggregator::AggregationState;

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

pub(crate) struct AggregateProofsResult {
    pub(crate) set_builder_proving_secs: Option<f64>,
    pub(crate) assessor_proving_secs: Option<f64>,
}
