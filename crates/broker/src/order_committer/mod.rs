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

//! Singleton order committer — manages global proving capacity across all
//! chains and dispatches priced orders to per-chain OrderLockers.
//!
//! Layout:
//! - [`service`] — the [`OrderCommitter`] struct, its constructor, and the
//!   [`BrokerService`](crate::task::BrokerService) `run` loop, plus the
//!   [`CommitmentComplete`]-handling capacity-tracking logic.
//! - `state` — internal `InFlightOrder` and `CommitterConfig` state structs
//!   used by the run loop.
//! - `types` — [`CommitmentOutcome`] and [`CommitmentComplete`], the
//!   capacity-release messages exchanged with downstream services.
//! - `error` — `OrderCommitterErr`.

mod error;
mod service;
mod state;
mod types;

pub(crate) use service::OrderCommitter;
pub(crate) use types::{CommitmentComplete, CommitmentOutcome};
