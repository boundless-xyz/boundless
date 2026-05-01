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

//! Per-chain proving service — picks committed orders out of the DB and
//! drives them through the prover backend (Bento or Bonsai), recording the
//! resulting proof IDs and emitting capacity completions back to the
//! OrderCommitter.
//!
//! Layout:
//! - [`service`] — the [`ProvingService`] struct, its constructor, and the
//!   [`BrokerService`](crate::task::BrokerService) `run` loop.
//! - `error` — [`ProvingErr`] enum and its `completion_outcome` mapping to
//!   `boundless_market::telemetry::CompletionOutcome`.

mod error;
mod service;

pub(crate) use service::ProvingService;
