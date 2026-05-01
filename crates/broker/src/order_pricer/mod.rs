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

//! Order pricing service — preflight every newly-discovered order and decide
//! whether to lock, fulfill after lock expiry, or skip.
//!
//! Layout:
//! - [`service`] — the [`OrderPricer`] struct, its constructor, and the
//!   [`BrokerService`](crate::task::BrokerService) `run` loop.
//! - `helpers` — pricing-state mutation and the
//!   [`OrderPricingContext`](crate::OrderPricingContext) trait implementation
//!   that backs the upstream `price_order` function.
//! - [`types`] — `ActivePreflights` cancellation tracker and the
//!   `OrderCache` dedup map.
//! - [`error`] — skip-code constants and the `CodedError` impl for
//!   `OrderPricingError`.
//! - `price_oracle` — adapter that runs
//!   [`PriceOracleManager`](boundless_market::price_oracle::PriceOracleManager)
//!   as a [`BrokerService`](crate::task::BrokerService).

mod error;
mod helpers;
mod price_oracle;
mod service;
mod types;

pub(crate) use service::OrderPricer;

#[cfg(test)]
pub(crate) use service::tests;
