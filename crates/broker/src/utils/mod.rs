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

//! Cross-service utilities: retry helpers, RPC layers, storage helpers,
//! the reaper task, and small misc helpers (gas estimation, timestamps,
//! and expiry formatting).

mod helpers;
pub(crate) mod reaper;
pub(crate) mod rpc_retry_policy;
pub mod rpcmetrics;
pub mod sequential_fallback;
pub(crate) mod storage;
// Re-export the standalone helpers at the `crate::utils` root so callers
// keep using `utils::...` without a redundant intermediate path component.
pub(crate) use helpers::{
    estimate_gas_to_fulfill, estimate_gas_to_lock, format_expiries, is_dev_mode, now_timestamp,
};
// `prune_receipt_claim_journal` is defined in the RISC0 backend and is used
// by `boundless-cli` to compute claim digests.
pub use crate::backend::prune_receipt_claim_journal;
