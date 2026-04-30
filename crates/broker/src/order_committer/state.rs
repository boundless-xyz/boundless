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

//! Internal state structs used by the OrderCommitter run loop.

use std::time::Instant;

use alloy::primitives::Address;

use crate::config::OrderCommitmentPriority;

pub(super) struct InFlightOrder {
    pub(super) dispatched_at: Instant,
    pub(super) total_cycles: Option<u64>,
    pub(super) proving_started_at: Option<u64>,
}

pub(super) struct CommitterConfig {
    pub(super) max_concurrent_proofs: usize,
    pub(super) peak_prove_khz: Option<u64>,
    pub(super) additional_proof_cycles: u64,
    pub(super) batch_buffer_time_secs: u64,
    pub(super) order_commitment_priority: OrderCommitmentPriority,
    pub(super) priority_addresses: Option<Vec<Address>>,
}
