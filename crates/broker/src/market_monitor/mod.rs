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

//! Per-chain market monitor — polls the BoundlessMarket contract for
//! `RequestSubmitted`, `RequestLocked`, and `RequestFulfilled` events and
//! turns them into orders / state-change broadcasts.
//!
//! Layout:
//! - [`service`] — the [`MarketMonitor`] struct, its constructor, the
//!   [`BrokerService`](crate::task::BrokerService) `run` loop, and the free
//!   `process_*` log-decoding helpers shared with `chain_monitor_v2`.
//! - `types` — [`MarketEvent`], the decoded contract-event enum.
//! - `error` — [`MarketMonitorErr`].

mod error;
mod service;
mod types;

pub(crate) use service::MarketMonitor;
pub(crate) use service::{
    process_log, process_new_logs, process_order_submitted, process_request_fulfilled,
    process_request_locked,
};
pub(crate) use types::MarketEvent;
