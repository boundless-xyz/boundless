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

//! Aggregator service — bundles fulfillments into a Merkle batch, runs the
//! set-builder + assessor guests, and writes the resulting batch state to the
//! database for later submission.
//!
//! Layout:
//! - [`service`] — the [`AggregatorService`] struct, its constructor, and the
//!   [`BrokerService`](crate::task::BrokerService) `run` loop.
//! - [`types`] — public batch types ([`Batch`], [`BatchStatus`],
//!   [`AggregationState`]) and the internal `AggregateProofsResult`.
//! - [`error`] — [`AggregatorErr`] enum.

mod error;
mod service;
mod types;

pub use service::AggregatorService;
pub use types::{AggregationState, Batch, BatchStatus};
