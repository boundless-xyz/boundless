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

//! Configuration and metadata types for the order locker service.

use std::collections::HashMap;
use std::sync::Arc;

use crate::OrderRequest;

/// Metadata attached to each order for telemetry when recording commitment decisions.
pub(crate) struct OrderCommitmentMeta {
    pub(crate) estimated_proving_time_secs: Option<u64>,
    pub(crate) estimated_proving_time_no_load_secs: Option<u64>,
    pub(crate) concurrent_proving_jobs: u32,
}

pub(super) struct CapacityResult {
    pub(super) orders: Vec<Arc<OrderRequest>>,
    pub(super) meta: HashMap<String, OrderCommitmentMeta>,
}

#[derive(Default)]
pub(crate) struct OrderLockerConfig {
    pub(crate) min_deadline: u64,
    pub(crate) peak_prove_khz: Option<u64>,
    pub(crate) max_concurrent_proofs: Option<u32>,
    pub(crate) additional_proof_cycles: u64,
}

#[derive(Clone)]
pub struct RpcRetryConfig {
    pub retry_count: u64,
    pub retry_sleep_ms: u64,
}
