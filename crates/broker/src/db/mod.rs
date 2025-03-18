// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

mod postgres;
mod sqlite;

use std::{default::Default, sync::Arc};

use alloy::primitives::{ruint::ParseError as RuintParseErr, B256, U256};
use async_trait::async_trait;
use risc0_zkvm::sha::Digest;
use sqlx::{
    postgres::PgPoolOptions,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Database, Pool,
};
use thiserror::Error;

use crate::{AggregationState, Batch, BatchStatus, Order, OrderStatus, ProofRequest};

#[allow(unused_imports)]
pub use postgres::PostgresDb;
#[allow(unused_imports)]
pub use sqlite::SqliteDb;

#[cfg(test)]
mod fuzz_db;

#[derive(Error, Debug)]
pub enum DbError {
    #[error("Order key {0} not found in DB")]
    OrderNotFound(U256),

    #[error("Batch key {0} not found in DB")]
    BatchNotFound(usize),

    #[error("Batch key {0} has no aggreagtion state")]
    BatchAggregationStateIsNone(usize),

    #[cfg(test)]
    #[error("Batch insert failed {0}")]
    BatchInsertFailure(usize),

    #[error("DB Missing column value: {0}")]
    MissingElm(&'static str),

    #[error("SQL error")]
    SqlErr(#[from] sqlx::Error),

    #[error("SQL Migration error")]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("JSON serialization err")]
    JsonErr(#[from] serde_json::Error),

    #[error("Invalid order id")]
    InvalidOrderId(#[from] RuintParseErr),

    #[error("Invalid block number: {0}")]
    BadBlockNumb(String),

    #[error("Failed to set last block")]
    SetBlockFail,

    #[error("Invalid order id: {0} missing field: {1}")]
    InvalidOrder(String, &'static str),

    #[error("Invalid max connection env var value")]
    MaxConnEnvVar(#[from] std::num::ParseIntError),
}

/// Struct containing the information about an order used by the aggregation worker.
#[derive(Clone, Debug)]
pub struct AggregationOrder {
    pub order_id: U256,
    pub proof_id: String,
    pub expiration: u64,
    pub fee: U256,
}

#[async_trait]
pub trait BrokerDb {
    async fn add_order(&self, id: U256, order: Order) -> Result<Option<Order>, DbError>;
    async fn order_exists(&self, id: U256) -> Result<bool, DbError>;
    async fn get_order(&self, id: U256) -> Result<Option<Order>, DbError>;
    async fn get_submission_order(
        &self,
        id: U256,
    ) -> Result<(ProofRequest, String, B256, U256), DbError>;
    async fn get_order_for_pricing(&self) -> Result<Option<(U256, Order)>, DbError>;
    async fn get_active_pricing_orders(&self) -> Result<Vec<(U256, Order)>, DbError>;
    async fn set_order_lock(
        &self,
        id: U256,
        lock_timestamp: u64,
        expire_timestamp: u64,
    ) -> Result<(), DbError>;
    async fn set_proving_status(&self, id: U256, lock_price: U256) -> Result<(), DbError>;
    async fn set_order_failure(&self, id: U256, failure_str: String) -> Result<(), DbError>;
    async fn set_order_complete(&self, id: U256) -> Result<(), DbError>;
    async fn skip_order(&self, id: U256) -> Result<(), DbError>;
    async fn get_last_block(&self) -> Result<Option<u64>, DbError>;
    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError>;
    async fn get_pending_lock_orders(
        &self,
        end_timestamp: u64,
    ) -> Result<Vec<(U256, Order)>, DbError>;
    async fn get_orders_committed_to_fulfill_count(&self) -> Result<u64, DbError>;
    async fn get_proving_order(&self) -> Result<Option<(U256, Order)>, DbError>;
    async fn get_active_proofs(&self) -> Result<Vec<(U256, Order)>, DbError>;
    async fn set_order_proof_id(&self, order_id: U256, proof_id: &str) -> Result<(), DbError>;
    async fn set_image_input_ids(
        &self,
        id: U256,
        image_id: &str,
        input_id: &str,
    ) -> Result<(), DbError>;
    async fn set_aggregation_status(&self, id: U256) -> Result<(), DbError>;
    async fn get_aggregation_proofs(&self) -> Result<Vec<AggregationOrder>, DbError>;
    async fn new_batch(&self) -> Result<usize, DbError>;
    async fn complete_batch(&self, batch_id: usize, g16_proof_id: String) -> Result<(), DbError>;
    async fn get_complete_batch(&self) -> Result<Option<(usize, Batch)>, DbError>;
    async fn set_batch_submitted(&self, batch_id: usize) -> Result<(), DbError>;
    async fn set_batch_failure(&self, batch_id: usize, err: String) -> Result<(), DbError>;
    async fn get_current_batch(&self) -> Result<usize, DbError>;

    /// Update a batch with the results of an aggregation step.
    ///
    /// Sets the aggregation state, and adds the given orders to the batch, updating the batch fees
    /// and deadline. During finalization, the assessor_claim_digest is recorded as well.
    async fn update_batch(
        &self,
        batch_id: usize,
        aggreagtion_state: &AggregationState,
        orders: &[AggregationOrder],
        assessor_claim_digest: Option<Digest>,
    ) -> Result<(), DbError>;
    async fn get_batch(&self, batch_id: usize) -> Result<Batch, DbError>;

    #[cfg(test)]
    async fn add_batch(&self, batch_id: usize, batch: Batch) -> Result<(), DbError>;
    #[cfg(test)]
    async fn set_batch_status(&self, batch_id: usize, status: BatchStatus) -> Result<(), DbError>;
}

pub type DbObj = Arc<dyn BrokerDb + Send + Sync>;

pub struct DbPool<D: Database> {
    pool: Pool<D>,
}

#[async_trait::async_trait]
pub trait DBPoolManager<D: Database> {
    async fn new(conn_str: &str) -> Result<DbPool<D>, DbError>;
}

#[derive(sqlx::FromRow)]
struct DbOrder {
    id: String,
    #[sqlx(json)]
    data: Order,
}

#[derive(sqlx::FromRow)]
struct DbBatch {
    id: i64,
    #[sqlx(json)]
    data: Batch,
}
