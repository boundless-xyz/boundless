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

//! Capacity-completion message types exchanged between the OrderCommitter
//! and the per-chain OrderLocker / proving pipeline.

/// Reason a proving capacity slot was released. Sent via [`CommitmentComplete`]
/// from the OrderLocker (for `Skipped`) or proving pipeline components
/// (for `ProvingCompleted`/`ProvingFailed`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum CommitmentOutcome {
    /// Order failed validation or locking in the OrderLocker and never entered the
    /// proving pipeline. Capacity is freed immediately.
    Skipped,
    /// Order was proven, aggregated, and fulfilled on-chain by the Submitter.
    ProvingCompleted,
    /// Order failed somewhere in the proving pipeline (ProvingService, Aggregator,
    /// Submitter, or ReaperTask).
    ProvingFailed,
}

impl std::fmt::Display for CommitmentOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitmentOutcome::Skipped => write!(f, "Skipped"),
            CommitmentOutcome::ProvingCompleted => write!(f, "ProvingCompleted"),
            CommitmentOutcome::ProvingFailed => write!(f, "ProvingFailed"),
        }
    }
}

/// Capacity release signal sent back to the OrderCommitter to free an `in_flight` slot.
/// Produced by the OrderLocker (Skipped) and proving pipeline (ProvingCompleted/Failed).
pub(crate) struct CommitmentComplete {
    pub order_id: String,
    pub chain_id: u64,
    pub outcome: CommitmentOutcome,
}
