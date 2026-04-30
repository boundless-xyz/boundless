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

//! Plain data types stored in or returned from the broker database.
//!
//! [`AggregationOrder`] is the public projection used by the aggregator service.
//! [`DbOrder`] / [`DbBatch`] / [`DbLockedRequest`] are crate-private row shapes
//! used by the SQLite-backed implementation in `db/sqlite.rs`.

use alloy::primitives::U256;

use crate::{Batch, Order};

/// Struct containing the information about an order used by the aggregation worker.
#[derive(Clone, Debug)]
pub struct AggregationOrder {
    pub order_id: String,
    pub proof_id: String,
    pub expiration: u64,
    pub fee: U256,
}

#[derive(sqlx::FromRow)]
pub(crate) struct DbOrder {
    pub(crate) id: String,
    #[sqlx(json)]
    pub(crate) data: Order,
}

#[derive(sqlx::FromRow)]
pub(crate) struct DbBatch {
    pub(crate) id: i64,
    #[sqlx(json)]
    pub(crate) data: Batch,
}

#[derive(sqlx::FromRow)]
pub(crate) struct DbLockedRequest {
    #[allow(dead_code)]
    pub(crate) id: String,
    pub(crate) locker: String,
    pub(crate) block_number: u64,
}
