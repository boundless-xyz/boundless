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

//! Constants and utilities for epoch-based aggregation.
//! The actual aggregation functions are in their respective modules:
//! - market.rs for market epoch aggregations
//! - provers.rs for prover epoch aggregations
//! - requestors.rs for requestor epoch aggregations

/// Number of epochs to recompute when running periodic aggregation
pub const EPOCH_AGGREGATION_RECOMPUTE_COUNT: u64 = 3;
