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

//! Preflight-completion message types exchanged between the OrderEvaluator
//! and the per-chain OrderPricer.

use alloy::primitives::U256;

#[derive(Debug, Clone, Copy)]
pub(crate) enum PreflightOutcome {
    Priced,
    Skipped,
    #[allow(dead_code)]
    Failed,
    Cancelled,
}

impl std::fmt::Display for PreflightOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreflightOutcome::Priced => write!(f, "Priced"),
            PreflightOutcome::Skipped => write!(f, "Skipped"),
            PreflightOutcome::Failed => write!(f, "Failed"),
            PreflightOutcome::Cancelled => write!(f, "Cancelled"),
        }
    }
}

pub(crate) struct PreflightComplete {
    pub order_id: String,
    #[allow(dead_code)]
    pub request_id: U256,
    pub chain_id: u64,
    pub outcome: PreflightOutcome,
}
