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

//! Persistent broker state — the [`BrokerDb`] trait, its public types, and the
//! SQLite implementation.
//!
//! The trait abstracts over the storage backend so services depend on
//! [`DbObj`] rather than the concrete implementation. Today the only impl is
//! [`SqliteDb`] in `db/sqlite.rs`.

use std::sync::Arc;

use alloy::primitives::{Bytes, U256};
use async_trait::async_trait;

#[cfg(test)]
use crate::BatchStatus;
use crate::{
    backend::{BackendBatchState, BackendId, BackendOrderState},
    Batch, FulfillmentType, Order, OrderRequest, OrderStatus, ProofRequest,
};

mod error;
#[cfg(test)]
mod fuzz_db;
mod sqlite;
mod types;

pub use error::DbError;
pub use sqlite::{broker_sqlite_url_for_chain, SqliteDb};
pub use types::BatchReadyOrder;

#[async_trait]
pub trait BrokerDb {
    async fn insert_accepted_request(
        &self,
        order_request: &OrderRequest,
        lock_price: U256,
    ) -> Result<Order, DbError>;
    async fn get_order(&self, id: &str) -> Result<Option<Order>, DbError>;
    async fn get_orders(&self, ids: &[&str]) -> Result<Vec<Order>, DbError>;
    async fn get_submission_order(
        &self,
        id: &str,
    ) -> Result<(ProofRequest, Bytes, String, U256, FulfillmentType), DbError>;
    async fn set_order_failure(&self, id: &str, failure_str: &str) -> Result<(), DbError>;
    async fn set_order_complete(&self, id: &str) -> Result<(), DbError>;
    /// Get all orders that are committed to be prove and be fulfilled.
    async fn get_committed_orders(&self) -> Result<Vec<Order>, DbError>;
    /// Get all orders that are committed to be proved but have expired based on their expire_timestamp.
    async fn get_expired_committed_orders(
        &self,
        grace_period_secs: i64,
    ) -> Result<Vec<Order>, DbError>;
    async fn get_proving_order(&self) -> Result<Option<Order>, DbError>;
    async fn get_active_proofs(&self) -> Result<Vec<Order>, DbError>;
    async fn set_order_backend_state(
        &self,
        order_id: &str,
        state: &BackendOrderState,
    ) -> Result<(), DbError>;
    async fn set_order_batch_status(
        &self,
        id: &str,
        status: OrderStatus,
        backend_id: &BackendId,
    ) -> Result<(), DbError>;
    async fn get_pending_batch_orders(
        &self,
        backend_id: &BackendId,
    ) -> Result<Vec<BatchReadyOrder>, DbError>;
    async fn get_pending_direct_submission_orders(
        &self,
        backend_id: &BackendId,
    ) -> Result<Vec<BatchReadyOrder>, DbError>;
    async fn complete_batch(
        &self,
        batch_id: usize,
        backend_state: &BackendBatchState,
    ) -> Result<(), DbError>;
    async fn get_complete_batch(&self) -> Result<Option<(usize, Batch)>, DbError>;
    async fn set_batch_submitted(&self, batch_id: usize) -> Result<(), DbError>;
    async fn set_batch_failure(&self, batch_id: usize, err: String) -> Result<(), DbError>;
    async fn get_current_batch(&self, backend_id: &BackendId) -> Result<usize, DbError>;
    async fn set_request_fulfilled(
        &self,
        request_id: U256,
        block_number: u64,
    ) -> Result<(), DbError>;
    // Checks the fulfillment table for the given request_id
    async fn is_request_fulfilled(&self, request_id: U256) -> Result<bool, DbError>;
    async fn set_request_locked(
        &self,
        request_id: U256,
        locker: &str,
        block_number: u64,
    ) -> Result<(), DbError>;
    // Checks the locked table for the given request_id
    async fn is_request_locked(&self, request_id: U256) -> Result<bool, DbError>;
    // Checks the locked table for the given request_id
    async fn get_request_locked(&self, request_id: U256) -> Result<Option<(String, u64)>, DbError>;
    /// Update a broker batch with backend state and newly claimed orders.
    ///
    /// Sets the backend state, and adds the given orders to the batch, updating the batch fees
    /// and deadline. When `finalize` is true, the batch transitions to `PendingCompression`.
    async fn update_batch(
        &self,
        batch_id: usize,
        backend_state: &BackendBatchState,
        orders: &[BatchReadyOrder],
        finalize: bool,
    ) -> Result<(), DbError>;
    async fn get_batch(&self, batch_id: usize) -> Result<Batch, DbError>;

    #[cfg(test)]
    async fn add_order(&self, order: &Order) -> Result<(), DbError>;
    #[cfg(test)]
    async fn add_batch(&self, batch_id: usize, batch: Batch) -> Result<(), DbError>;
    #[cfg(test)]
    async fn set_batch_status(&self, batch_id: usize, status: BatchStatus) -> Result<(), DbError>;
    #[cfg(test)]
    async fn execute_raw(&self, sql: &str) -> Result<(), DbError>;
}

pub type DbObj = Arc<dyn BrokerDb + Send + Sync>;
