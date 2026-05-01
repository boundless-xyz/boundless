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

//! Chain monitor — fetches the current chain head and gas price for downstream
//! services (`OrderPricer`, `OrderLocker`, `MarketMonitor`, etc.) via the
//! shared [`ChainMonitorApi`] trait.
//!
//! [`ChainMonitorV2`] is the default implementation — one polling loop per
//! chain that drives `eth_getBlockReceipts` and updates head atomics.
//! [`ChainMonitorService`] is the legacy V1 implementation, selected via
//! `--legacy-rpc` and slated for removal.
//!
//! Layout:
//! - [`service`] — [`ChainMonitorV2`] + its `BrokerService` `run` loop.
//! - [`legacy`] — [`ChainMonitorService`] (V1) + [`ChainMonitorErr`].
//! - [`types`] — shared [`ChainHead`], [`ChainMonitorApi`], [`ChainMonitorObj`].
//! - [`block_history`] — sliding window of recent blocks for V2's gas
//!   estimation.
//! - [`error`] — [`ChainMonitorV2Err`].

mod block_history;
mod error;
mod legacy;
mod service;
mod types;

pub(crate) use legacy::ChainMonitorService;
pub(crate) use service::ChainMonitorV2;
pub(crate) use types::{ChainHead, ChainMonitorObj};
