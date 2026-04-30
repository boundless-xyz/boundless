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

//! Per-chain order locker — validates orders dispatched by the OrderCommitter,
//! locks `LockAndFulfill` orders on-chain, and inserts non-locking orders into
//! the per-chain DB for the proving pipeline.
//!
//! Layout:
//! - [`service`] — the [`OrderLocker`] struct, its constructor, and the
//!   [`BrokerService`](crate::task::BrokerService) `run` loop.
//! - `types` — [`OrderLockerConfig`], [`OrderCommitmentMeta`],
//!   [`RpcRetryConfig`], and the internal `CapacityResult` helper.
//! - [`error`] — [`OrderLockerErr`] enum.

mod error;
mod service;
mod types;

pub(crate) use service::OrderLocker;
pub(crate) use types::{OrderCommitmentMeta, OrderLockerConfig, RpcRetryConfig};

#[cfg(test)]
pub(crate) use service::tests;
