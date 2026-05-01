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

//! Off-chain market monitor — subscribes to the order-stream WebSocket and
//! turns incoming pre-signed orders into `OrderRequest`s for downstream
//! services.
//!
//! Layout:
//! - [`service`] — the [`OffchainMarketMonitor`] struct, its constructor,
//!   and the [`BrokerService`](crate::task::BrokerService) `run` loop.
//! - `error` — [`OffchainMarketMonitorErr`].

mod error;
mod service;

pub(crate) use service::OffchainMarketMonitor;
