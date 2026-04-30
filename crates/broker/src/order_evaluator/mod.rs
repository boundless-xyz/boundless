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

//! Singleton order evaluator — receives newly discovered orders, applies
//! preflight-capacity constraints, and dispatches them to the per-chain
//! OrderPricer.
//!
//! Layout:
//! - [`service`] — the [`OrderEvaluator`] struct and the
//!   [`BrokerService`](crate::task::BrokerService) `run` loop.
//! - `types` — [`PreflightOutcome`] and [`PreflightComplete`], the
//!   capacity-release messages exchanged with the per-chain OrderPricer.
//! - `error` — [`OrderEvaluatorErr`].

mod error;
mod service;
mod types;

pub(crate) use service::OrderEvaluator;
pub(crate) use types::{PreflightComplete, PreflightOutcome};
