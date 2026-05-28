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

//! Broker-facing backend boundary.
//!
//! The broker keeps DB state, retry policy, cancellation, and batch lifecycle
//! orchestration; backend implementations own zkVM-specific proof processing.

mod risc0;
mod router;
mod types;

pub use risc0::{prune_receipt_claim_journal, Risc0Backend};
pub use router::BackendRouter;
pub use types::{
    BackendBatchState, BackendEntry, BackendError, BackendId, BackendOrderState, BatchOrder,
    BatchSizeEstimateRequest, BatchUpdate, CloseBatch, FulfillmentBatch, FulfillmentOrder,
    OrderProcessProgress, ProcessOrder, ProcessedOrder, UpdateBatch, VerifierUpdate,
    VerifierUpdateError,
};

#[cfg(test)]
pub(crate) use types::BatchProcessorObj;
#[cfg(test)]
pub use types::{Backend, SubmissionPlan};
