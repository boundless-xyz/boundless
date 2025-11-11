// Copyright 2025 RISC Zero, Inc.
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

use std::{str::FromStr, sync::Arc};

use super::DbError;
use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;
use boundless_market::contracts::{
    AssessorReceipt, Fulfillment, FulfillmentDataType, Predicate, PredicateType, ProofRequest,
    RequestInputType,
};
use sqlx::{
    any::{install_default_drivers, AnyConnectOptions, AnyPoolOptions},
    AnyPool, Row,
};

const SQL_BLOCK_KEY: i64 = 0;

// Batch insert chunk size for request statuses
// Setting too high may result in hitting parameter limits for the db engine.
const REQUEST_STATUS_BATCH_SIZE: usize = 200;

// Batch insert chunk sizes for various table inserts
// Conservative sizes to support both PostgreSQL (32,767 params) and SQLite (999 params)
const TX_BATCH_SIZE: usize = 500; // 5 params per row = 2,500 params max
const REQUEST_SUBMITTED_EVENT_BATCH_SIZE: usize = 1000; // 5 params per row = 5,000 params max
const REQUEST_LOCKED_EVENT_BATCH_SIZE: usize = 800; // 6 params per row = 4,800 params max
const PROOF_DELIVERED_EVENT_BATCH_SIZE: usize = 800; // 6 params per row = 4,800 params max
const REQUEST_FULFILLED_EVENT_BATCH_SIZE: usize = 800; // 6 params per row = 4,800 params max
const PROOF_REQUEST_BATCH_SIZE: usize = 1000; // 23 params per row = 23,000 params max

#[derive(Debug, Clone, Copy)]
pub enum SortDirection {
    /// Ascending order (oldest first)
    Asc,
    /// Descending order (newest first)
    Desc,
}

#[derive(Debug, Clone, Copy)]
pub enum RequestSortField {
    /// Sort by updated_at timestamp
    UpdatedAt,
    /// Sort by created_at timestamp
    CreatedAt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestStatusType {
    Submitted,
    Locked,
    Fulfilled,
    Expired,
}

impl std::fmt::Display for RequestStatusType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestStatusType::Submitted => write!(f, "submitted"),
            RequestStatusType::Locked => write!(f, "locked"),
            RequestStatusType::Fulfilled => write!(f, "fulfilled"),
            RequestStatusType::Expired => write!(f, "expired"),
        }
    }
}

impl FromStr for RequestStatusType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "submitted" => Ok(RequestStatusType::Submitted),
            "locked" => Ok(RequestStatusType::Locked),
            "fulfilled" => Ok(RequestStatusType::Fulfilled),
            "expired" => Ok(RequestStatusType::Expired),
            _ => Err(format!("Invalid request status: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashedStatus {
    NotApplicable,
    Pending,
    Slashed,
}

impl std::fmt::Display for SlashedStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlashedStatus::NotApplicable => write!(f, "N/A"),
            SlashedStatus::Pending => write!(f, "pending"),
            SlashedStatus::Slashed => write!(f, "slashed"),
        }
    }
}

impl FromStr for SlashedStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "N/A" => Ok(SlashedStatus::NotApplicable),
            "pending" => Ok(SlashedStatus::Pending),
            "slashed" => Ok(SlashedStatus::Slashed),
            _ => Err(format!("Invalid slashed status: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TxMetadata {
    pub tx_hash: B256,
    pub from: Address,
    pub block_number: u64,
    pub block_timestamp: u64,
    pub transaction_index: u64,
}

#[derive(Debug, Clone)]
pub struct HourlyMarketSummary {
    pub period_timestamp: u64,
    pub total_fulfilled: u64,
    pub unique_provers_locking_requests: u64,
    pub unique_requesters_submitting_requests: u64,
    pub total_fees_locked: String,
    pub total_collateral_locked: String,
    pub p10_fees_locked: String,
    pub p25_fees_locked: String,
    pub p50_fees_locked: String,
    pub p75_fees_locked: String,
    pub p90_fees_locked: String,
    pub p95_fees_locked: String,
    pub p99_fees_locked: String,
    pub total_requests_submitted: u64,
    pub total_requests_submitted_onchain: u64,
    pub total_requests_submitted_offchain: u64,
    pub total_requests_locked: u64,
    pub total_requests_slashed: u64,
    pub total_expired: u64,
    pub total_locked_and_expired: u64,
    pub total_locked_and_fulfilled: u64,
    pub locked_orders_fulfillment_rate: f32,
    pub total_cycles: u64,
    pub best_peak_prove_mhz: u64,
    pub best_peak_prove_mhz_prover: Option<String>,
    pub best_peak_prove_mhz_request_id: Option<String>,
    pub best_effective_prove_mhz: u64,
    pub best_effective_prove_mhz_prover: Option<String>,
    pub best_effective_prove_mhz_request_id: Option<String>,
}

// Type aliases for different aggregation periods - they all use the same struct
pub type DailyMarketSummary = HourlyMarketSummary;
pub type WeeklyMarketSummary = HourlyMarketSummary;
pub type MonthlyMarketSummary = HourlyMarketSummary;

#[derive(Debug, Clone)]
pub struct RequestStatus {
    pub request_digest: B256,
    pub request_id: String,
    pub request_status: RequestStatusType,
    pub slashed_status: SlashedStatus,
    pub source: String,
    pub client_address: Address,
    pub lock_prover_address: Option<Address>,
    pub fulfill_prover_address: Option<Address>,
    pub created_at: u64,
    pub updated_at: u64,
    pub locked_at: Option<u64>,
    pub fulfilled_at: Option<u64>,
    pub slashed_at: Option<u64>,
    pub submit_block: Option<u64>,
    pub lock_block: Option<u64>,
    pub fulfill_block: Option<u64>,
    pub slashed_block: Option<u64>,
    pub min_price: String,
    pub max_price: String,
    pub lock_collateral: String,
    pub ramp_up_start: u64,
    pub ramp_up_period: u64,
    pub expires_at: u64,
    pub lock_end: u64,
    pub slash_recipient: Option<Address>,
    pub slash_transferred_amount: Option<String>,
    pub slash_burned_amount: Option<String>,
    pub cycles: Option<u64>,
    pub peak_prove_mhz: Option<u64>,
    pub effective_prove_mhz: Option<u64>,
    pub submit_tx_hash: Option<B256>,
    pub lock_tx_hash: Option<B256>,
    pub fulfill_tx_hash: Option<B256>,
    pub slash_tx_hash: Option<B256>,
    pub image_id: String,
    pub image_url: Option<String>,
    pub selector: String,
    pub predicate_type: String,
    pub predicate_data: String,
    pub input_type: String,
    pub input_data: String,
    pub fulfill_journal: Option<String>,
    pub fulfill_seal: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestCursor {
    pub timestamp: u64, // Either updated_at or created_at depending on sort_by
    pub request_digest: String,
}

// Raw data fetched from database for status computation
#[derive(Debug, Clone)]
pub struct RequestComprehensive {
    pub request_digest: B256,
    pub request_id: String,
    pub source: String,
    pub client_address: Address,
    pub created_at: u64,
    pub submit_block: Option<u64>,
    pub submit_tx_hash: Option<B256>,
    pub min_price: String,
    pub max_price: String,
    pub lock_collateral: String,
    pub ramp_up_start: u64,
    pub ramp_up_period: u64,
    pub expires_at: u64,
    pub lock_end: u64,
    pub image_id: String,
    pub image_url: Option<String>,
    pub selector: String,
    pub predicate_type: String,
    pub predicate_data: String,
    pub input_type: String,
    pub input_data: String,
    // Event data
    pub submitted_at: Option<u64>,
    pub locked_at: Option<u64>,
    pub lock_block: Option<u64>,
    pub lock_tx_hash: Option<B256>,
    pub lock_prover_address: Option<Address>,
    pub fulfilled_at: Option<u64>,
    pub fulfill_prover_address: Option<Address>,
    pub fulfill_block: Option<u64>,
    pub fulfill_tx_hash: Option<B256>,
    pub cycles: Option<u64>,
    pub peak_prove_mhz: Option<u64>,
    pub effective_prove_mhz: Option<u64>,
    pub fulfill_journal: Option<String>,
    pub fulfill_seal: Option<String>,
    pub slashed_at: Option<u64>,
    pub slashed_block: Option<u64>,
    pub slash_tx_hash: Option<B256>,
    pub slash_burned_amount: Option<String>,
    pub slash_transferred_amount: Option<String>,
    pub slash_recipient: Option<Address>,
}

#[derive(Debug, Clone)]
pub struct LockPricingData {
    pub min_price: String,
    pub max_price: String,
    pub bidding_start: u64,
    pub ramp_up_period: u32,
    pub lock_end: u64,
    pub lock_collateral: String,
    pub lock_timestamp: u64,
}

impl TxMetadata {
    pub fn new(
        tx_hash: B256,
        from: Address,
        block_number: u64,
        block_timestamp: u64,
        transaction_index: u64,
    ) -> Self {
        Self { tx_hash, from, block_number, block_timestamp, transaction_index }
    }
}

#[async_trait]
pub trait IndexerDb {
    fn pool(&self) -> &AnyPool;

    async fn get_last_block(&self) -> Result<Option<u64>, DbError>;
    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError>;

    async fn add_block(&self, block_numb: u64, block_timestamp: u64) -> Result<(), DbError>;
    async fn get_block_timestamp(&self, block_numb: u64) -> Result<Option<u64>, DbError>;

    async fn add_tx(&self, metadata: &TxMetadata) -> Result<(), DbError>;

    /// Batch insert transactions
    async fn add_txs_batch(&self, metadata_list: &[TxMetadata]) -> Result<(), DbError>;

    async fn add_proof_request(
        &self,
        request_digest: B256,
        request: ProofRequest,
        metadata: &TxMetadata,
        source: &str,
    ) -> Result<(), DbError>;

    /// Batch insert proof requests
    async fn add_proof_requests_batch(
        &self,
        requests: &[(B256, ProofRequest, TxMetadata, String)],
    ) -> Result<(), DbError>;

    async fn has_proof_request(&self, request_digest: B256) -> Result<bool, DbError>;

    async fn get_request_digests_by_request_id(
        &self,
        request_id: U256,
    ) -> Result<Vec<B256>, DbError>;

    async fn add_assessor_receipt(
        &self,
        receipt: AssessorReceipt,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_proof(
        &self,
        fill: Fulfillment,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_request_submitted_event(
        &self,
        request_digest: B256,
        request_id: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    /// Batch insert request submitted events
    async fn add_request_submitted_events_batch(
        &self,
        events: &[(B256, U256, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_request_locked_event(
        &self,
        request_digest: B256,
        request_id: U256,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    /// Batch insert request locked events
    async fn add_request_locked_events_batch(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_proof_delivered_event(
        &self,
        request_digest: B256,
        request_id: U256,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    /// Batch insert proof delivered events
    async fn add_proof_delivered_events_batch(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_request_fulfilled_event(
        &self,
        request_digest: B256,
        request_id: U256,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    /// Batch insert request fulfilled events
    async fn add_request_fulfilled_events_batch(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_prover_slashed_event(
        &self,
        request_id: U256,
        burn_value: U256,
        transfer_value: U256,
        collateral_recipient: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_withdrawal_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_collateral_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_collateral_withdrawal_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_callback_failed_event(
        &self,
        request_id: U256,
        callback_address: Address,
        error_data: Vec<u8>,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn get_last_order_stream_timestamp(
        &self,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, DbError>;

    async fn set_last_order_stream_timestamp(
        &self,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DbError>;

    async fn upsert_hourly_market_summary(
        &self,
        summary: HourlyMarketSummary,
    ) -> Result<(), DbError>;

    async fn get_hourly_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<HourlyMarketSummary>, DbError>;

    async fn upsert_daily_market_summary(&self, summary: DailyMarketSummary)
        -> Result<(), DbError>;

    async fn get_daily_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<DailyMarketSummary>, DbError>;

    async fn upsert_weekly_market_summary(
        &self,
        summary: WeeklyMarketSummary,
    ) -> Result<(), DbError>;

    async fn get_weekly_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<WeeklyMarketSummary>, DbError>;

    async fn upsert_monthly_market_summary(
        &self,
        summary: MonthlyMarketSummary,
    ) -> Result<(), DbError>;

    async fn get_monthly_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<MonthlyMarketSummary>, DbError>;

    /// Upserts request statuses.
    /// Note on conflict, this function will not update all fields.
    /// Only the mutable fields e.g. locked_at, fulfilled_at, slashed_at, etc. will be updated.
    /// Things like image id, offer details, etc. will not be updated.
    async fn upsert_request_statuses(&self, statuses: &[RequestStatus]) -> Result<(), DbError>;

    // Joins multiple tables to get a comprehensive view of a request.
    async fn get_requests_comprehensive(
        &self,
        request_digests: &std::collections::HashSet<B256>,
    ) -> Result<Vec<RequestComprehensive>, DbError>;

    async fn find_newly_expired_requests(
        &self,
        from_block_timestamp: u64,
        to_block_timestamp: u64,
    ) -> Result<std::collections::HashSet<B256>, DbError>;

    async fn list_requests(
        &self,
        cursor: Option<RequestCursor>,
        limit: u32,
        sort_by: RequestSortField,
    ) -> Result<(Vec<RequestStatus>, Option<RequestCursor>), DbError>;

    async fn list_requests_by_requestor(
        &self,
        client_address: Address,
        cursor: Option<RequestCursor>,
        limit: u32,
        sort_by: RequestSortField,
    ) -> Result<(Vec<RequestStatus>, Option<RequestCursor>), DbError>;

    async fn list_requests_by_prover(
        &self,
        prover_address: Address,
        cursor: Option<RequestCursor>,
        limit: u32,
        sort_by: RequestSortField,
    ) -> Result<(Vec<RequestStatus>, Option<RequestCursor>), DbError>;

    async fn get_requests_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<RequestStatus>, DbError>;

    /// Gets the count of fulfillment events in the half-open period [period_start, period_end).
    /// Filters by `request_fulfilled_events.block_timestamp` (when the fulfillment event occurred on-chain).
    async fn get_period_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets the count of unique prover addresses that locked requests in the half-open period [period_start, period_end).
    /// Filters by `request_locked_events.block_timestamp` (when the lock event occurred on-chain).
    async fn get_period_unique_provers(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets the count of unique requester addresses that submitted requests in the half-open period [period_start, period_end).
    /// Filters by `proof_requests.block_timestamp` (when the request was created/submitted).
    async fn get_period_unique_requesters(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets the total count of requests submitted (both on-chain and off-chain) in the half-open period [period_start, period_end).
    /// Filters by `proof_requests.block_timestamp` (when the request was created/submitted).
    async fn get_period_total_requests_submitted(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets the count of on-chain request submission events in the half-open period [period_start, period_end).
    /// Filters by `request_submitted_events.block_timestamp` (when the submission event occurred on-chain).
    async fn get_period_total_requests_submitted_onchain(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets the count of request lock events in the half-open period [period_start, period_end).
    /// Filters by `request_locked_events.block_timestamp` (when the lock event occurred on-chain).
    async fn get_period_total_requests_locked(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets the count of prover slash events in the half-open period [period_start, period_end).
    /// Filters by `prover_slashed_events.block_timestamp` (when the slash event occurred on-chain).
    async fn get_period_total_requests_slashed(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets pricing data for requests fulfilled in the half-open period [period_start, period_end).
    /// Only includes requests where the prover who locked the request is the same as the prover who fulfilled it.
    /// Filters by `request_status.fulfilled_at` timestamp.
    async fn get_period_lock_pricing_data(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<LockPricingData>, DbError>;

    /// Gets collateral amounts for all locked requests in the half-open period [period_start, period_end).
    /// Filters by `request_locked_events.block_timestamp` (when the lock event occurred on-chain).
    /// Used for total_collateral_locked metric which tracks all locks regardless of fulfillment.
    async fn get_period_all_lock_collateral(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<String>, DbError>;

    /// Gets the count of requests that expired during the half-open period [period_start, period_end).
    /// Filters by `request_status.expires_at` (when the request's deadline passed).
    /// Note: These requests may have been submitted in an earlier period.
    async fn get_period_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets the count of locked requests that expired during the half-open period [period_start, period_end).
    /// Filters by `request_status.expires_at` (when the request's deadline passed).
    /// Note: These requests may have been locked in an earlier period.
    async fn get_period_locked_and_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets the count of locked requests that were fulfilled during the half-open period [period_start, period_end).
    /// Filters by `request_status.fulfilled_at` (when the fulfillment occurred).
    /// Note: These requests may have been locked in an earlier period.
    async fn get_period_locked_and_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError>;

    /// Gets request digests using cursor-based pagination.
    /// Returns digests greater than the cursor, ordered by digest value.
    /// Used for backfilling request statuses.
    async fn get_request_digests_paginated(
        &self,
        cursor: Option<B256>,
        limit: i64,
    ) -> Result<Vec<B256>, DbError>;
}

pub type DbObj = Arc<dyn IndexerDb + Send + Sync>;

#[derive(Debug, Clone)]
pub struct AnyDb {
    pub pool: AnyPool,
}

impl AnyDb {
    /// For SQLite use a `sqlite:file_path` URL; for Postgres `postgres://`.
    pub async fn new(conn_str: &str) -> Result<Self, DbError> {
        install_default_drivers(); 
        let opts = AnyConnectOptions::from_str(conn_str)?;

        let pool = AnyPoolOptions::new().max_connections(7).connect_with(opts).await?;

        // apply any migrations
        sqlx::migrate!().run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }
}

#[async_trait]
impl IndexerDb for AnyDb {
    fn pool(&self) -> &AnyPool {
        &self.pool
    }

    async fn get_last_block(&self) -> Result<Option<u64>, DbError> {
        let res = sqlx::query("SELECT block FROM last_block WHERE id = $1")
            .bind(SQL_BLOCK_KEY)
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = res else {
            return Ok(None);
        };

        let block_str: String = row.try_get("block")?;

        Ok(Some(block_str.parse().map_err(|_err| DbError::BadBlockNumb(block_str))?))
    }

    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError> {
        let res = sqlx::query(
            "INSERT INTO last_block (id, block) VALUES ($1, $2)
         ON CONFLICT (id) DO UPDATE SET block = EXCLUDED.block",
        )
        .bind(SQL_BLOCK_KEY)
        .bind(block_numb.to_string())
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::SetBlockFail);
        }

        Ok(())
    }

    async fn add_block(&self, block_numb: u64, block_timestamp: u64) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO blocks (block_number, block_timestamp) VALUES ($1, $2)
         ON CONFLICT (block_number) DO NOTHING",
        )
        .bind(block_numb as i64)
        .bind(block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_block_timestamp(&self, block_numb: u64) -> Result<Option<u64>, DbError> {
        let result = sqlx::query("SELECT block_timestamp FROM blocks WHERE block_number = $1")
            .bind(block_numb as i64)
            .fetch_optional(&self.pool)
            .await?;

        if let Some(row) = result {
            let block_timestamp: i64 = row.get(0);
            Ok(Some(block_timestamp as u64))
        } else {
            Ok(None)
        }
    }

    async fn add_tx(&self, metadata: &TxMetadata) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO transactions (
                tx_hash, 
                block_number, 
                from_address, 
                block_timestamp,
                transaction_index
            ) VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(format!("{:x}", metadata.from))
        .bind(metadata.block_timestamp as i64)
        .bind(metadata.transaction_index as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_txs_batch(&self, metadata_list: &[TxMetadata]) -> Result<(), DbError> {
        if metadata_list.is_empty() {
            return Ok(());
        }

        // Start a transaction
        let mut tx = self.pool.begin().await?;

        // Process in chunks
        for chunk in metadata_list.chunks(TX_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut query = String::from(
                "INSERT INTO transactions (
                    tx_hash,
                    block_number,
                    from_address,
                    block_timestamp,
                    transaction_index
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for metadata in chunk {
                query_builder = query_builder
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(format!("{:x}", metadata.from))
                    .bind(metadata.block_timestamp as i64)
                    .bind(metadata.transaction_index as i64);
            }

            query_builder.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn has_proof_request(&self, request_digest: B256) -> Result<bool, DbError> {
        let result = sqlx::query("SELECT 1 FROM proof_requests WHERE request_digest = $1")
            .bind(format!("{request_digest:x}"))
            .fetch_optional(&self.pool)
            .await?;

        Ok(result.is_some())
    }

    async fn get_request_digests_by_request_id(
        &self,
        request_id: U256,
    ) -> Result<Vec<B256>, DbError> {
        let request_id_str = format!("{request_id:x}");
        let rows = sqlx::query("SELECT request_digest FROM proof_requests WHERE request_id = $1")
            .bind(&request_id_str)
            .fetch_all(&self.pool)
            .await?;

        let mut digests = Vec::new();
        for row in rows {
            let digest_str: String = row.try_get("request_digest")?;
            let digest = B256::from_str(&digest_str)
                .map_err(|e| DbError::BadTransaction(format!("Invalid request_digest: {}", e)))?;
            digests.push(digest);
        }

        Ok(digests)
    }

    async fn add_proof_request(
        &self,
        request_digest: B256,
        request: ProofRequest,
        metadata: &TxMetadata,
        source: &str,
    ) -> Result<(), DbError> {
        tracing::debug!("add_proof_request called for digest: 0x{:x}", request_digest);
        self.add_tx(metadata).await?;
        let predicate_type = match request.requirements.predicate.predicateType {
            PredicateType::DigestMatch => "DigestMatch",
            PredicateType::PrefixMatch => "PrefixMatch",
            PredicateType::ClaimDigestMatch => "ClaimDigestMatch",
            _ => return Err(DbError::BadTransaction("Invalid predicate type".to_string())),
        };
        let input_type = match request.input.inputType {
            RequestInputType::Inline => "Inline",
            RequestInputType::Url => "Url",
            _ => return Err(DbError::BadTransaction("Invalid input type".to_string())),
        };

        let image_id_str = match Predicate::try_from(request.requirements.predicate.clone()) {
            Ok(predicate) => predicate
                .image_id()
                .map(|digest| format!("{:x}", B256::from(<[u8; 32]>::from(digest))))
                .unwrap_or_default(),
            Err(_) => String::new(),
        };

        tracing::debug!("Executing INSERT for proof_request digest: 0x{:x}", request_digest);
        sqlx::query(
            "INSERT INTO proof_requests (
                request_digest,
                request_id,
                client_address,
                predicate_type,
                predicate_data,
                callback_address,
                callback_gas_limit,
                selector,
                input_type,
                input_data,
                min_price,
                max_price,
                lock_collateral,
                bidding_start,
                expires_at,
                lock_end,
                ramp_up_period,
                tx_hash,
                block_number,
                block_timestamp,
                source,
                image_id,
                image_url
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)
            ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{:x}", request.id))
        .bind(format!("{:x}", request.client_address()))
        .bind(predicate_type)
        .bind(format!("{:x}", request.requirements.predicate.data))
        .bind(format!("{:x}", request.requirements.callback.addr))
        .bind(request.requirements.callback.gasLimit.to_string())
        .bind(format!("{:x}", request.requirements.selector))
        .bind(input_type)
        .bind(format!("{:x}", request.input.data))
        .bind(request.offer.minPrice.to_string())
        .bind(request.offer.maxPrice.to_string())
        .bind(request.offer.lockCollateral.to_string())
        .bind(request.offer.rampUpStart as i64)
        .bind((request.offer.rampUpStart + request.offer.timeout as u64)  as i64)
        .bind((request.offer.rampUpStart + request.offer.lockTimeout as u64)  as i64)
        .bind(request.offer.rampUpPeriod as i64)
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .bind(source)
        .bind(&image_id_str)
        .bind(&request.imageUrl)
        .execute(&self.pool)
        .await?;
        tracing::debug!("INSERT completed successfully for digest: 0x{:x}", request_digest);
        Ok(())
    }

    async fn add_proof_requests_batch(
        &self,
        requests: &[(B256, ProofRequest, TxMetadata, String)],
    ) -> Result<(), DbError> {
        if requests.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = requests
            .iter()
            .map(|(_, _, metadata, _)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs_batch(&unique_txs).await?;

        // Then batch insert proof requests in chunks
        for chunk in requests.chunks(PROOF_REQUEST_BATCH_SIZE) {
            let mut query_builder = sqlx::QueryBuilder::new(
                "INSERT INTO proof_requests (
                    request_digest,
                    request_id,
                    client_address,
                    predicate_type,
                    predicate_data,
                    callback_address,
                    callback_gas_limit,
                    selector,
                    input_type,
                    input_data,
                    min_price,
                    max_price,
                    lock_collateral,
                    bidding_start,
                    expires_at,
                    lock_end,
                    ramp_up_period,
                    tx_hash,
                    block_number,
                    block_timestamp,
                    source,
                    image_id,
                    image_url
                ) ",
            );

            query_builder.push_values(chunk, |mut b, (request_digest, request, metadata, source)| {
                // Extract predicate type string
                let predicate_type = match request.requirements.predicate.predicateType {
                    PredicateType::DigestMatch => "DigestMatch",
                    PredicateType::PrefixMatch => "PrefixMatch",
                    PredicateType::ClaimDigestMatch => "ClaimDigestMatch",
                    _ => {
                        // This is a bit tricky in batch - we can't return an error mid-batch
                        // For now, we'll use "Invalid" and let the DB constraint handle it
                        "Invalid"
                    }
                };

                // Extract input type string
                let input_type = match request.input.inputType {
                    RequestInputType::Inline => "Inline",
                    RequestInputType::Url => "Url",
                    _ => "Invalid",
                };

                // Extract image_id from predicate
                let image_id_str = match Predicate::try_from(request.requirements.predicate.clone()) {
                    Ok(predicate) => predicate
                        .image_id()
                        .map(|digest| format!("{:x}", B256::from(<[u8; 32]>::from(digest))))
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                };

                b.push_bind(format!("{request_digest:x}"))
                    .push_bind(format!("{:x}", request.id))
                    .push_bind(format!("{:x}", request.client_address()))
                    .push_bind(predicate_type)
                    .push_bind(format!("{:x}", request.requirements.predicate.data))
                    .push_bind(format!("{:x}", request.requirements.callback.addr))
                    .push_bind(request.requirements.callback.gasLimit.to_string())
                    .push_bind(format!("{:x}", request.requirements.selector))
                    .push_bind(input_type)
                    .push_bind(format!("{:x}", request.input.data))
                    .push_bind(request.offer.minPrice.to_string())
                    .push_bind(request.offer.maxPrice.to_string())
                    .push_bind(request.offer.lockCollateral.to_string())
                    .push_bind(request.offer.rampUpStart as i64)
                    .push_bind((request.offer.rampUpStart + request.offer.timeout as u64) as i64)
                    .push_bind((request.offer.rampUpStart + request.offer.lockTimeout as u64) as i64)
                    .push_bind(request.offer.rampUpPeriod as i64)
                    .push_bind(format!("{:x}", metadata.tx_hash))
                    .push_bind(metadata.block_number as i64)
                    .push_bind(metadata.block_timestamp as i64)
                    .push_bind(source.as_str())
                    .push_bind(image_id_str)
                    .push_bind(&request.imageUrl);
            });

            query_builder.push(" ON CONFLICT (request_digest) DO NOTHING");

            let query = query_builder.build();
            query.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn add_assessor_receipt(
        &self,
        receipt: AssessorReceipt,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO assessor_receipts (
                tx_hash,
                prover_address,
                seal,
                block_number,
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(format!("{:x}", receipt.prover))
        .bind(format!("{:x}", receipt.seal))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_proof(
        &self,
        fill: Fulfillment,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        let fulfillment_data_type: &'static str = match fill.fulfillmentDataType {
            FulfillmentDataType::ImageIdAndJournal => "ImageIdAndJournal",
            FulfillmentDataType::None => "None",
            _ => return Err(DbError::BadTransaction("Invalid fulfillment data type".to_string())),
        };
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO proofs (
                request_digest,
                request_id,
                prover_address,
                claim_digest,
                fulfillment_data_type,
                fulfillment_data,
                seal,
                tx_hash,
                block_number,
                block_timestamp,
                transaction_index
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (request_digest, tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", fill.requestDigest))
        .bind(format!("{:x}", fill.id))
        .bind(format!("{prover_address:x}"))
        .bind(format!("{:x}", fill.claimDigest))
        .bind(fulfillment_data_type)
        .bind(format!("{:x}", fill.fulfillmentData))
        .bind(format!("{:x}", fill.seal))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .bind(metadata.transaction_index as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_request_submitted_event(
        &self,
        request_digest: B256,
        request_id: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO request_submitted_events (
                request_digest,
                request_id, 
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{request_id:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_request_submitted_events_batch(
        &self,
        events: &[(B256, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // Start a transaction
        let mut tx = self.pool.begin().await?;

        // First, batch insert all unique transactions
        let unique_txs: Vec<TxMetadata> = {
            let mut seen = std::collections::HashSet::new();
            events.iter()
                .filter_map(|(_, _, metadata)| {
                    if seen.insert(metadata.tx_hash) {
                        Some(*metadata)
                    } else {
                        None
                    }
                })
                .collect()
        };

        // Use the add_txs_batch function we just created
        self.add_txs_batch(&unique_txs).await?;

        // Now batch insert the events
        for chunk in events.chunks(REQUEST_SUBMITTED_EVENT_BATCH_SIZE) {
            let mut query_builder = sqlx::QueryBuilder::new(
                "INSERT INTO request_submitted_events (
                    request_digest,
                    request_id,
                    tx_hash,
                    block_number,
                    block_timestamp
                ) ",
            );

            query_builder.push_values(chunk, |mut b, (request_digest, request_id, metadata)| {
                b.push_bind(format!("{request_digest:x}"))
                    .push_bind(format!("{request_id:x}"))
                    .push_bind(format!("{:x}", metadata.tx_hash))
                    .push_bind(metadata.block_number as i64)
                    .push_bind(metadata.block_timestamp as i64);
            });

            query_builder.push(" ON CONFLICT (request_digest) DO NOTHING");

            let query = query_builder.build();
            query.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn add_request_locked_event(
        &self,
        request_digest: B256,
        request_id: U256,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO request_locked_events (
                request_digest,
                request_id, 
                prover_address,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{request_id:x}"))
        .bind(format!("{prover_address:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_request_locked_events_batch(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // Start a transaction
        let mut tx = self.pool.begin().await?;

        // First, batch insert all unique transactions
        let unique_txs: Vec<TxMetadata> = {
            let mut seen = std::collections::HashSet::new();
            events.iter()
                .filter_map(|(_, _, _, metadata)| {
                    if seen.insert(metadata.tx_hash) {
                        Some(*metadata)
                    } else {
                        None
                    }
                })
                .collect()
        };

        // Use the add_txs_batch function
        self.add_txs_batch(&unique_txs).await?;

        // Now batch insert the events
        for chunk in events.chunks(REQUEST_LOCKED_EVENT_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut query = String::from(
                "INSERT INTO request_locked_events (
                    request_digest,
                    request_id,
                    prover_address,
                    tx_hash,
                    block_number,
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5,
                    params_count + 6
                ));
                params_count += 6;
            }
            query.push_str(" ON CONFLICT (request_digest) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_digest, request_id, prover_address, metadata) in chunk {
                // Use the exact same formatting as the individual insert
                query_builder = query_builder
                    .bind(format!("{request_digest:x}"))
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{prover_address:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn add_proof_delivered_events_batch(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs_batch(&unique_txs).await?;

        // Then batch insert proof delivered events in chunks
        for chunk in events.chunks(PROOF_DELIVERED_EVENT_BATCH_SIZE) {
            let mut query_builder = sqlx::QueryBuilder::new(
                "INSERT INTO proof_delivered_events (
                    request_digest,
                    request_id,
                    prover_address,
                    tx_hash,
                    block_number,
                    block_timestamp
                ) ",
            );

            query_builder.push_values(chunk, |mut b, (request_digest, request_id, prover_address, metadata)| {
                b.push_bind(format!("{request_digest:x}"))
                    .push_bind(format!("{request_id:x}"))
                    .push_bind(format!("{prover_address:x}"))
                    .push_bind(format!("{:x}", metadata.tx_hash))
                    .push_bind(metadata.block_number as i64)
                    .push_bind(metadata.block_timestamp as i64);
            });

            query_builder.push(" ON CONFLICT (request_digest, tx_hash) DO NOTHING");

            let query = query_builder.build();
            query.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn add_request_fulfilled_events_batch(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs_batch(&unique_txs).await?;

        // Then batch insert request fulfilled events in chunks
        for chunk in events.chunks(REQUEST_FULFILLED_EVENT_BATCH_SIZE) {
            let mut query_builder = sqlx::QueryBuilder::new(
                "INSERT INTO request_fulfilled_events (
                    request_digest,
                    request_id,
                    prover_address,
                    tx_hash,
                    block_number,
                    block_timestamp
                ) ",
            );

            query_builder.push_values(chunk, |mut b, (request_digest, request_id, prover_address, metadata)| {
                b.push_bind(format!("{request_digest:x}"))
                    .push_bind(format!("{request_id:x}"))
                    .push_bind(format!("{prover_address:x}"))
                    .push_bind(format!("{:x}", metadata.tx_hash))
                    .push_bind(metadata.block_number as i64)
                    .push_bind(metadata.block_timestamp as i64);
            });

            query_builder.push(" ON CONFLICT (request_digest) DO NOTHING");

            let query = query_builder.build();
            query.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn add_proof_delivered_event(
        &self,
        request_digest: B256,
        request_id: U256,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO proof_delivered_events (
                request_digest,
                request_id,
                prover_address,
                tx_hash,
                block_number,
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (request_digest, tx_hash) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{request_id:x}"))
        .bind(format!("{prover_address:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_request_fulfilled_event(
        &self,
        request_digest: B256,
        request_id: U256,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO request_fulfilled_events (
                request_digest,
                request_id,
                prover_address,
                tx_hash,
                block_number,
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{request_id:x}"))
        .bind(format!("{prover_address:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_prover_slashed_event(
        &self,
        request_id: U256,
        burn_value: U256,
        transfer_value: U256,
        collateral_recipient: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        let result =
            sqlx::query("SELECT prover_address FROM request_locked_events WHERE request_id = $1")
                .bind(format!("{request_id:x}"))
                .fetch_optional(&self.pool)
                .await?;
        // TODO: Improve this
        // If for some reason due to a gap in the db that is missing the associated locked request event,
        // we set the prover address to zero.
        let prover_address =
            result.map(|row| row.try_get("prover_address")).transpose()?.unwrap_or_else(|| {
                tracing::warn!(
                    "Missing request locked event for slashed event for request id: {:x}",
                    request_id
                );
                format!("{:x}", Address::ZERO)
            });
        sqlx::query(
            "INSERT INTO prover_slashed_events (
                request_id, 
                prover_address,
                burn_value,
                transfer_value,
                collateral_recipient,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (request_id) DO NOTHING",
        )
        .bind(format!("{request_id:x}"))
        .bind(prover_address)
        .bind(burn_value.to_string())
        .bind(transfer_value.to_string())
        .bind(format!("{collateral_recipient:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO deposit_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{account:x}"))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_withdrawal_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO withdrawal_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{account:x}"))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_collateral_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO collateral_deposit_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{account:x}"))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_collateral_withdrawal_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO collateral_withdrawal_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{account:x}"))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_callback_failed_event(
        &self,
        request_id: U256,
        callback_address: Address,
        error_data: Vec<u8>,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO callback_failed_events (
                request_id,
                callback_address,
                error_data,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (request_id, tx_hash) DO NOTHING",
        )
        .bind(format!("{request_id:x}"))
        .bind(format!("{callback_address:x}"))
        .bind(error_data)
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_last_order_stream_timestamp(
        &self,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, DbError> {
        let res =
            sqlx::query("SELECT last_processed_timestamp FROM order_stream_state WHERE id = TRUE")
                .fetch_optional(&self.pool)
                .await?;

        let Some(row) = res else {
            return Ok(None);
        };

        let timestamp_str: Option<String> = row.try_get("last_processed_timestamp")?;

        let timestamp = timestamp_str
            .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
            .transpose()
            .map_err(|e| DbError::BadTransaction(format!("Failed to parse timestamp: {}", e)))?;

        Ok(timestamp)
    }

    async fn set_last_order_stream_timestamp(
        &self,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DbError> {
        let timestamp_str = timestamp.to_rfc3339();

        sqlx::query("UPDATE order_stream_state SET last_processed_timestamp = $1 WHERE id = TRUE")
            .bind(timestamp_str)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn upsert_hourly_market_summary(
        &self,
        summary: HourlyMarketSummary,
    ) -> Result<(), DbError> {
        self.upsert_market_summary_generic(summary, "hourly_market_summary").await
    }

    async fn get_hourly_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<HourlyMarketSummary>, DbError> {
        self.get_market_summaries_generic(
            cursor,
            limit,
            sort,
            before,
            after,
            "hourly_market_summary",
        )
        .await
    }

    async fn upsert_daily_market_summary(
        &self,
        summary: DailyMarketSummary,
    ) -> Result<(), DbError> {
        self.upsert_market_summary_generic(summary, "daily_market_summary").await
    }

    async fn get_daily_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<DailyMarketSummary>, DbError> {
        self.get_market_summaries_generic(
            cursor,
            limit,
            sort,
            before,
            after,
            "daily_market_summary",
        )
        .await
    }

    async fn upsert_weekly_market_summary(
        &self,
        summary: WeeklyMarketSummary,
    ) -> Result<(), DbError> {
        self.upsert_market_summary_generic(summary, "weekly_market_summary").await
    }

    async fn get_weekly_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<WeeklyMarketSummary>, DbError> {
        self.get_market_summaries_generic(
            cursor,
            limit,
            sort,
            before,
            after,
            "weekly_market_summary",
        )
        .await
    }

    async fn upsert_monthly_market_summary(
        &self,
        summary: MonthlyMarketSummary,
    ) -> Result<(), DbError> {
        self.upsert_market_summary_generic(summary, "monthly_market_summary").await
    }

    async fn get_monthly_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<MonthlyMarketSummary>, DbError> {
        self.get_market_summaries_generic(
            cursor,
            limit,
            sort,
            before,
            after,
            "monthly_market_summary",
        )
        .await
    }

    async fn upsert_request_statuses(&self, statuses: &[RequestStatus]) -> Result<(), DbError> {
        if statuses.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in statuses.chunks(REQUEST_STATUS_BATCH_SIZE) {
            let mut query_builder = sqlx::QueryBuilder::new(
                "INSERT INTO request_status (
                    request_digest, request_id, request_status, slashed_status, source, client_address, lock_prover_address, fulfill_prover_address,
                    created_at, updated_at, locked_at, fulfilled_at, slashed_at,
                    submit_block, lock_block, fulfill_block, slashed_block,
                    min_price, max_price, lock_collateral, ramp_up_start, ramp_up_period, expires_at, lock_end,
                    slash_recipient, slash_transferred_amount, slash_burned_amount,
                    cycles, peak_prove_mhz, effective_prove_mhz,
                    submit_tx_hash, lock_tx_hash, fulfill_tx_hash, slash_tx_hash,
                    image_id, image_url, selector, predicate_type, predicate_data, input_type, input_data,
                    fulfill_journal, fulfill_seal
                ) ",
            );

            query_builder.push_values(chunk, |mut b, status| {
                b.push_bind(status.request_digest.to_string())
                    .push_bind(&status.request_id)
                    .push_bind(status.request_status.to_string())
                    .push_bind(status.slashed_status.to_string())
                    .push_bind(&status.source)
                    .push_bind(status.client_address.to_string())
                    .push_bind(status.lock_prover_address.map(|a| a.to_string()))
                    .push_bind(status.fulfill_prover_address.map(|a| a.to_string()))
                    .push_bind(status.created_at as i64)
                    .push_bind(status.updated_at as i64)
                    .push_bind(status.locked_at.map(|t| t as i64))
                    .push_bind(status.fulfilled_at.map(|t| t as i64))
                    .push_bind(status.slashed_at.map(|t| t as i64))
                    .push_bind(status.submit_block.map(|b| b as i64))
                    .push_bind(status.lock_block.map(|b| b as i64))
                    .push_bind(status.fulfill_block.map(|b| b as i64))
                    .push_bind(status.slashed_block.map(|b| b as i64))
                    .push_bind(&status.min_price)
                    .push_bind(&status.max_price)
                    .push_bind(&status.lock_collateral)
                    .push_bind(status.ramp_up_start as i64)
                    .push_bind(status.ramp_up_period as i64)
                    .push_bind(status.expires_at as i64)
                    .push_bind(status.lock_end as i64)
                    .push_bind(status.slash_recipient.map(|a| a.to_string()))
                    .push_bind(&status.slash_transferred_amount)
                    .push_bind(&status.slash_burned_amount)
                    .push_bind(status.cycles.map(|c| c as i64))
                    .push_bind(status.peak_prove_mhz.map(|m| m as i64))
                    .push_bind(status.effective_prove_mhz.map(|m| m as i64))
                    .push_bind(status.submit_tx_hash.map(|h| h.to_string()))
                    .push_bind(status.lock_tx_hash.map(|h| h.to_string()))
                    .push_bind(status.fulfill_tx_hash.map(|h| h.to_string()))
                    .push_bind(status.slash_tx_hash.map(|h| h.to_string()))
                    .push_bind(&status.image_id)
                    .push_bind(&status.image_url)
                    .push_bind(&status.selector)
                    .push_bind(&status.predicate_type)
                    .push_bind(&status.predicate_data)
                    .push_bind(&status.input_type)
                    .push_bind(&status.input_data)
                    .push_bind(&status.fulfill_journal)
                    .push_bind(&status.fulfill_seal);
            });

            query_builder.push(
                " ON CONFLICT (request_digest) DO UPDATE SET
                    request_status = EXCLUDED.request_status,
                    slashed_status = EXCLUDED.slashed_status,
                    lock_prover_address = EXCLUDED.lock_prover_address,
                    fulfill_prover_address = EXCLUDED.fulfill_prover_address,
                    updated_at = EXCLUDED.updated_at,
                    locked_at = EXCLUDED.locked_at,
                    fulfilled_at = EXCLUDED.fulfilled_at,
                    slashed_at = EXCLUDED.slashed_at,
                    lock_block = EXCLUDED.lock_block,
                    fulfill_block = EXCLUDED.fulfill_block,
                    slashed_block = EXCLUDED.slashed_block,
                    lock_tx_hash = EXCLUDED.lock_tx_hash,
                    fulfill_tx_hash = EXCLUDED.fulfill_tx_hash,
                    slash_tx_hash = EXCLUDED.slash_tx_hash,
                    slash_recipient = EXCLUDED.slash_recipient,
                    slash_transferred_amount = EXCLUDED.slash_transferred_amount,
                    slash_burned_amount = EXCLUDED.slash_burned_amount,
                    cycles = EXCLUDED.cycles,
                    peak_prove_mhz = EXCLUDED.peak_prove_mhz,
                    effective_prove_mhz = EXCLUDED.effective_prove_mhz,
                    fulfill_journal = EXCLUDED.fulfill_journal,
                    fulfill_seal = EXCLUDED.fulfill_seal"
            );

            let query = query_builder.build();
            query.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_requests_comprehensive(
        &self,
        request_digests: &std::collections::HashSet<B256>,
    ) -> Result<Vec<RequestComprehensive>, DbError> {
        if request_digests.is_empty() {
            return Ok(Vec::new());
        }

        let mut requests = Vec::new();

        // Process requests individually since ANY array syntax doesn't work with AnyPool
        for digest in request_digests {
            let digest_str = format!("{:x}", digest);

            tracing::trace!("Querying proof_requests for digest: {}", digest_str);

            let rows = sqlx::query(
                "SELECT
                    pr.request_digest,
                    pr.request_id,
                    pr.source,
                    pr.client_address,
                    pr.block_timestamp as created_at,
                    pr.block_number as submit_block,
                    pr.tx_hash as submit_tx_hash,
                    pr.min_price,
                    pr.max_price,
                    pr.lock_collateral,
                    pr.bidding_start as ramp_up_start,
                    pr.ramp_up_period,
                    pr.expires_at,
                    pr.lock_end,
                    pr.image_id,
                    pr.image_url,
                    pr.selector,
                    pr.predicate_type,
                    pr.predicate_data,
                    pr.input_type,
                    pr.input_data,
                    rse.block_timestamp as submitted_at,
                    rle.block_timestamp as locked_at,
                    rle.block_number as lock_block,
                    rle.tx_hash as lock_tx_hash,
                    rle.prover_address as lock_prover_address,
                    rfe.block_timestamp as fulfilled_at,
                    rfe.block_number as fulfill_block,
                    rfe.tx_hash as fulfill_tx_hash,
                    rfe.prover_address as fulfill_prover_address,
                    pse.block_timestamp as slashed_at,
                    pse.block_number as slashed_block,
                    pse.tx_hash as slash_tx_hash,
                    pse.burn_value as slash_burned_amount,
                    pse.transfer_value as slash_transferred_amount,
                    pse.collateral_recipient as slash_recipient
                FROM proof_requests pr
                LEFT JOIN request_submitted_events rse ON rse.request_digest = pr.request_digest
                LEFT JOIN request_locked_events rle ON rle.request_digest = pr.request_digest
                LEFT JOIN request_fulfilled_events rfe ON rfe.request_digest = pr.request_digest
                LEFT JOIN prover_slashed_events pse ON pse.request_id = pr.request_id
                WHERE pr.request_digest = $1",
            )
            .bind(&digest_str)
            .fetch_all(&self.pool)
            .await?;

            tracing::trace!("Query returned {} rows for digest: {}", rows.len(), digest_str);

            if rows.is_empty() {
                tracing::warn!("No proof_request found for digest: {}", digest_str);
            }

            // Should be at most one row since request_digest is unique. If there are multiple, we parse all so that we can
            // log and warn for further investigation.
            let mut cur_request = Vec::new();
            for row in rows {
                let request_digest_str: String = row.get("request_digest");
                let request_digest = B256::from_str(&request_digest_str).map_err(|e| {
                    DbError::BadTransaction(format!("Invalid request_digest: {}", e))
                })?;
                let request_id: String = row.get("request_id");
                let source: String = row.get("source");
                let client_address_str: String = row.get("client_address");
                let client_address = Address::from_str(&client_address_str).map_err(|e| {
                    DbError::BadTransaction(format!("Invalid client_address: {}", e))
                })?;

                let created_at: i64 = row.get("created_at");
                let submit_block: Option<i64> = row.try_get("submit_block").ok();
                let submit_tx_hash_str: Option<String> = row.try_get("submit_tx_hash").ok();
                let submit_tx_hash = submit_tx_hash_str.and_then(|s| B256::from_str(&s).ok());

                let min_price: String = row.get("min_price");
                let max_price: String = row.get("max_price");
                let lock_collateral: String = row.get("lock_collateral");
                let ramp_up_start: i64 = row.get("ramp_up_start");
                let ramp_up_period: i64 = row.get("ramp_up_period");
                let expires_at: i64 = row.get("expires_at");
                let lock_end: i64 = row.get("lock_end");

                let image_id: String = row.get("image_id");
                let image_url: Option<String> = row.try_get("image_url").ok();
                let selector: String = row.get("selector");
                let predicate_type: String = row.get("predicate_type");
                let predicate_data: String = row.get("predicate_data");
                let input_type: String = row.get("input_type");
                let input_data: String = row.get("input_data");

                // Event timestamps
                let submitted_at: Option<i64> = row.try_get("submitted_at").ok();
                let locked_at: Option<i64> = row.try_get("locked_at").ok();
                let lock_block: Option<i64> = row.try_get("lock_block").ok();
                let lock_tx_hash_str: Option<String> = row.try_get("lock_tx_hash").ok();
                let lock_tx_hash = lock_tx_hash_str.and_then(|s| B256::from_str(&s).ok());

                let lock_prover_address_str: Option<String> =
                    row.try_get("lock_prover_address").ok();
                let lock_prover_address =
                    lock_prover_address_str.and_then(|s| Address::from_str(&s).ok());

                let fulfilled_at: Option<i64> = row.try_get("fulfilled_at").ok();
                let fulfill_prover_address_str: Option<String> =
                    row.try_get("fulfill_prover_address").ok();
                let fulfill_prover_address =
                    fulfill_prover_address_str.and_then(|s| Address::from_str(&s).ok());
                let fulfill_block: Option<i64> = row.try_get("fulfill_block").ok();
                let fulfill_tx_hash_str: Option<String> = row.try_get("fulfill_tx_hash").ok();
                let fulfill_tx_hash = fulfill_tx_hash_str.and_then(|s| B256::from_str(&s).ok());

                let slashed_at: Option<i64> = row.try_get("slashed_at").ok();
                let slashed_block: Option<i64> = row.try_get("slashed_block").ok();
                let slash_tx_hash_str: Option<String> = row.try_get("slash_tx_hash").ok();
                let slash_tx_hash = slash_tx_hash_str.and_then(|s| B256::from_str(&s).ok());
                let slash_burned_amount_str: Option<String> =
                    row.try_get("slash_burned_amount").ok();
                let slash_transferred_amount_str: Option<String> =
                    row.try_get("slash_transferred_amount").ok();
                let slash_recipient_str: Option<String> = row.try_get("slash_recipient").ok();
                let slash_recipient = slash_recipient_str.and_then(|s| Address::from_str(&s).ok());

                cur_request.push(RequestComprehensive {
                    request_digest,
                    request_id,
                    source,
                    client_address,
                    created_at: created_at as u64,
                    submit_block: submit_block.map(|b| b as u64),
                    submit_tx_hash,
                    min_price,
                    max_price,
                    lock_collateral,
                    ramp_up_start: ramp_up_start as u64,
                    ramp_up_period: ramp_up_period as u64,
                    expires_at: expires_at as u64,
                    lock_end: lock_end as u64,
                    image_id,
                    image_url,
                    selector,
                    predicate_type,
                    predicate_data,
                    input_type,
                    input_data,
                    submitted_at: submitted_at.map(|t| t as u64),
                    locked_at: locked_at.map(|t| t as u64),
                    lock_block: lock_block.map(|b| b as u64),
                    lock_tx_hash,
                    lock_prover_address,
                    fulfilled_at: fulfilled_at.map(|t| t as u64),
                    fulfill_prover_address,
                    fulfill_block: fulfill_block.map(|b| b as u64),
                    fulfill_tx_hash,
                    cycles: None,              // TODO
                    peak_prove_mhz: None,      // TODO
                    effective_prove_mhz: None, // TODO
                    fulfill_journal: None,     // TODO
                    fulfill_seal: None,        // Will be populated from proofs table below
                    slashed_at: slashed_at.map(|t| t as u64),
                    slashed_block: slashed_block.map(|b| b as u64),
                    slash_tx_hash,
                    slash_burned_amount: slash_burned_amount_str,
                    slash_transferred_amount: slash_transferred_amount_str,
                    slash_recipient,
                });
            }

            if cur_request.len() > 1 {
                tracing::warn!("Multiple request_comprehensive computed found for digest {}. Adding first: {:?}", digest_str, cur_request);
            } 

            if !cur_request.is_empty() {
                requests.push(cur_request.first().unwrap().clone());
            }
        }

        // Now query proofs table to get seals for fulfilled requests
        // Build map of (request_digest, prover_address) -> seal
        let mut fulfillments_map: std::collections::HashMap<(B256, Address), String> =
            std::collections::HashMap::new();

        if !requests.is_empty() {
            // Collect all unique request_digests that have a fulfill_prover_address
            let digest_strs: Vec<String> = requests
                .iter()
                .filter_map(|req| {
                    req.fulfill_prover_address.map(|_| format!("{:x}", req.request_digest))
                })
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            // Query fulfillments in chunks of 500 to avoid parameter limits
            const CHUNK_SIZE: usize = 500;
            for chunk in digest_strs.chunks(CHUNK_SIZE) {
                // Build dynamic IN clause
                let placeholders: Vec<String> =
                    (1..=chunk.len()).map(|i| format!("${}", i)).collect();
                let query_str = format!(
                    "SELECT request_digest, prover_address, seal, block_timestamp
                     FROM proofs
                     WHERE request_digest IN ({})
                     ORDER BY request_digest, prover_address, block_timestamp ASC",
                    placeholders.join(", ")
                );

                let mut query = sqlx::query(&query_str);
                for digest_str in chunk {
                    query = query.bind(digest_str);
                }

                let rows = query.fetch_all(&self.pool).await?;

                // Build map: keep only first (earliest) seal for each (digest, prover) pair.
                // Provers can deliver proofs multiple times, so there may be multiple entries for the same (digest, prover) pair
                // in the proofs table.
                for row in rows {
                    let digest_str: String = row.get("request_digest");
                    let prover_str: String = row.get("prover_address");
                    let seal: String = row.get("seal");

                    let digest = B256::from_str(&digest_str)
                        .map_err(|e| DbError::BadTransaction(format!("Invalid digest: {}", e)))?;
                    let prover = Address::from_str(&prover_str)
                        .map_err(|e| DbError::BadTransaction(format!("Invalid address: {}", e)))?;

                    fulfillments_map.entry((digest, prover)).or_insert(seal); // First one wins
                }
            }

            // Fill in seals in RequestComprehensive objects
            for req in &mut requests {
                if let Some(prover_addr) = req.fulfill_prover_address {
                    let key = (req.request_digest, prover_addr);
                    if let Some(seal) = fulfillments_map.get(&key) {
                        req.fulfill_seal = Some(seal.clone());
                    }
                }
            }
        }

        Ok(requests)
    }

    /// Finds requests that expired within the half-open timestamp range [from_block_timestamp, to_block_timestamp).
    /// Note: to_block_timestamp is typically "now", so we use a half-open range to avoid including requests that 
    /// with timeout == now, as they are not expired yet.
    async fn find_newly_expired_requests(
        &self,
        from_block_timestamp: u64,
        to_block_timestamp: u64,
    ) -> Result<std::collections::HashSet<B256>, DbError> {
        let rows = sqlx::query(
            "SELECT request_digest
             FROM proof_requests pr
             WHERE pr.expires_at >= $1
               AND pr.expires_at < $2
               AND NOT EXISTS (
                   SELECT 1 FROM request_fulfilled_events rfe
                   WHERE rfe.request_digest = pr.request_digest
               )
               ",
        )
        .bind(from_block_timestamp as i64)
        .bind(to_block_timestamp as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut expired = std::collections::HashSet::new();
        for row in rows {
            let digest_str: String = row.get("request_digest");
            let digest = B256::from_str(&digest_str)
                .map_err(|e| DbError::BadTransaction(format!("Invalid request_digest: {}", e)))?;
            expired.insert(digest);
        }

        Ok(expired)
    }

    async fn list_requests(
        &self,
        cursor: Option<RequestCursor>,
        limit: u32,
        sort_by: RequestSortField,
    ) -> Result<(Vec<RequestStatus>, Option<RequestCursor>), DbError> {
        let sort_field = match sort_by {
            RequestSortField::UpdatedAt => "updated_at",
            RequestSortField::CreatedAt => "created_at",
        };

        let rows = if let Some(c) = &cursor {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE {} < $1 OR ({} = $1 AND request_digest < $2)
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $3",
                sort_field, sort_field, sort_field
            );
            sqlx::query(&query_str)
                .bind(c.timestamp as i64)
                .bind(&c.request_digest)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        } else {
            let query_str = format!(
                "SELECT * FROM request_status
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $1",
                sort_field
            );
            sqlx::query(&query_str).bind(limit as i64).fetch_all(&self.pool).await?
        };
        let mut results = Vec::new();

        for row in rows {
            results.push(self.row_to_request_status(&row)?);
        }

        let next_cursor = if results.len() == limit as usize {
            results.last().map(|r| {
                let timestamp = match sort_by {
                    RequestSortField::UpdatedAt => r.updated_at,
                    RequestSortField::CreatedAt => r.created_at,
                };
                RequestCursor { timestamp, request_digest: r.request_digest.to_string() }
            })
        } else {
            None
        };

        Ok((results, next_cursor))
    }

    async fn list_requests_by_requestor(
        &self,
        client_address: Address,
        cursor: Option<RequestCursor>,
        limit: u32,
        sort_by: RequestSortField,
    ) -> Result<(Vec<RequestStatus>, Option<RequestCursor>), DbError> {
        let client_str = client_address.to_string();
        let sort_field = match sort_by {
            RequestSortField::UpdatedAt => "updated_at",
            RequestSortField::CreatedAt => "created_at",
        };

        let rows = if let Some(c) = &cursor {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE client_address = $1
                   AND ({} < $2 OR ({} = $2 AND request_digest < $3))
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $4",
                sort_field, sort_field, sort_field
            );
            sqlx::query(&query_str)
                .bind(&client_str)
                .bind(c.timestamp as i64)
                .bind(&c.request_digest)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        } else {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE client_address = $1
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $2",
                sort_field
            );
            sqlx::query(&query_str)
                .bind(&client_str)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        };
        let mut results = Vec::new();

        for row in rows {
            results.push(self.row_to_request_status(&row)?);
        }

        let next_cursor = if results.len() == limit as usize {
            results.last().map(|r| {
                let timestamp = match sort_by {
                    RequestSortField::UpdatedAt => r.updated_at,
                    RequestSortField::CreatedAt => r.created_at,
                };
                RequestCursor { timestamp, request_digest: r.request_digest.to_string() }
            })
        } else {
            None
        };

        Ok((results, next_cursor))
    }

    async fn list_requests_by_prover(
        &self,
        prover_address: Address,
        cursor: Option<RequestCursor>,
        limit: u32,
        sort_by: RequestSortField,
    ) -> Result<(Vec<RequestStatus>, Option<RequestCursor>), DbError> {
        let prover_str = prover_address.to_string();
        let sort_field = match sort_by {
            RequestSortField::UpdatedAt => "updated_at",
            RequestSortField::CreatedAt => "created_at",
        };

        let rows = if let Some(c) = &cursor {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE prover_address = $1
                   AND ({} < $2 OR ({} = $2 AND request_digest < $3))
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $4",
                sort_field, sort_field, sort_field
            );
            sqlx::query(&query_str)
                .bind(&prover_str)
                .bind(c.timestamp as i64)
                .bind(&c.request_digest)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        } else {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE prover_address = $1
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $2",
                sort_field
            );
            sqlx::query(&query_str)
                .bind(&prover_str)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
        };
        let mut results = Vec::new();

        for row in rows {
            results.push(self.row_to_request_status(&row)?);
        }

        let next_cursor = if results.len() == limit as usize {
            results.last().map(|r| {
                let timestamp = match sort_by {
                    RequestSortField::UpdatedAt => r.updated_at,
                    RequestSortField::CreatedAt => r.created_at,
                };
                RequestCursor { timestamp, request_digest: r.request_digest.to_string() }
            })
        } else {
            None
        };

        Ok((results, next_cursor))
    }

    async fn get_requests_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<RequestStatus>, DbError> {
        let rows = sqlx::query(
            "SELECT * FROM request_status
             WHERE request_id = $1
             ORDER BY updated_at DESC",
        )
        .bind(request_id)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::new();
        for row in rows {
            results.push(self.row_to_request_status(&row)?);
        }

        Ok(results)
    }

    /// Gets the count of fulfillment events in the half-open period [period_start, period_end).
    /// Filters by `request_fulfilled_events.block_timestamp` (when the fulfillment event occurred on-chain).
    async fn get_period_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_fulfilled_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets the count of unique prover addresses that locked requests in the half-open period [period_start, period_end).
    /// Filters by `request_locked_events.block_timestamp` (when the lock event occurred on-chain).
    async fn get_period_unique_provers(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT prover_address) FROM request_locked_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets the count of unique requester addresses that submitted requests in the half-open period [period_start, period_end).
    /// Filters by `proof_requests.block_timestamp` (when the request was created/submitted).
    async fn get_period_unique_requesters(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT client_address) FROM proof_requests
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets the total count of requests submitted (both on-chain and off-chain) in the half-open period [period_start, period_end).
    /// Filters by `proof_requests.block_timestamp` (when the request was created/submitted).
    async fn get_period_total_requests_submitted(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM proof_requests
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets the count of on-chain request submission events in the half-open period [period_start, period_end).
    /// Filters by `request_submitted_events.block_timestamp` (when the submission event occurred on-chain).
    async fn get_period_total_requests_submitted_onchain(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_submitted_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets the count of request lock events in the half-open period [period_start, period_end).
    /// Filters by `request_locked_events.block_timestamp` (when the lock event occurred on-chain).
    async fn get_period_total_requests_locked(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_locked_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets the count of prover slash events in the half-open period [period_start, period_end).
    /// Filters by `prover_slashed_events.block_timestamp` (when the slash event occurred on-chain).
    async fn get_period_total_requests_slashed(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM prover_slashed_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets pricing data for requests fulfilled in the half-open period [period_start, period_end).
    /// Only includes requests where the prover who locked the request is the same as the prover who fulfilled it.
    /// Filters by `request_status.fulfilled_at` timestamp.
    async fn get_period_lock_pricing_data(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<LockPricingData>, DbError> {
        let rows = sqlx::query(
            "SELECT
                rs.min_price,
                rs.max_price,
                rs.ramp_up_start as bidding_start,
                rs.ramp_up_period,
                rs.lock_end,
                rs.lock_collateral,
                rs.locked_at as lock_timestamp
             FROM request_status rs
             WHERE rs.fulfilled_at >= $1
               AND rs.fulfilled_at < $2
               AND rs.request_status = 'fulfilled'
               AND rs.lock_prover_address = rs.fulfill_prover_address
               AND rs.lock_prover_address IS NOT NULL",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::new();
        for row in rows {
            let min_price: String = row.get("min_price");
            let max_price: String = row.get("max_price");
            let bidding_start: i64 = row.get("bidding_start");
            let ramp_up_period: i64 = row.get("ramp_up_period");
            let lock_end: i64 = row.get("lock_end");
            let lock_collateral: String = row.get("lock_collateral");
            let lock_timestamp: i64 = row.get("lock_timestamp");

            results.push(LockPricingData {
                min_price,
                max_price,
                bidding_start: bidding_start as u64,
                ramp_up_period: ramp_up_period as u32,
                lock_end: lock_end as u64,
                lock_collateral,
                lock_timestamp: lock_timestamp as u64,
            });
        }

        Ok(results)
    }

    /// Gets collateral amounts for all locked requests in the half-open period [period_start, period_end).
    /// Filters by `request_locked_events.block_timestamp` (when the lock event occurred on-chain).
    /// Used for total_collateral_locked metric which tracks all locks regardless of fulfillment.
    async fn get_period_all_lock_collateral(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<String>, DbError> {
        let rows = sqlx::query(
            "SELECT pr.lock_collateral
             FROM request_locked_events rle
             JOIN proof_requests pr ON rle.request_digest = pr.request_digest
             WHERE rle.block_timestamp >= $1 AND rle.block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::new();
        for row in rows {
            let lock_collateral: String = row.get("lock_collateral");
            results.push(lock_collateral);
        }

        Ok(results)
    }

    /// Gets the count of requests that expired during the half-open period [period_start, period_end).
    /// Filters by `request_status.expires_at` (when the request's deadline passed).
    /// Note: These requests may have been submitted in an earlier period.
    async fn get_period_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_status
             WHERE request_status = 'expired'
             AND expires_at >= $1 AND expires_at < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets the count of locked requests that expired during the half-open period [period_start, period_end).
    /// Filters by `request_status.expires_at` (when the request's deadline passed).
    /// Note: These requests may have been locked in an earlier period.
    async fn get_period_locked_and_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_status
             WHERE request_status = 'expired'
             AND locked_at IS NOT NULL
             AND expires_at >= $1 AND expires_at < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets the count of locked requests that were fulfilled during the half-open period [period_start, period_end).
    /// Filters by `request_status.fulfilled_at` (when the fulfillment occurred).
    /// Note: These requests may have been locked in an earlier period.
    async fn get_period_locked_and_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_status
             WHERE request_status = 'fulfilled'
             AND locked_at IS NOT NULL
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $1 AND fulfilled_at < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    async fn get_request_digests_paginated(
        &self,
        cursor: Option<B256>,
        limit: i64,
    ) -> Result<Vec<B256>, DbError> {
        let rows = if let Some(cursor) = cursor {
            let cursor_hex = format!("0x{:x}", cursor);
            sqlx::query(
                "SELECT DISTINCT request_digest FROM (
                    SELECT request_digest FROM proof_requests
                    UNION
                    SELECT request_digest FROM request_submitted_events
                    UNION
                    SELECT request_digest FROM request_locked_events
                    UNION
                    SELECT request_digest FROM request_fulfilled_events
                    UNION
                    SELECT request_digest FROM proof_delivered_events
                ) AS all_digests
                WHERE request_digest > $1
                ORDER BY request_digest
                LIMIT $2",
            )
            .bind(cursor_hex)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT DISTINCT request_digest FROM (
                    SELECT request_digest FROM proof_requests
                    UNION
                    SELECT request_digest FROM request_submitted_events
                    UNION
                    SELECT request_digest FROM request_locked_events
                    UNION
                    SELECT request_digest FROM request_fulfilled_events
                    UNION
                    SELECT request_digest FROM proof_delivered_events
                ) AS all_digests
                ORDER BY request_digest
                LIMIT $1",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        let mut digests = Vec::new();
        for row in rows {
            let digest_hex: String = row.try_get("request_digest")?;
            let digest = B256::from_str(&digest_hex)
                .map_err(|e| DbError::BadTransaction(format!("Invalid digest: {}", e)))?;
            digests.push(digest);
        }

        Ok(digests)
    }
}

impl AnyDb {
    // Generic helper for upserting market summaries to avoid code duplication
    async fn upsert_market_summary_generic(
        &self,
        summary: HourlyMarketSummary, // Can be any alias type
        table_name: &str,
    ) -> Result<(), DbError> {
        let query_str = format!(
            "INSERT INTO {} (
                period_timestamp,
                total_fulfilled,
                unique_provers_locking_requests,
                unique_requesters_submitting_requests,
                total_fees_locked,
                total_collateral_locked,
                p10_fees_locked,
                p25_fees_locked,
                p50_fees_locked,
                p75_fees_locked,
                p90_fees_locked,
                p95_fees_locked,
                p99_fees_locked,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, CURRENT_TIMESTAMP)
            ON CONFLICT (period_timestamp) DO UPDATE SET
                total_fulfilled = EXCLUDED.total_fulfilled,
                unique_provers_locking_requests = EXCLUDED.unique_provers_locking_requests,
                unique_requesters_submitting_requests = EXCLUDED.unique_requesters_submitting_requests,
                total_fees_locked = EXCLUDED.total_fees_locked,
                total_collateral_locked = EXCLUDED.total_collateral_locked,
                p10_fees_locked = EXCLUDED.p10_fees_locked,
                p25_fees_locked = EXCLUDED.p25_fees_locked,
                p50_fees_locked = EXCLUDED.p50_fees_locked,
                p75_fees_locked = EXCLUDED.p75_fees_locked,
                p90_fees_locked = EXCLUDED.p90_fees_locked,
                p95_fees_locked = EXCLUDED.p95_fees_locked,
                p99_fees_locked = EXCLUDED.p99_fees_locked,
                total_requests_submitted = EXCLUDED.total_requests_submitted,
                total_requests_submitted_onchain = EXCLUDED.total_requests_submitted_onchain,
                total_requests_submitted_offchain = EXCLUDED.total_requests_submitted_offchain,
                total_requests_locked = EXCLUDED.total_requests_locked,
                total_requests_slashed = EXCLUDED.total_requests_slashed,
                total_expired = EXCLUDED.total_expired,
                total_locked_and_expired = EXCLUDED.total_locked_and_expired,
                total_locked_and_fulfilled = EXCLUDED.total_locked_and_fulfilled,
                locked_orders_fulfillment_rate = EXCLUDED.locked_orders_fulfillment_rate,
                updated_at = CURRENT_TIMESTAMP",
            table_name
        );

        sqlx::query(&query_str)
            .bind(summary.period_timestamp as i64)
            .bind(summary.total_fulfilled as i64)
            .bind(summary.unique_provers_locking_requests as i64)
            .bind(summary.unique_requesters_submitting_requests as i64)
            .bind(summary.total_fees_locked)
            .bind(summary.total_collateral_locked)
            .bind(summary.p10_fees_locked)
            .bind(summary.p25_fees_locked)
            .bind(summary.p50_fees_locked)
            .bind(summary.p75_fees_locked)
            .bind(summary.p90_fees_locked)
            .bind(summary.p95_fees_locked)
            .bind(summary.p99_fees_locked)
            .bind(summary.total_requests_submitted as i64)
            .bind(summary.total_requests_submitted_onchain as i64)
            .bind(summary.total_requests_submitted_offchain as i64)
            .bind(summary.total_requests_locked as i64)
            .bind(summary.total_requests_slashed as i64)
            .bind(summary.total_expired as i64)
            .bind(summary.total_locked_and_expired as i64)
            .bind(summary.total_locked_and_fulfilled as i64)
            .bind(summary.locked_orders_fulfillment_rate)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // Generic helper for getting market summaries to avoid code duplication
    async fn get_market_summaries_generic(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
        table_name: &str,
    ) -> Result<Vec<HourlyMarketSummary>, DbError> {
        let mut conditions = Vec::new();
        let mut bind_count = 0;

        // Add cursor condition
        let cursor_condition = match (cursor, sort) {
            (Some(_), SortDirection::Asc) => {
                bind_count += 1;
                Some(format!("period_timestamp > ${}", bind_count))
            }
            (Some(_), SortDirection::Desc) => {
                bind_count += 1;
                Some(format!("period_timestamp < ${}", bind_count))
            }
            (None, _) => None,
        };

        if let Some(cond) = cursor_condition {
            conditions.push(cond);
        }

        // Add after condition
        if after.is_some() {
            bind_count += 1;
            conditions.push(format!("period_timestamp > ${}", bind_count));
        }

        // Add before condition
        if before.is_some() {
            bind_count += 1;
            conditions.push(format!("period_timestamp < ${}", bind_count));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let order_clause = match sort {
            SortDirection::Asc => "ORDER BY period_timestamp ASC",
            SortDirection::Desc => "ORDER BY period_timestamp DESC",
        };

        bind_count += 1;
        let query_str = format!(
            "SELECT
                period_timestamp,
                total_fulfilled,
                unique_provers_locking_requests,
                unique_requesters_submitting_requests,
                total_fees_locked,
                total_collateral_locked,
                p10_fees_locked,
                p25_fees_locked,
                p50_fees_locked,
                p75_fees_locked,
                p90_fees_locked,
                p95_fees_locked,
                p99_fees_locked,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_cycles,
                best_peak_prove_mhz,
                best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz,
                best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id
            FROM {}
            {}
            {}
            LIMIT ${}",
            table_name, where_clause, order_clause, bind_count
        );

        let mut query = sqlx::query(&query_str);

        // Bind parameters in the same order as bind_count increments
        if let Some(cursor_ts) = cursor {
            query = query.bind(cursor_ts);
        }

        if let Some(after_ts) = after {
            query = query.bind(after_ts);
        }

        if let Some(before_ts) = before {
            query = query.bind(before_ts);
        }

        query = query.bind(limit);

        let rows = query.fetch_all(&self.pool).await?;

        let summaries = rows
            .into_iter()
            .map(|row| HourlyMarketSummary {
                period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
                total_fulfilled: row.get::<i64, _>("total_fulfilled") as u64,
                unique_provers_locking_requests: row
                    .get::<i64, _>("unique_provers_locking_requests")
                    as u64,
                unique_requesters_submitting_requests: row
                    .get::<i64, _>("unique_requesters_submitting_requests")
                    as u64,
                total_fees_locked: row.get("total_fees_locked"),
                total_collateral_locked: row.get("total_collateral_locked"),
                p10_fees_locked: row.get("p10_fees_locked"),
                p25_fees_locked: row.get("p25_fees_locked"),
                p50_fees_locked: row.get("p50_fees_locked"),
                p75_fees_locked: row.get("p75_fees_locked"),
                p90_fees_locked: row.get("p90_fees_locked"),
                p95_fees_locked: row.get("p95_fees_locked"),
                p99_fees_locked: row.get("p99_fees_locked"),
                total_requests_submitted: row.get::<i64, _>("total_requests_submitted") as u64,
                total_requests_submitted_onchain: row
                    .get::<i64, _>("total_requests_submitted_onchain")
                    as u64,
                total_requests_submitted_offchain: row
                    .get::<i64, _>("total_requests_submitted_offchain")
                    as u64,
                total_requests_locked: row.get::<i64, _>("total_requests_locked") as u64,
                total_requests_slashed: row.get::<i64, _>("total_requests_slashed") as u64,
                total_expired: row.get::<i64, _>("total_expired") as u64,
                total_locked_and_expired: row.get::<i64, _>("total_locked_and_expired") as u64,
                total_locked_and_fulfilled: row.get::<i64, _>("total_locked_and_fulfilled") as u64,
                locked_orders_fulfillment_rate: row.get::<f64, _>("locked_orders_fulfillment_rate")
                    as f32,
                total_cycles: row.get::<i64, _>("total_cycles") as u64,
                best_peak_prove_mhz: row.get::<i64, _>("best_peak_prove_mhz") as u64,
                best_peak_prove_mhz_prover: row
                    .try_get("best_peak_prove_mhz_prover")
                    .ok()
                    .flatten(),
                best_peak_prove_mhz_request_id: row
                    .try_get("best_peak_prove_mhz_request_id")
                    .ok()
                    .flatten(),
                best_effective_prove_mhz: row.get::<i64, _>("best_effective_prove_mhz") as u64,
                best_effective_prove_mhz_prover: row
                    .try_get("best_effective_prove_mhz_prover")
                    .ok()
                    .flatten(),
                best_effective_prove_mhz_request_id: row
                    .try_get("best_effective_prove_mhz_request_id")
                    .ok()
                    .flatten(),
            })
            .collect();

        Ok(summaries)
    }

    pub fn row_to_request_status(&self, row: &sqlx::any::AnyRow) -> Result<RequestStatus, DbError> {
        let request_digest_str: String = row.get("request_digest");
        let request_digest = B256::from_str(&request_digest_str)
            .map_err(|e| DbError::BadTransaction(format!("Invalid request_digest: {}", e)))?;

        let client_address_str: String = row.get("client_address");
        let client_address = Address::from_str(&client_address_str)
            .map_err(|e| DbError::BadTransaction(format!("Invalid client_address: {}", e)))?;

        let lock_prover_address_str: Option<String> =
            row.try_get("lock_prover_address").ok().flatten();
        let lock_prover_address = lock_prover_address_str.and_then(|s| Address::from_str(&s).ok());

        let fulfill_prover_address_str: Option<String> =
            row.try_get("fulfill_prover_address").ok().flatten();
        let fulfill_prover_address =
            fulfill_prover_address_str.and_then(|s| Address::from_str(&s).ok());

        let submit_tx_hash_str: Option<String> = row.try_get("submit_tx_hash").ok().flatten();
        let submit_tx_hash = submit_tx_hash_str.and_then(|s| B256::from_str(&s).ok());

        let lock_tx_hash_str: Option<String> = row.try_get("lock_tx_hash").ok().flatten();
        let lock_tx_hash = lock_tx_hash_str.and_then(|s| B256::from_str(&s).ok());

        let fulfill_tx_hash_str: Option<String> = row.try_get("fulfill_tx_hash").ok().flatten();
        let fulfill_tx_hash = fulfill_tx_hash_str.and_then(|s| B256::from_str(&s).ok());

        let slash_tx_hash_str: Option<String> = row.try_get("slash_tx_hash").ok().flatten();
        let slash_tx_hash = slash_tx_hash_str.and_then(|s| B256::from_str(&s).ok());

        let slash_recipient_str: Option<String> = row.try_get("slash_recipient").ok().flatten();
        let slash_recipient = slash_recipient_str.and_then(|s| Address::from_str(&s).ok());

        let request_status_str: String = row.get("request_status");
        let request_status = RequestStatusType::from_str(&request_status_str)
            .map_err(|e| DbError::BadTransaction(format!("Invalid request_status: {}", e)))?;

        let slashed_status_str: String = row.get("slashed_status");
        let slashed_status = SlashedStatus::from_str(&slashed_status_str)
            .map_err(|e| DbError::BadTransaction(format!("Invalid slashed_status: {}", e)))?;

        Ok(RequestStatus {
            request_digest,
            request_id: row.get("request_id"),
            request_status,
            slashed_status,
            source: row.get("source"),
            client_address,
            lock_prover_address,
            fulfill_prover_address,
            created_at: row.get::<i64, _>("created_at") as u64,
            updated_at: row.get::<i64, _>("updated_at") as u64,
            locked_at: row.try_get::<Option<i64>, _>("locked_at").ok().flatten().map(|t| t as u64),
            fulfilled_at: row
                .try_get::<Option<i64>, _>("fulfilled_at")
                .ok()
                .flatten()
                .map(|t| t as u64),
            slashed_at: row
                .try_get::<Option<i64>, _>("slashed_at")
                .ok()
                .flatten()
                .map(|t| t as u64),
            submit_block: row
                .try_get::<Option<i64>, _>("submit_block")
                .ok()
                .flatten()
                .map(|b| b as u64),
            lock_block: row
                .try_get::<Option<i64>, _>("lock_block")
                .ok()
                .flatten()
                .map(|b| b as u64),
            fulfill_block: row
                .try_get::<Option<i64>, _>("fulfill_block")
                .ok()
                .flatten()
                .map(|b| b as u64),
            slashed_block: row
                .try_get::<Option<i64>, _>("slashed_block")
                .ok()
                .flatten()
                .map(|b| b as u64),
            min_price: row.get("min_price"),
            max_price: row.get("max_price"),
            lock_collateral: row.get("lock_collateral"),
            ramp_up_start: row.get::<i64, _>("ramp_up_start") as u64,
            ramp_up_period: row.get::<i64, _>("ramp_up_period") as u64,
            expires_at: row.get::<i64, _>("expires_at") as u64,
            lock_end: row.get::<i64, _>("lock_end") as u64,
            slash_recipient,
            slash_transferred_amount: row.try_get("slash_transferred_amount").ok(),
            slash_burned_amount: row.try_get("slash_burned_amount").ok(),
            cycles: row.try_get::<Option<i64>, _>("cycles").ok().flatten().map(|c| c as u64),
            peak_prove_mhz: row
                .try_get::<Option<i64>, _>("peak_prove_mhz")
                .ok()
                .flatten()
                .map(|m| m as u64),
            effective_prove_mhz: row
                .try_get::<Option<i64>, _>("effective_prove_mhz")
                .ok()
                .flatten()
                .map(|m| m as u64),
            submit_tx_hash,
            lock_tx_hash,
            fulfill_tx_hash,
            slash_tx_hash,
            image_id: row.get("image_id"),
            image_url: row.try_get("image_url").ok(),
            selector: row.get("selector"),
            predicate_type: row.get("predicate_type"),
            predicate_data: row.get("predicate_data"),
            input_type: row.get("input_type"),
            input_data: row.get("input_data"),
            fulfill_journal: row.try_get("fulfill_journal").ok(),
            fulfill_seal: row.try_get("fulfill_seal").ok(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestDb;
    use alloy::primitives::{Address, Bytes, B256, U256};
    use boundless_market::contracts::{
        AssessorReceipt, Fulfillment, FulfillmentDataType, Offer, Predicate, ProofRequest,
        RequestId, RequestInput, Requirements, PredicateType, RequestInputType,
    };
    use risc0_zkvm::Digest;

    // generate a test request
    fn generate_request(id: u32, addr: &Address) -> ProofRequest {
        ProofRequest::new(
            RequestId::new(*addr, id),
            Requirements::new(Predicate::prefix_match(Digest::default(), Bytes::default())),
            "https://image_url.dev",
            RequestInput::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
            Offer {
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                rampUpStart: 0,
                timeout: 420,
                lockTimeout: 420,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        )
    }

    #[tokio::test]
    async fn set_get_block() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let mut block_numb = 20;
        db.set_last_block(block_numb).await.unwrap();

        let db_block = db.get_last_block().await.unwrap().unwrap();
        assert_eq!(block_numb, db_block);

        block_numb = 21;
        db.set_last_block(block_numb).await.unwrap();

        let db_block = db.get_last_block().await.unwrap().unwrap();
        assert_eq!(block_numb, db_block);
    }

    #[tokio::test]
    async fn test_transactions() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        db.add_tx(&metadata).await.unwrap();

        // Verify transaction was added
        let result = sqlx::query("SELECT * FROM transactions WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<i64, _>("block_number"), metadata.block_number as i64);
    }

    #[tokio::test]
    async fn test_proof_requests() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let request_digest = B256::ZERO;
        let request = generate_request(0, &Address::ZERO);
        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);
        db.add_proof_request(request_digest, request.clone(), &metadata, "onchain").await.unwrap();

        // Verify proof request was added
        let result = sqlx::query("SELECT * FROM proof_requests WHERE request_digest = $1")
            .bind(format!("{request_digest:x}"))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_id"), format!("{:x}", request.id));
    }

    #[tokio::test]
    async fn test_has_proof_request() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let request_digest = B256::ZERO;
        let non_existent_digest = B256::from([1; 32]);
        let request = generate_request(0, &Address::ZERO);
        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);
        // Initially, both digests should not exist
        assert!(!db.has_proof_request(request_digest).await.unwrap());
        assert!(!db.has_proof_request(non_existent_digest).await.unwrap());

        // Add a proof request
        db.add_proof_request(request_digest, request, &metadata, "onchain").await.unwrap();

        // Now the added request should exist, but the non-existent one should not
        assert!(db.has_proof_request(request_digest).await.unwrap());
        assert!(!db.has_proof_request(non_existent_digest).await.unwrap());
    }

    #[tokio::test]
    async fn test_assessor_receipts() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let receipt = AssessorReceipt {
            prover: Address::ZERO,
            callbacks: vec![],
            selectors: vec![],
            seal: Bytes::default(),
        };

        db.add_assessor_receipt(receipt.clone(), &metadata).await.unwrap();

        // Verify assessor receipt was added
        let result = sqlx::query("SELECT * FROM assessor_receipts WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("prover_address"), format!("{:x}", receipt.prover));
    }

    #[tokio::test]
    async fn test_fulfillments() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let fill = Fulfillment {
            requestDigest: B256::ZERO,
            id: U256::from(1),
            claimDigest: B256::ZERO,
            fulfillmentData: Bytes::default(),
            fulfillmentDataType: FulfillmentDataType::None,
            seal: Bytes::default(),
        };

        let prover_address = Address::ZERO;
        db.add_tx(&metadata).await.unwrap();
        db.add_proof_delivered_event(fill.requestDigest, fill.id, prover_address, &metadata)
            .await
            .unwrap();
        db.add_proof(fill.clone(), prover_address, &metadata).await.unwrap();

        // Verify proof was added
        let result = sqlx::query("SELECT * FROM proofs WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_digest"), format!("{:x}", fill.requestDigest));
    }

    #[tokio::test]
    async fn test_events() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let request_digest = B256::ZERO;
        let request_id = U256::from(1);

        // Test request submitted event
        db.add_request_submitted_event(request_digest, request_id, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM request_submitted_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_digest"), format!("{request_digest:x}"));

        // Test request locked event
        let prover_address = Address::ZERO;
        db.add_request_locked_event(request_digest, request_id, prover_address, &metadata)
            .await
            .unwrap();
        let result = sqlx::query("SELECT * FROM request_locked_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("prover_address"), format!("{prover_address:x}"));

        // Test proof delivered event
        db.add_proof_delivered_event(request_digest, request_id, prover_address, &metadata)
            .await
            .unwrap();
        let result = sqlx::query("SELECT * FROM proof_delivered_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_digest"), format!("{request_digest:x}"));

        // Test request fulfilled event
        db.add_request_fulfilled_event(request_digest, request_id, prover_address, &metadata)
            .await
            .unwrap();
        let result = sqlx::query("SELECT * FROM request_fulfilled_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_digest"), format!("{request_digest:x}"));
    }

    #[tokio::test]
    async fn test_prover_slashed_event() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let request_id = U256::from(1);
        let burn_value = U256::from(100);
        let transfer_value = U256::from(50);
        let collateral_recipient = Address::ZERO;

        // First add a request locked event (required for prover slashed event)
        let request_digest = B256::ZERO;
        let prover_address = Address::ZERO;
        db.add_request_locked_event(request_digest, request_id, prover_address, &metadata)
            .await
            .unwrap();

        // Then test prover slashed event
        db.add_prover_slashed_event(
            request_id,
            burn_value,
            transfer_value,
            collateral_recipient,
            &metadata,
        )
        .await
        .unwrap();
        let result = sqlx::query("SELECT * FROM prover_slashed_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("burn_value"), burn_value.to_string());
    }

    #[tokio::test]
    async fn test_account_events() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let account = Address::ZERO;
        let value = U256::from(100);

        // Test deposit event
        db.add_deposit_event(account, value, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM deposit_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());

        // Test withdrawal event
        db.add_withdrawal_event(account, value, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM withdrawal_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());

        // Test collateral deposit event
        db.add_collateral_deposit_event(account, value, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM collateral_deposit_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());

        // Test collateral withdrawal event
        db.add_collateral_withdrawal_event(account, value, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM collateral_withdrawal_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());
    }

    #[tokio::test]
    async fn test_callback_failed_event() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let request_id = U256::from(1);
        let callback_address = Address::ZERO;
        let error_data = vec![1, 2, 3, 4];

        db.add_callback_failed_event(request_id, callback_address, error_data.clone(), &metadata)
            .await
            .unwrap();
        let result = sqlx::query("SELECT * FROM callback_failed_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<Vec<u8>, _>("error_data"), error_data);
    }

    // Helper function to create test data for hourly market summaries
    async fn setup_hourly_summaries(db: &DbObj) -> (i64, i64) {
        let base_timestamp = 1700000000i64; // Nov 14, 2023 22:13:20 UTC
        let hour_in_seconds = 3600i64;

        // Insert 10 hourly summaries
        for i in 0..10u64 {
            let summary = HourlyMarketSummary {
                period_timestamp: (base_timestamp + (i as i64 * hour_in_seconds)) as u64,
                total_fulfilled: i,
                unique_provers_locking_requests: i * 2,
                unique_requesters_submitting_requests: i * 3,
                total_fees_locked: format!("{}", i * 1000),
                total_collateral_locked: format!("{}", i * 2000),
                p10_fees_locked: format!("{}", i * 100),
                p25_fees_locked: format!("{}", i * 250),
                p50_fees_locked: format!("{}", i * 500),
                p75_fees_locked: format!("{}", i * 750),
                p90_fees_locked: format!("{}", i * 900),
                p95_fees_locked: format!("{}", i * 950),
                p99_fees_locked: format!("{}", i * 990),
                total_requests_submitted: i * 10,
                total_requests_submitted_onchain: i * 6,
                total_requests_submitted_offchain: i * 4,
                total_requests_locked: i * 5,
                total_requests_slashed: i,
                total_expired: i,
                total_locked_and_expired: i / 2,
                total_locked_and_fulfilled: i,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: 0,
                best_peak_prove_mhz: 0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_hourly_market_summary(summary).await.unwrap();
        }

        (base_timestamp, hour_in_seconds)
    }

    #[tokio::test]
    async fn test_hourly_summaries_basic_desc_pagination() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let results =
            db.get_hourly_market_summaries(None, 3, SortDirection::Desc, None, None).await.unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].period_timestamp, (base_timestamp + (9 * hour_in_seconds)) as u64);
        assert_eq!(results[1].period_timestamp, (base_timestamp + (8 * hour_in_seconds)) as u64);
        assert_eq!(results[2].period_timestamp, (base_timestamp + (7 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_cursor_desc_pagination() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        // Get first page
        let first_page =
            db.get_hourly_market_summaries(None, 3, SortDirection::Desc, None, None).await.unwrap();

        // Use last item as cursor for next page
        let cursor = first_page[2].period_timestamp as i64;
        let results = db
            .get_hourly_market_summaries(Some(cursor), 3, SortDirection::Desc, None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].period_timestamp, (base_timestamp + (6 * hour_in_seconds)) as u64);
        assert_eq!(results[1].period_timestamp, (base_timestamp + (5 * hour_in_seconds)) as u64);
        assert_eq!(results[2].period_timestamp, (base_timestamp + (4 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_basic_asc_pagination() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let results =
            db.get_hourly_market_summaries(None, 3, SortDirection::Asc, None, None).await.unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].period_timestamp, base_timestamp as u64); // oldest first
        assert_eq!(results[1].period_timestamp, (base_timestamp + hour_in_seconds) as u64);
        assert_eq!(results[2].period_timestamp, (base_timestamp + (2 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_cursor_asc_pagination() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        // Get first page
        let first_page =
            db.get_hourly_market_summaries(None, 3, SortDirection::Asc, None, None).await.unwrap();

        // Use last item as cursor for next page
        let cursor = first_page[2].period_timestamp as i64;
        let results = db
            .get_hourly_market_summaries(Some(cursor), 3, SortDirection::Asc, None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].period_timestamp, (base_timestamp + (3 * hour_in_seconds)) as u64);
        assert_eq!(results[1].period_timestamp, (base_timestamp + (4 * hour_in_seconds)) as u64);
        assert_eq!(results[2].period_timestamp, (base_timestamp + (5 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_after_filter() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let after = base_timestamp + (3 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(None, 10, SortDirection::Desc, None, Some(after))
            .await
            .unwrap();

        // Should get hours 4-9 (6 results) - all after hour 3, desc so most recent first
        assert_eq!(results.len(), 6);
        assert_eq!(results[0].period_timestamp, (base_timestamp + (9 * hour_in_seconds)) as u64);
        assert_eq!(
            results[results.len() - 1].period_timestamp,
            (base_timestamp + (4 * hour_in_seconds)) as u64
        );
    }

    #[tokio::test]
    async fn test_hourly_summaries_before_filter() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let before = base_timestamp + (7 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(None, 10, SortDirection::Desc, Some(before), None)
            .await
            .unwrap();

        // Should get hours 0-6 (7 results) - all before hour 7
        assert_eq!(results.len(), 7);
        assert_eq!(results[0].period_timestamp, (base_timestamp + (6 * hour_in_seconds)) as u64);
        assert_eq!(results[results.len() - 1].period_timestamp, base_timestamp as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_before_and_after_filter() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let after = base_timestamp + (2 * hour_in_seconds);
        let before = base_timestamp + (7 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(None, 10, SortDirection::Desc, Some(before), Some(after))
            .await
            .unwrap();

        // Should get hours 3-6 (4 results) - between hour 2 and hour 7
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].period_timestamp, (base_timestamp + (6 * hour_in_seconds)) as u64);
        assert_eq!(results[3].period_timestamp, (base_timestamp + (3 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_daily_summaries_basic() {
        let test_db = TestDb::new().await.unwrap();
        let db = test_db.get_db();

        let base_timestamp = 1700000000i64; // 2023-11-14
        let day_in_seconds = 86400i64;

        // Insert 5 daily summaries
        for i in 0..5u64 {
            let summary = DailyMarketSummary {
                period_timestamp: (base_timestamp + (i as i64 * day_in_seconds)) as u64,
                total_fulfilled: i * 10,
                unique_provers_locking_requests: i * 2,
                unique_requesters_submitting_requests: i * 3,
                total_fees_locked: format!("{}", i * 1000),
                total_collateral_locked: format!("{}", i * 2000),
                p10_fees_locked: format!("{}", i * 100),
                p25_fees_locked: format!("{}", i * 250),
                p50_fees_locked: format!("{}", i * 500),
                p75_fees_locked: format!("{}", i * 750),
                p90_fees_locked: format!("{}", i * 900),
                p95_fees_locked: format!("{}", i * 950),
                p99_fees_locked: format!("{}", i * 990),
                total_requests_submitted: i * 10,
                total_requests_submitted_onchain: i * 6,
                total_requests_submitted_offchain: i * 4,
                total_requests_locked: i * 5,
                total_requests_slashed: i,
                total_expired: i,
                total_locked_and_expired: i / 2,
                total_locked_and_fulfilled: i * 10,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: 0,
                best_peak_prove_mhz: 0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_daily_market_summary(summary).await.unwrap();
        }

        // Test retrieval
        let results =
            db.get_daily_market_summaries(None, 10, SortDirection::Desc, None, None).await.unwrap();

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].period_timestamp, (base_timestamp + (4 * day_in_seconds)) as u64);
        assert_eq!(results[0].total_fulfilled, 40);
        assert_eq!(results[4].period_timestamp, base_timestamp as u64);
    }

    #[tokio::test]
    async fn test_weekly_summaries_basic() {
        let test_db = TestDb::new().await.unwrap();
        let db = test_db.get_db();

        let base_timestamp = 1700000000i64; // 2023-11-14
        let week_in_seconds = 604800i64;

        // Insert 4 weekly summaries
        for i in 0..4u64 {
            let summary = WeeklyMarketSummary {
                period_timestamp: (base_timestamp + (i as i64 * week_in_seconds)) as u64,
                total_fulfilled: i * 100,
                unique_provers_locking_requests: i * 20,
                unique_requesters_submitting_requests: i * 30,
                total_fees_locked: format!("{}", i * 10000),
                total_collateral_locked: format!("{}", i * 20000),
                p10_fees_locked: format!("{}", i * 1000),
                p25_fees_locked: format!("{}", i * 2500),
                p50_fees_locked: format!("{}", i * 5000),
                p75_fees_locked: format!("{}", i * 7500),
                p90_fees_locked: format!("{}", i * 9000),
                p95_fees_locked: format!("{}", i * 9500),
                p99_fees_locked: format!("{}", i * 9900),
                total_requests_submitted: i * 100,
                total_requests_submitted_onchain: i * 60,
                total_requests_submitted_offchain: i * 40,
                total_requests_locked: i * 50,
                total_requests_slashed: i * 5,
                total_expired: i * 10,
                total_locked_and_expired: i * 5,
                total_locked_and_fulfilled: i * 100,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: 0,
                best_peak_prove_mhz: 0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_weekly_market_summary(summary).await.unwrap();
        }

        // Test retrieval
        let results =
            db.get_weekly_market_summaries(None, 10, SortDirection::Asc, None, None).await.unwrap();

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].period_timestamp, base_timestamp as u64);
        assert_eq!(results[3].period_timestamp, (base_timestamp + (3 * week_in_seconds)) as u64);
        assert_eq!(results[3].total_fulfilled, 300);
    }

    #[tokio::test]
    async fn test_monthly_summaries_basic() {
        let test_db = TestDb::new().await.unwrap();
        let db = test_db.get_db();

        // Use actual month boundaries for testing
        use chrono::TimeZone;
        let jan = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap().timestamp() as u64;
        let feb = chrono::Utc.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap().timestamp() as u64;
        let mar = chrono::Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap().timestamp() as u64;

        // Insert 3 monthly summaries
        for (i, timestamp) in [jan, feb, mar].iter().enumerate() {
            let i = i as u64;
            let summary = MonthlyMarketSummary {
                period_timestamp: *timestamp,
                total_fulfilled: i * 1000,
                unique_provers_locking_requests: i * 200,
                unique_requesters_submitting_requests: i * 300,
                total_fees_locked: format!("{}", i * 100000),
                total_collateral_locked: format!("{}", i * 200000),
                p10_fees_locked: format!("{}", i * 10000),
                p25_fees_locked: format!("{}", i * 25000),
                p50_fees_locked: format!("{}", i * 50000),
                p75_fees_locked: format!("{}", i * 75000),
                p90_fees_locked: format!("{}", i * 90000),
                p95_fees_locked: format!("{}", i * 95000),
                p99_fees_locked: format!("{}", i * 99000),
                total_requests_submitted: i * 1000,
                total_requests_submitted_onchain: i * 600,
                total_requests_submitted_offchain: i * 400,
                total_requests_locked: i * 500,
                total_requests_slashed: i * 50,
                total_expired: i * 100,
                total_locked_and_expired: i * 50,
                total_locked_and_fulfilled: i * 1000,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: 0,
                best_peak_prove_mhz: 0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_monthly_market_summary(summary).await.unwrap();
        }

        // Test retrieval
        let results = db
            .get_monthly_market_summaries(None, 10, SortDirection::Desc, None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].period_timestamp, mar);
        assert_eq!(results[1].period_timestamp, feb);
        assert_eq!(results[2].period_timestamp, jan);
        assert_eq!(results[0].total_fulfilled, 2000); // March (index 2)
    }

    #[tokio::test]
    async fn test_daily_summaries_cursor_pagination() {
        let test_db = TestDb::new().await.unwrap();
        let db = test_db.get_db();

        let base_timestamp = 1700000000i64;
        let day_in_seconds = 86400i64;

        // Insert 10 daily summaries
        for i in 0..10u64 {
            let summary = DailyMarketSummary {
                period_timestamp: (base_timestamp + (i as i64 * day_in_seconds)) as u64,
                total_fulfilled: i,
                unique_provers_locking_requests: i * 2,
                unique_requesters_submitting_requests: i * 3,
                total_fees_locked: format!("{}", i * 1000),
                total_collateral_locked: format!("{}", i * 2000),
                p10_fees_locked: format!("{}", i * 100),
                p25_fees_locked: format!("{}", i * 250),
                p50_fees_locked: format!("{}", i * 500),
                p75_fees_locked: format!("{}", i * 750),
                p90_fees_locked: format!("{}", i * 900),
                p95_fees_locked: format!("{}", i * 950),
                p99_fees_locked: format!("{}", i * 990),
                total_requests_submitted: i * 10,
                total_requests_submitted_onchain: i * 6,
                total_requests_submitted_offchain: i * 4,
                total_requests_locked: i * 5,
                total_requests_slashed: i,
                total_expired: i,
                total_locked_and_expired: i / 2,
                total_locked_and_fulfilled: i,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: 0,
                best_peak_prove_mhz: 0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_daily_market_summary(summary).await.unwrap();
        }

        // Get first page
        let first_page =
            db.get_daily_market_summaries(None, 3, SortDirection::Desc, None, None).await.unwrap();

        assert_eq!(first_page.len(), 3);

        // Use cursor to get next page
        let cursor = first_page[2].period_timestamp as i64;
        let second_page = db
            .get_daily_market_summaries(Some(cursor), 3, SortDirection::Desc, None, None)
            .await
            .unwrap();

        assert_eq!(second_page.len(), 3);
        assert_eq!(second_page[0].period_timestamp, (base_timestamp + (6 * day_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_cursor_with_before_filter() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let cursor = base_timestamp + (8 * hour_in_seconds);
        let before = base_timestamp + (5 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(Some(cursor), 10, SortDirection::Desc, Some(before), None)
            .await
            .unwrap();

        // Cursor at hour 8 means timestamp < 8 (DESC)
        // Before at hour 5 means timestamp < 5
        // Combined: timestamp < 5, so we get hours 0-4 (5 results)
        assert!(results.len() <= 5);
    }

    #[tokio::test]
    async fn test_hourly_summaries_limit() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        setup_hourly_summaries(&db).await;

        let results =
            db.get_hourly_market_summaries(None, 2, SortDirection::Desc, None, None).await.unwrap();

        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_hourly_summaries_no_results() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        // Use a timestamp way in the future
        let after = base_timestamp + (100 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(None, 10, SortDirection::Desc, None, Some(after))
            .await
            .unwrap();

        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_hourly_summaries_data_integrity() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        setup_hourly_summaries(&db).await;

        let results =
            db.get_hourly_market_summaries(None, 1, SortDirection::Asc, None, None).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_fulfilled, 0);
        assert_eq!(results[0].unique_provers_locking_requests, 0);
        assert_eq!(results[0].total_fees_locked, "0");
    }

    fn create_test_status(digest: B256, status_type: RequestStatusType) -> RequestStatus {
        RequestStatus {
            request_digest: digest,
            request_id: format!("test_id_{:x}", digest),
            request_status: status_type,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: Address::ZERO,
            lock_prover_address: None,
            fulfill_prover_address: None,
            created_at: 1234567890,
            updated_at: 1234567890,
            locked_at: None,
            fulfilled_at: None,
            slashed_at: None,
            submit_block: Some(100),
            lock_block: None,
            fulfill_block: None,
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "100".to_string(),
            ramp_up_start: 0,
            ramp_up_period: 10,
            expires_at: 9999999999,
            lock_end: 9999999999,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            cycles: None,
            peak_prove_mhz: None,
            effective_prove_mhz: None,
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: None,
            fulfill_tx_hash: None,
            slash_tx_hash: None,
            image_id: "test_image".to_string(),
            image_url: Some("https://test.com".to_string()),
            selector: "test_selector".to_string(),
            predicate_type: "digest_match".to_string(),
            predicate_data: "0x00".to_string(),
            input_type: "inline".to_string(),
            input_data: "0x00".to_string(),
            fulfill_journal: None,
            fulfill_seal: None,
        }
    }

    #[tokio::test]
    async fn test_upsert_request_statuses_single_insert() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let digest = B256::from([1; 32]);
        let status = create_test_status(digest, RequestStatusType::Submitted);

        db.upsert_request_statuses(&[status.clone()]).await.unwrap();

        let result = sqlx::query("SELECT * FROM request_status WHERE request_digest = $1")
            .bind(digest.to_string())
            .fetch_one(&test_db.pool)
            .await
            .unwrap();

        assert_eq!(result.get::<String, _>("request_status"), "submitted");
        assert_eq!(result.get::<String, _>("request_id"), status.request_id);
        assert_eq!(result.get::<String, _>("source"), "onchain");
    }

    #[tokio::test]
    async fn test_upsert_request_statuses_update_conflict() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let digest = B256::from([2; 32]);
        let mut status = create_test_status(digest, RequestStatusType::Submitted);

        db.upsert_request_statuses(&[status.clone()]).await.unwrap();

        status.request_status = RequestStatusType::Locked;
        status.locked_at = Some(1234567900);
        status.lock_block = Some(200);
        status.lock_prover_address = Some(Address::from([5; 20]));
        status.lock_tx_hash = Some(B256::from([3; 32]));

        db.upsert_request_statuses(&[status.clone()]).await.unwrap();

        let result = sqlx::query("SELECT * FROM request_status WHERE request_digest = $1")
            .bind(digest.to_string())
            .fetch_one(&test_db.pool)
            .await
            .unwrap();

        assert_eq!(result.get::<String, _>("request_status"), "locked");
        assert_eq!(result.get::<Option<i64>, _>("locked_at"), Some(1234567900));
        assert_eq!(result.get::<Option<i64>, _>("lock_block"), Some(200));
        assert_eq!(result.get::<String, _>("request_id"), status.request_id);
    }

    #[tokio::test]
    async fn test_upsert_request_statuses_batch() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let mut statuses = Vec::new();
        for i in 0..100 {
            let digest = B256::from([i as u8; 32]);
            statuses.push(create_test_status(digest, RequestStatusType::Submitted));
        }

        db.upsert_request_statuses(&statuses).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_status")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();

        assert_eq!(count, 100);
    }

    #[tokio::test]
    async fn test_upsert_request_statuses_empty() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        db.upsert_request_statuses(&[]).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_status")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();

        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_get_requests_comprehensive_with_multiple_fulfillments() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let request_digest = B256::from([1; 32]);
        let request = generate_request(1, &Address::ZERO);
        let prover_a = Address::from([2; 20]);
        let prover_b = Address::from([3; 20]);

        // Add proof request
        let metadata1 = TxMetadata::new(B256::from([10; 32]), Address::ZERO, 100, 1000, 0);
        db.add_proof_request(request_digest, request.clone(), &metadata1, "onchain").await.unwrap();

        // Add submitted event
        db.add_request_submitted_event(request_digest, request.id, &metadata1).await.unwrap();

        // Add locked event
        let metadata2 = TxMetadata::new(B256::from([11; 32]), Address::ZERO, 101, 1100, 0);
        db.add_request_locked_event(request_digest, request.id, prover_a, &metadata2)
            .await
            .unwrap();

        // Add fulfilled event (fulfilled by prover_a)
        let metadata3 = TxMetadata::new(B256::from([12; 32]), Address::ZERO, 102, 1200, 0);
        db.add_request_fulfilled_event(request_digest, request.id, prover_a, &metadata3)
            .await
            .unwrap();

        // Add fulfillment from prover_b (earlier timestamp but wrong prover!)
        let seal_wrong_prover = Bytes::from(vec![99, 99, 99]);
        let metadata_wrong_prover =
            TxMetadata::new(B256::from([19; 32]), Address::ZERO, 103, 1250, 0);
        let fulfillment_wrong_prover = Fulfillment {
            requestDigest: request_digest,
            id: request.id,
            claimDigest: B256::from([29; 32]),
            fulfillmentData: Bytes::default(),
            fulfillmentDataType: FulfillmentDataType::None,
            seal: seal_wrong_prover,
        };
        db.add_proof(fulfillment_wrong_prover, prover_b, &metadata_wrong_prover).await.unwrap();

        // Add multiple proofs from prover_a with different timestamps
        let seal_early = Bytes::from(vec![1, 2, 3, 4]);
        let seal_late = Bytes::from(vec![5, 6, 7, 8]);

        let metadata_early = TxMetadata::new(B256::from([20; 32]), Address::ZERO, 104, 1300, 0);
        let fulfillment_early = Fulfillment {
            requestDigest: request_digest,
            id: request.id,
            claimDigest: B256::from([30; 32]),
            fulfillmentData: Bytes::default(),
            fulfillmentDataType: FulfillmentDataType::None,
            seal: seal_early.clone(),
        };
        db.add_proof(fulfillment_early, prover_a, &metadata_early).await.unwrap();

        let metadata_late = TxMetadata::new(B256::from([21; 32]), Address::ZERO, 105, 1400, 1);
        let fulfillment_late = Fulfillment {
            requestDigest: request_digest,
            id: request.id,
            claimDigest: B256::from([31; 32]),
            fulfillmentData: Bytes::default(),
            fulfillmentDataType: FulfillmentDataType::None,
            seal: seal_late,
        };
        db.add_proof(fulfillment_late, prover_a, &metadata_late).await.unwrap();

        // Get comprehensive request data
        let mut digest_set = std::collections::HashSet::new();
        digest_set.insert(request_digest);
        let results = db.get_requests_comprehensive(&digest_set).await.unwrap();

        // Verify we got exactly one result (no duplicates!)
        assert_eq!(results.len(), 1);

        let comprehensive = &results[0];

        // Verify basic fields
        assert_eq!(comprehensive.request_digest, request_digest);
        assert_eq!(comprehensive.source, "onchain");

        // Verify event data
        assert_eq!(comprehensive.submitted_at, Some(1000));
        assert_eq!(comprehensive.locked_at, Some(1100));
        assert_eq!(comprehensive.fulfilled_at, Some(1200));
        assert_eq!(comprehensive.lock_prover_address, Some(prover_a));
        assert_eq!(comprehensive.fulfill_prover_address, Some(prover_a));

        // Verify seal is from prover_a's EARLIEST fulfillment (timestamp 1300)
        // NOT from prover_b's fulfillment (timestamp 1250) even though it's earlier
        assert_eq!(comprehensive.fulfill_seal, Some(format!("0x{}", hex::encode(&seal_early))));
    }

    #[tokio::test]
    async fn test_add_request_submitted_events_batch() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty events - should not fail
        db.add_request_submitted_events_batch(&[]).await.unwrap();

        // Create test events with different request digests
        let mut events = Vec::new();
        for i in 0..15 {
            let request_digest = B256::from([i as u8; 32]);
            let request_id = U256::from(i);
            let metadata = TxMetadata::new(
                B256::from([(i + 100) as u8; 32]), // Different tx_hash for each
                Address::from([i as u8; 20]),
                1000 + i as u64,
                1234567890 + i as u64,
                i as u64,
            );
            events.push((request_digest, request_id, metadata));
        }

        // Add events in batch
        db.add_request_submitted_events_batch(&events).await.unwrap();

        // Verify all events were added correctly
        for (request_digest, request_id, metadata) in &events {
            let result = sqlx::query("SELECT * FROM request_submitted_events WHERE request_digest = $1")
                .bind(format!("{request_digest:x}"))
                .fetch_one(&test_db.pool)
                .await
                .unwrap();

            assert_eq!(result.get::<String, _>("request_id"), format!("{request_id:x}"));
            assert_eq!(result.get::<String, _>("tx_hash"), format!("{:x}", metadata.tx_hash));
            assert_eq!(result.get::<i64, _>("block_number"), metadata.block_number as i64);
            assert_eq!(result.get::<i64, _>("block_timestamp"), metadata.block_timestamp as i64);
        }

        // Test idempotency - adding same events again should not fail
        db.add_request_submitted_events_batch(&events).await.unwrap();

        // Verify we still have exactly 15 events (not duplicated)
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_submitted_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 15);

        // Test with large batch to verify chunking works
        let mut large_batch = Vec::new();
        for i in 100..1200 {  // 1100 events, will require 2 chunks
            let request_digest = B256::from([(i % 256) as u8; 32]);
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i / 256) as u8;
            digest_bytes[1] = (i % 256) as u8;
            let unique_digest = B256::from(digest_bytes);

            let request_id = U256::from(i);
            let metadata = TxMetadata::new(
                B256::from([(i % 256) as u8; 32]),
                Address::from([(i % 256) as u8; 20]),
                2000 + i as u64,
                2234567890 + i as u64,
                (i % 100) as u64,
            );
            large_batch.push((unique_digest, request_id, metadata));
        }

        db.add_request_submitted_events_batch(&large_batch).await.unwrap();

        // Verify a sample from the large batch
        let sample_index = 500;
        let (sample_digest, sample_id, sample_metadata) = &large_batch[sample_index];
        let result = sqlx::query("SELECT * FROM request_submitted_events WHERE request_digest = $1")
            .bind(format!("{sample_digest:x}"))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_id"), format!("{sample_id:x}"));
    }

    #[tokio::test]
    async fn test_add_txs_batch() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty list - should not fail
        db.add_txs_batch(&[]).await.unwrap();

        // Create test transactions
        let mut txs = Vec::new();
        for i in 0..10 {
            let metadata = TxMetadata::new(
                B256::from([i as u8; 32]),
                Address::from([i as u8; 20]),
                100 + i as u64,
                1234567890 + i as u64,
                i as u64,
            );
            txs.push(metadata);
        }

        // Add transactions in batch
        db.add_txs_batch(&txs).await.unwrap();

        // Verify all transactions were added
        for tx in &txs {
            let result = sqlx::query("SELECT * FROM transactions WHERE tx_hash = $1")
                .bind(format!("{:x}", tx.tx_hash))
                .fetch_one(&test_db.pool)
                .await
                .unwrap();
            assert_eq!(result.get::<i64, _>("block_number"), tx.block_number as i64);
            assert_eq!(result.get::<String, _>("from_address"), format!("{:x}", tx.from));
            assert_eq!(result.get::<i64, _>("block_timestamp"), tx.block_timestamp as i64);
            assert_eq!(result.get::<i64, _>("transaction_index"), tx.transaction_index as i64);
        }

        // Test idempotency - adding same transactions should not fail
        db.add_txs_batch(&txs).await.unwrap();

        // Verify we still have exactly 10 transactions
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM transactions")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 10);
    }

    #[tokio::test]
    async fn test_add_request_locked_events_batch() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty events - should not fail
        db.add_request_locked_events_batch(&[]).await.unwrap();

        // Create test data - more than REQUEST_LOCKED_EVENT_BATCH_SIZE to test chunking
        let mut events = Vec::new();
        for i in 0..1500 {
            // Create unique request_digest using multiple bytes to avoid collisions
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i % 256) as u8;
            digest_bytes[1] = ((i / 256) % 256) as u8;
            digest_bytes[2] = ((i / 65536) % 256) as u8;
            let request_digest = B256::from(digest_bytes);

            let request_id = U256::from(i);
            let prover = Address::from([(i % 256) as u8; 20]);

            // Create unique tx_hash to avoid collisions
            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0] = ((i + 1) % 256) as u8;
            tx_hash_bytes[1] = (((i + 1) / 256) % 256) as u8;
            tx_hash_bytes[2] = (((i + 1) / 65536) % 256) as u8;
            tx_hash_bytes[3] = 0xFF; // Add a marker byte to ensure uniqueness

            let metadata = TxMetadata::new(
                B256::from(tx_hash_bytes),
                Address::from([100; 20]),
                1000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            events.push((request_digest, request_id, prover, metadata));
        }

        // Add events in batch
        db.add_request_locked_events_batch(&events).await.unwrap();

        // Verify events were added correctly
        for (i, (request_digest, request_id, prover, metadata)) in events.iter().enumerate() {
            let result = sqlx::query(
                "SELECT * FROM request_locked_events WHERE request_digest = $1 AND tx_hash = $2"
            )
            .bind(format!("{request_digest:x}"))
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some(), "Event {} should exist", i);
            let row = result.unwrap();

            let db_request_id = row.get::<String, _>("request_id");
            let expected_request_id = format!("{request_id:x}");
            if i < 3 || i == 256 {  // Debug first few and the problematic one
                eprintln!("Event {}: DB request_id='{}', expected='{}'", i, db_request_id, expected_request_id);
                eprintln!("  request_digest: {:x}", request_digest);
                eprintln!("  tx_hash: {:x}", metadata.tx_hash);
            }
            assert_eq!(db_request_id, expected_request_id, "Request ID mismatch at index {}", i);
            assert_eq!(row.get::<String, _>("prover_address"), format!("{prover:x}"));
            assert_eq!(row.get::<i64, _>("block_number"), metadata.block_number as i64);
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_locked_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500);

        // Test idempotency - adding same events should not fail and not duplicate
        db.add_request_locked_events_batch(&events).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_locked_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500);
    }

    #[tokio::test]
    async fn test_add_proof_delivered_events_batch() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty events - should not fail
        db.add_proof_delivered_events_batch(&[]).await.unwrap();

        // Create test data - more than PROOF_DELIVERED_EVENT_BATCH_SIZE to test chunking
        let mut events = Vec::new();
        for i in 0..1200 {
            // Create unique request_digest using multiple bytes to avoid collisions
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i % 256) as u8;
            digest_bytes[1] = ((i / 256) % 256) as u8;
            digest_bytes[2] = ((i / 65536) % 256) as u8;
            let request_digest = B256::from(digest_bytes);

            let request_id = U256::from(i);
            let prover = Address::from([(i % 256) as u8; 20]);

            // Create unique tx_hash to avoid collisions
            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0] = ((i + 1) % 256) as u8;
            tx_hash_bytes[1] = (((i + 1) / 256) % 256) as u8;
            tx_hash_bytes[2] = (((i + 1) / 65536) % 256) as u8;
            tx_hash_bytes[3] = 0xEE; // Different marker byte from the other test

            let metadata = TxMetadata::new(
                B256::from(tx_hash_bytes),
                Address::from([100; 20]),
                2000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            events.push((request_digest, request_id, prover, metadata));
        }

        // Add events in batch
        db.add_proof_delivered_events_batch(&events).await.unwrap();

        // Verify events were added correctly
        for (i, (request_digest, request_id, prover, metadata)) in events.iter().enumerate() {
            let result = sqlx::query(
                "SELECT * FROM proof_delivered_events WHERE request_digest = $1 AND tx_hash = $2"
            )
            .bind(format!("{request_digest:x}"))
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some(), "Event {} should exist", i);
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("request_id"), format!("{request_id:x}"));
            assert_eq!(row.get::<String, _>("prover_address"), format!("{prover:x}"));
            assert_eq!(row.get::<i64, _>("block_number"), metadata.block_number as i64);
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proof_delivered_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1200);

        // Test idempotency - adding same events should not fail and not duplicate
        db.add_proof_delivered_events_batch(&events).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proof_delivered_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1200);
    }

    #[tokio::test]
    async fn test_add_request_fulfilled_events_batch() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty events - should not fail
        db.add_request_fulfilled_events_batch(&[]).await.unwrap();

        // Create test data - more than REQUEST_FULFILLED_EVENT_BATCH_SIZE to test chunking
        let mut events = Vec::new();
        for i in 0..1000 {
            // Create unique request_digest using multiple bytes to avoid collisions
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i % 256) as u8;
            digest_bytes[1] = ((i / 256) % 256) as u8;
            digest_bytes[2] = ((i / 65536) % 256) as u8;
            let request_digest = B256::from(digest_bytes);

            let request_id = U256::from(i);
            let prover = Address::from([(i % 256) as u8; 20]);

            // Create unique tx_hash to avoid collisions
            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0] = ((i + 1) % 256) as u8;
            tx_hash_bytes[1] = (((i + 1) / 256) % 256) as u8;
            tx_hash_bytes[2] = (((i + 1) / 65536) % 256) as u8;
            tx_hash_bytes[3] = 0xDD; // Different marker byte from the other tests

            let metadata = TxMetadata::new(
                B256::from(tx_hash_bytes),
                Address::from([100; 20]),
                3000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            events.push((request_digest, request_id, prover, metadata));
        }

        // Add events in batch
        db.add_request_fulfilled_events_batch(&events).await.unwrap();

        // Verify events were added correctly - note: only check first few due to ON CONFLICT
        // Since request_fulfilled_events has ON CONFLICT (request_digest) DO NOTHING,
        // only the first occurrence of each request_digest will be inserted
        let mut seen_digests = std::collections::HashSet::new();
        for (request_digest, request_id, prover, metadata) in &events {
            if !seen_digests.insert(*request_digest) {
                // Skip duplicates
                continue;
            }

            let result = sqlx::query(
                "SELECT * FROM request_fulfilled_events WHERE request_digest = $1"
            )
            .bind(format!("{request_digest:x}"))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some(), "Event with digest {:x} should exist", request_digest);
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("request_id"), format!("{request_id:x}"));
            assert_eq!(row.get::<String, _>("prover_address"), format!("{prover:x}"));
            assert_eq!(row.get::<i64, _>("block_number"), metadata.block_number as i64);
        }

        // Verify count - should be 1000 unique events (all are unique now)
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_fulfilled_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1000);

        // Test idempotency - adding same events should not fail and not duplicate
        db.add_request_fulfilled_events_batch(&events).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_fulfilled_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1000); // Still 1000 unique events
    }

    #[tokio::test]
    async fn test_add_proof_requests_batch() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty requests - should not fail
        db.add_proof_requests_batch(&[]).await.unwrap();

        // Helper function to create a test ProofRequest
        fn create_test_proof_request(i: usize, addr: &Address) -> ProofRequest {
            ProofRequest::new(
                RequestId::new(*addr, i as u32),
                Requirements::new(Predicate::digest_match(Digest::default(), Digest::default())),
                &format!("http://example.com/image_{}", i),
                RequestInput::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
                Offer {
                    minPrice: U256::from(1000 + i),
                    maxPrice: U256::from(2000 + i),
                    lockCollateral: U256::from(500),
                    rampUpStart: 1600000000 + i as u64,
                    timeout: 3600,
                    lockTimeout: 7200,
                    rampUpPeriod: 600,
                },
            )
        }

        // Create test data - more than PROOF_REQUEST_BATCH_SIZE to test chunking
        let mut requests = Vec::new();
        for i in 0..1500 {
            // Create unique request_digest
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i % 256) as u8;
            digest_bytes[1] = ((i / 256) % 256) as u8;
            digest_bytes[2] = ((i / 65536) % 256) as u8;
            let request_digest = B256::from(digest_bytes);

            let test_addr = Address::from([100; 20]);
            let request = create_test_proof_request(i, &test_addr);

            // Create unique metadata
            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0] = ((i + 1) % 256) as u8;
            tx_hash_bytes[1] = (((i + 1) / 256) % 256) as u8;
            tx_hash_bytes[2] = (((i + 1) / 65536) % 256) as u8;
            tx_hash_bytes[3] = 0xAA; // Unique marker

            let metadata = TxMetadata::new(
                B256::from(tx_hash_bytes),
                Address::from([100; 20]),
                5000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );

            let source = if i % 2 == 0 { "onchain" } else { "offchain" };
            requests.push((request_digest, request, metadata, source.to_string()));
        }

        // Add requests in batch
        db.add_proof_requests_batch(&requests).await.unwrap();

        // Verify requests were added correctly - check a sample
        for i in [0, 500, 999, 1499].iter() {
            let (request_digest, request, _metadata, source) = &requests[*i];

            let result = sqlx::query(
                "SELECT * FROM proof_requests WHERE request_digest = $1"
            )
            .bind(format!("{request_digest:x}"))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some(), "Request {} should exist", i);
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("request_id"), format!("{:x}", request.id));
            assert_eq!(row.get::<String, _>("source"), source.as_str());
            assert_eq!(row.get::<String, _>("min_price"), request.offer.minPrice.to_string());
            assert_eq!(row.get::<String, _>("max_price"), request.offer.maxPrice.to_string());
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proof_requests")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500);

        // Test idempotency - adding same requests should not fail and not duplicate
        db.add_proof_requests_batch(&requests).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proof_requests")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500); // Still 1500 unique requests
    }
}
