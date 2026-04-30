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

//! Per-chain submitter — drives complete batches through the on-chain
//! `fulfillBatch` flow, recording results back into the DB and emitting
//! capacity completions to the OrderCommitter.
//!
//! Layout:
//! - [`service`] — the [`Submitter`] struct, its constructor, and the
//!   [`BrokerService`](crate::task::BrokerService) `run` loop.
//! - `error` — [`SubmitterErr`] enum.

mod error;
mod service;

pub use service::Submitter;
