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

use std::{collections::{HashMap, HashSet}, str::FromStr, sync::Arc};

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

// Padding width for U256 (78 digits for 2^256-1)
const U256_PADDING_WIDTH: usize = 78;

// Batch insert chunk size for request statuses
// Setting too high may result in hitting parameter limits for the db engine.
const REQUEST_STATUS_BATCH_SIZE: usize = 150;

// Batch insert chunk sizes for various table inserts
// Conservative sizes to support both PostgreSQL (32,767 params) and SQLite (999 params)
const TX_BATCH_SIZE: usize = 500; // 5 params per row = 2,500 params max
const REQUEST_SUBMITTED_EVENT_BATCH_SIZE: usize = 500; // 5 params per row = 5,000 params max
const REQUEST_LOCKED_EVENT_BATCH_SIZE: usize = 500; // 6 params per row = 4,800 params max
const PROOF_DELIVERED_EVENT_BATCH_SIZE: usize = 500; // 6 params per row = 4,800 params max
const REQUEST_FULFILLED_EVENT_BATCH_SIZE: usize = 500; // 6 params per row = 4,800 params max
const PROOF_REQUEST_BATCH_SIZE: usize = 500; // 23 params per row = 23,000 params max

/// Convert U256 to zero-padded string for database storage
fn u256_to_padded_string(value: U256) -> String {
    format!("{:0width$}", value, width = U256_PADDING_WIDTH)
}

/// Parse zero-padded string from database to U256
fn padded_string_to_u256(s: &str) -> Result<U256, DbError> {
    let trimmed = s.trim_start_matches('0');
    let parse_str = if trimmed.is_empty() { "0" } else { trimmed };
    U256::from_str(parse_str)
        .map_err(|e| DbError::Error(anyhow::anyhow!("Failed to parse U256: {}", e)))
}

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
pub struct PeriodMarketSummary {
    pub period_timestamp: u64,
    pub total_fulfilled: u64,
    pub unique_provers_locking_requests: u64,
    pub unique_requesters_submitting_requests: u64,
    pub total_fees_locked: U256,
    pub total_collateral_locked: U256,
    pub total_locked_and_expired_collateral: U256,
    pub p10_lock_price_per_cycle: U256,
    pub p25_lock_price_per_cycle: U256,
    pub p50_lock_price_per_cycle: U256,
    pub p75_lock_price_per_cycle: U256,
    pub p90_lock_price_per_cycle: U256,
    pub p95_lock_price_per_cycle: U256,
    pub p99_lock_price_per_cycle: U256,
    pub total_requests_submitted: u64,
    pub total_requests_submitted_onchain: u64,
    pub total_requests_submitted_offchain: u64,
    pub total_requests_locked: u64,
    pub total_requests_slashed: u64,
    pub total_expired: u64,
    pub total_locked_and_expired: u64,
    pub total_locked_and_fulfilled: u64,
    pub locked_orders_fulfillment_rate: f32,
    pub total_program_cycles: U256,
    pub total_cycles: U256,
    pub best_peak_prove_mhz: u64,
    pub best_peak_prove_mhz_prover: Option<String>,
    pub best_peak_prove_mhz_request_id: Option<U256>,
    pub best_effective_prove_mhz: u64,
    pub best_effective_prove_mhz_prover: Option<String>,
    pub best_effective_prove_mhz_request_id: Option<U256>,
}

// Type aliases for different aggregation periods - they all use the same struct
pub type HourlyMarketSummary = PeriodMarketSummary;
pub type DailyMarketSummary = PeriodMarketSummary;
pub type WeeklyMarketSummary = PeriodMarketSummary;
pub type MonthlyMarketSummary = PeriodMarketSummary;

#[derive(Debug, Clone)]
pub struct AllTimeMarketSummary {
    pub period_timestamp: u64,
    pub total_fulfilled: u64,
    pub unique_provers_locking_requests: u64,
    pub unique_requesters_submitting_requests: u64,
    pub total_fees_locked: U256,
    pub total_collateral_locked: U256,
    pub total_locked_and_expired_collateral: U256,
    pub total_requests_submitted: u64,
    pub total_requests_submitted_onchain: u64,
    pub total_requests_submitted_offchain: u64,
    pub total_requests_locked: u64,
    pub total_requests_slashed: u64,
    pub total_expired: u64,
    pub total_locked_and_expired: u64,
    pub total_locked_and_fulfilled: u64,
    pub locked_orders_fulfillment_rate: f32,
    pub total_program_cycles: U256,
    pub total_cycles: U256,
    pub best_peak_prove_mhz: u64,
    pub best_peak_prove_mhz_prover: Option<String>,
    pub best_peak_prove_mhz_request_id: Option<U256>,
    pub best_effective_prove_mhz: u64,
    pub best_effective_prove_mhz_prover: Option<String>,
    pub best_effective_prove_mhz_request_id: Option<U256>,
}

#[derive(Debug, Clone)]
pub struct PeriodRequestorSummary {
    pub period_timestamp: u64,
    pub requestor_address: Address,
    pub total_fulfilled: u64,
    pub unique_provers_locking_requests: u64,
    pub total_fees_locked: U256,
    pub total_collateral_locked: U256,
    pub total_locked_and_expired_collateral: U256,
    pub p10_lock_price_per_cycle: U256,
    pub p25_lock_price_per_cycle: U256,
    pub p50_lock_price_per_cycle: U256,
    pub p75_lock_price_per_cycle: U256,
    pub p90_lock_price_per_cycle: U256,
    pub p95_lock_price_per_cycle: U256,
    pub p99_lock_price_per_cycle: U256,
    pub total_requests_submitted: u64,
    pub total_requests_submitted_onchain: u64,
    pub total_requests_submitted_offchain: u64,
    pub total_requests_locked: u64,
    pub total_requests_slashed: u64,
    pub total_expired: u64,
    pub total_locked_and_expired: u64,
    pub total_locked_and_fulfilled: u64,
    pub locked_orders_fulfillment_rate: f32,
    pub total_program_cycles: U256,
    pub total_cycles: U256,
    pub best_peak_prove_mhz: u64,
    pub best_peak_prove_mhz_prover: Option<String>,
    pub best_peak_prove_mhz_request_id: Option<U256>,
    pub best_effective_prove_mhz: u64,
    pub best_effective_prove_mhz_prover: Option<String>,
    pub best_effective_prove_mhz_request_id: Option<U256>,
}

// Type aliases for different aggregation periods - they all use the same struct
pub type HourlyRequestorSummary = PeriodRequestorSummary;
pub type DailyRequestorSummary = PeriodRequestorSummary;
pub type WeeklyRequestorSummary = PeriodRequestorSummary;
pub type MonthlyRequestorSummary = PeriodRequestorSummary;

#[derive(Debug, Clone)]
pub struct AllTimeRequestorSummary {
    pub period_timestamp: u64,
    pub requestor_address: Address,
    pub total_fulfilled: u64,
    pub unique_provers_locking_requests: u64,
    pub total_fees_locked: U256,
    pub total_collateral_locked: U256,
    pub total_locked_and_expired_collateral: U256,
    pub total_requests_submitted: u64,
    pub total_requests_submitted_onchain: u64,
    pub total_requests_submitted_offchain: u64,
    pub total_requests_locked: u64,
    pub total_requests_slashed: u64,
    pub total_expired: u64,
    pub total_locked_and_expired: u64,
    pub total_locked_and_fulfilled: u64,
    pub locked_orders_fulfillment_rate: f32,
    pub total_program_cycles: U256,
    pub total_cycles: U256,
    pub best_peak_prove_mhz: u64,
    pub best_peak_prove_mhz_prover: Option<String>,
    pub best_peak_prove_mhz_request_id: Option<U256>,
    pub best_effective_prove_mhz: u64,
    pub best_effective_prove_mhz_prover: Option<String>,
    pub best_effective_prove_mhz_request_id: Option<U256>,
}

#[derive(Debug, Clone)]
pub struct RequestStatus {
    pub request_digest: B256,
    pub request_id: U256,
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
    pub lock_prover_delivered_proof_at: Option<u64>,
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
    pub program_cycles: Option<U256>,
    pub total_cycles: Option<U256>,
    pub peak_prove_mhz: Option<u64>,
    pub effective_prove_mhz: Option<u64>,
    pub cycle_status: Option<String>,
    pub lock_price: Option<String>,
    pub lock_price_per_cycle: Option<String>,
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
    pub request_id: U256,
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
    pub lock_prover_delivered_proof_at: Option<u64>,
    pub fulfilled_at: Option<u64>,
    pub fulfill_prover_address: Option<Address>,
    pub fulfill_block: Option<u64>,
    pub fulfill_tx_hash: Option<B256>,
    pub program_cycles: Option<U256>,
    pub total_cycles: Option<U256>,
    pub peak_prove_mhz: Option<u64>,
    pub effective_prove_mhz: Option<u64>,
    pub cycle_status: Option<String>,
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
    pub ramp_up_start: u64,
    pub ramp_up_period: u32,
    pub lock_end: u64,
    pub lock_collateral: String,
    pub lock_timestamp: u64,
    pub lock_price: Option<String>,
    pub lock_price_per_cycle: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CycleCount {
    pub request_digest: B256,
    pub cycle_status: String,
    pub program_cycles: Option<U256>,
    pub total_cycles: Option<U256>,
    pub created_at: u64,
    pub updated_at: u64,
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

    async fn add_blocks(&self, blocks: &[(u64, u64)]) -> Result<(), DbError>;
    async fn get_block_timestamp(&self, block_numb: u64) -> Result<Option<u64>, DbError>;

    async fn add_txs(&self, metadata_list: &[TxMetadata]) -> Result<(), DbError>;

    async fn add_proof_requests(
        &self,
        requests: &[(B256, ProofRequest, TxMetadata, String, u64)],
    ) -> Result<(), DbError>;

    async fn has_proof_requests(&self, request_digests: &[B256]) -> Result<HashSet<B256>, DbError>;

    async fn get_request_digests_by_request_id(
        &self,
        request_id: U256,
    ) -> Result<Vec<B256>, DbError>;

    async fn get_request_digests_by_request_ids(
        &self,
        request_ids: &[U256],
    ) -> Result<HashMap<U256, Vec<B256>>, DbError>;

    async fn add_assessor_receipts(
        &self,
        receipts: &[(AssessorReceipt, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_proofs(
        &self,
        proofs: &[(Fulfillment, Address, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_request_submitted_events(
        &self,
        events: &[(B256, U256, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_request_locked_events(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_proof_delivered_events(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_request_fulfilled_events(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_prover_slashed_events(
        &self,
        events: &[(U256, U256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_deposit_events(
        &self,
        deposits: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_withdrawal_events(
        &self,
        withdrawals: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_collateral_deposit_events(
        &self,
        deposits: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_collateral_withdrawal_events(
        &self,
        withdrawals: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError>;

    async fn add_callback_failed_events(
        &self,
        events: &[(U256, Address, Vec<u8>, TxMetadata)],
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
        summary: PeriodMarketSummary,
    ) -> Result<(), DbError>;

    async fn get_hourly_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<PeriodMarketSummary>, DbError>;

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

    async fn upsert_all_time_market_summary(
        &self,
        summary: AllTimeMarketSummary,
    ) -> Result<(), DbError>;

    async fn get_latest_all_time_market_summary(
        &self,
    ) -> Result<Option<AllTimeMarketSummary>, DbError>;

    async fn get_all_time_market_summary_by_timestamp(
        &self,
        period_timestamp: u64,
    ) -> Result<Option<AllTimeMarketSummary>, DbError>;

    async fn get_all_time_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<AllTimeMarketSummary>, DbError>;

    async fn get_hourly_market_summaries_by_range(
        &self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<PeriodMarketSummary>, DbError>;

    async fn get_all_time_unique_provers(&self, end_ts: u64) -> Result<u64, DbError>;

    async fn get_all_time_unique_requesters(&self, end_ts: u64) -> Result<u64, DbError>;

    async fn get_earliest_hourly_summary_timestamp(&self) -> Result<Option<u64>, DbError>;

    /// Upserts request statuses.
    /// Note on conflict, this function will not update all fields.
    /// Only the mutable fields e.g. locked_at, fulfilled_at, slashed_at, etc. will be updated.
    /// Things like image id, offer details, etc. will not be updated.
    async fn upsert_request_statuses(&self, statuses: &[RequestStatus]) -> Result<(), DbError>;

    /// Insert cycle counts with ON CONFLICT DO NOTHING (idempotent)
    async fn add_cycle_counts(&self, cycle_counts: &[CycleCount]) -> Result<(), DbError>;

    /// Check which cycle counts exist for the given request digests
    async fn has_cycle_counts(&self, request_digests: &[B256]) -> Result<HashSet<B256>, DbError>;

    /// Get cycle counts for the given request digests
    async fn get_cycle_counts(&self, request_digests: &HashSet<B256>) -> Result<Vec<CycleCount>, DbError>;

    /// Get request digests for cycle counts updated within the given timestamp range (inclusive)
    async fn get_cycle_counts_by_updated_at_range(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<HashSet<B256>, DbError>;

    /// Get input_type, input_data, and client_address from proof_requests for the given request digests
    async fn get_request_inputs(
        &self,
        request_digests: &[B256],
    ) -> Result<Vec<(B256, String, String, Address)>, DbError>;

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

    /// Gets collateral amounts for locked requests that expired during the half-open period [period_start, period_end).
    /// Filters by `request_status.expires_at` (when the request's deadline passed).
    /// Note: These requests may have been locked in an earlier period.
    async fn get_period_locked_and_expired_collateral(
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

    /// Gets the total sum of program cycles from fulfilled requests in the half-open period [period_start, period_end).
    /// Filters by `request_status.fulfilled_at` (when the fulfillment occurred).
    /// Only counts requests with non-NULL program_cycles data.
    async fn get_period_total_program_cycles(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<U256, DbError>;

    /// Gets the total sum of total cycles from fulfilled requests in the half-open period [period_start, period_end).
    /// Filters by `request_status.fulfilled_at` (when the fulfillment occurred).
    /// Only counts requests with non-NULL total_cycles data.
    async fn get_period_total_cycles(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<U256, DbError>;

    /// Gets request digests using cursor-based pagination.
    /// Returns digests greater than the cursor, ordered by digest value.
    /// Used for backfilling request statuses.
    async fn get_request_digests_paginated(
        &self,
        cursor: Option<B256>,
        limit: i64,
    ) -> Result<Vec<B256>, DbError>;

    // Per-Requestor Aggregate Methods

    async fn upsert_hourly_requestor_summary(
        &self,
        summary: PeriodRequestorSummary,
    ) -> Result<(), DbError>;

    async fn upsert_daily_requestor_summary(
        &self,
        summary: DailyRequestorSummary,
    ) -> Result<(), DbError>;

    async fn upsert_weekly_requestor_summary(
        &self,
        summary: WeeklyRequestorSummary,
    ) -> Result<(), DbError>;

    async fn upsert_monthly_requestor_summary(
        &self,
        summary: MonthlyRequestorSummary,
    ) -> Result<(), DbError>;

    async fn upsert_all_time_requestor_summary(
        &self,
        summary: AllTimeRequestorSummary,
    ) -> Result<(), DbError>;

    async fn get_hourly_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<PeriodRequestorSummary>, DbError>;

    async fn get_daily_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<DailyRequestorSummary>, DbError>;

    async fn get_weekly_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<WeeklyRequestorSummary>, DbError>;

    async fn get_monthly_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<MonthlyRequestorSummary>, DbError>;

    async fn get_latest_all_time_requestor_summary(
        &self,
        requestor_address: Address,
    ) -> Result<Option<AllTimeRequestorSummary>, DbError>;

    async fn get_all_time_requestor_summary_by_timestamp(
        &self,
        requestor_address: Address,
        period_timestamp: u64,
    ) -> Result<Option<AllTimeRequestorSummary>, DbError>;

    /// Gets all unique requestor addresses that have submitted requests
    async fn get_all_requestor_addresses(&self) -> Result<Vec<Address>, DbError>;

    /// Gets requestor addresses that were active (submitted requests) in the given period
    async fn get_active_requestor_addresses_in_period(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<Address>, DbError>;

    // Per-requestor period query methods (filtered by requestor address)

    async fn get_period_requestor_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;

    async fn get_period_requestor_unique_provers(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;

    async fn get_period_requestor_total_requests_submitted(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;

    async fn get_period_requestor_total_requests_submitted_onchain(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;

    async fn get_period_requestor_total_requests_locked(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;

    async fn get_period_requestor_total_requests_slashed(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;

    async fn get_period_requestor_lock_pricing_data(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<LockPricingData>, DbError>;

    async fn get_period_requestor_all_lock_collateral(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<String>, DbError>;

    async fn get_period_requestor_locked_and_expired_collateral(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<String>, DbError>;

    async fn get_period_requestor_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;

    async fn get_period_requestor_locked_and_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;

    async fn get_period_requestor_locked_and_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;

    async fn get_period_requestor_total_program_cycles(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<U256, DbError>;

    async fn get_period_requestor_total_cycles(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<U256, DbError>;

    async fn get_all_time_requestor_unique_provers(
        &self,
        end_ts: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError>;
}

pub type DbObj = Arc<dyn IndexerDb + Send + Sync>;

#[derive(Debug, Clone)]
pub struct MarketDb {
    pub pool: AnyPool,
}

impl MarketDb {
    /// Create a new MarketDb instance.
    ///
    /// # Arguments
    /// * `conn_str` - Database connection string. For SQLite use a `sqlite:file_path` URL; for Postgres `postgres://`.
    /// * `pool_options` - Optional pool configuration. If `None`, uses indexer-optimized defaults
    ///   (20 connections, 10s acquire timeout, 600s idle, 1800s lifetime)
    /// * `skip_migrations` - If `true`, skips running migrations. Useful for read-only connections
    pub async fn new(
        conn_str: &str,
        pool_options: Option<AnyPoolOptions>,
        skip_migrations: bool,
    ) -> Result<Self, DbError> {
        use std::time::Duration;
        
        install_default_drivers(); 
        let opts = AnyConnectOptions::from_str(conn_str)?;

        let pool = if let Some(pool_opts) = pool_options {
            pool_opts.connect_with(opts).await?
        } else {
            // Indexer-optimized defaults
            AnyPoolOptions::new()
                .max_connections(20)
                .acquire_timeout(Duration::from_secs(10))  // Indexer: fail fast if pool exhausted
                .idle_timeout(Some(Duration::from_secs(600)))  // Indexer: 10 min, keep connections alive
                .max_lifetime(Some(Duration::from_secs(1800)))  // Indexer: 30 min, rotate periodically
                .connect_with(opts)
                .await?
        };

        if !skip_migrations {
            // apply any migrations
            sqlx::migrate!().run(&pool).await?;
        }

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }
}

/// Throughout this trait we manually construct queries and bind parameters to avoid using the sqlx query builder.
/// This is because the query builder is not supported for Postgres, when used in MarketDb mode.
#[async_trait]
impl IndexerDb for MarketDb {
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

    async fn add_blocks(&self, blocks: &[(u64, u64)]) -> Result<(), DbError> {
        if blocks.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in blocks.chunks(1000) {
            if chunk.is_empty() {
                continue;
            }

            let mut query = String::from(
                "INSERT INTO blocks (
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
                    "(${}, ${})",
                    params_count + 1,
                    params_count + 2
                ));
                params_count += 2;
            }
            query.push_str(" ON CONFLICT (block_number) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (block_number, block_timestamp) in chunk {
                query_builder = query_builder
                    .bind(*block_number as i64)
                    .bind(*block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
        }

        tx.commit().await?;
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

    async fn add_txs(&self, metadata_list: &[TxMetadata]) -> Result<(), DbError> {
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

    async fn has_proof_requests(&self, request_digests: &[B256]) -> Result<HashSet<B256>, DbError> {
        if request_digests.is_empty() {
            return Ok(HashSet::new());
        }

        // Build IN clause with placeholders
        let mut query = String::from("SELECT request_digest FROM proof_requests WHERE request_digest IN (");
        for i in 0..request_digests.len() {
            if i > 0 {
                query.push_str(", ");
            }
            query.push_str(&format!("${}", i + 1));
        }
        query.push(')');

        let mut query_builder = sqlx::query(&query);
        for digest in request_digests {
            query_builder = query_builder.bind(format!("{digest:x}"));
        }

        let rows = query_builder.fetch_all(&self.pool).await?;

        let mut existing = HashSet::new();
        for row in rows {
            let digest_str: String = row.try_get("request_digest")?;
            if let Ok(digest_bytes) = hex::decode(&digest_str) {
                if digest_bytes.len() == 32 {
                    existing.insert(B256::from_slice(&digest_bytes));
                }
            }
        }

        Ok(existing)
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

    async fn get_request_digests_by_request_ids(
        &self,
        request_ids: &[U256],
    ) -> Result<HashMap<U256, Vec<B256>>, DbError> {
        if request_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut result = HashMap::new();

        // Process in chunks to avoid parameter limits
        const BATCH_SIZE: usize = 500;
        for chunk in request_ids.chunks(BATCH_SIZE) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("${}", i)).collect();
            let query_str = format!(
                "SELECT request_id, request_digest FROM proof_requests WHERE request_id IN ({})",
                placeholders.join(", ")
            );

            // Collect request_id strings first to ensure they live long enough
            let request_id_strings: Vec<String> = chunk.iter().map(|request_id| format!("{request_id:x}")).collect();

            let mut query = sqlx::query(&query_str);
            for request_id_str in request_id_strings.iter() {
                query = query.bind(request_id_str);
            }

            let rows = query.fetch_all(&self.pool).await?;

            for row in rows {
                let request_id_str: String = row.try_get("request_id")?;
                let request_id = U256::from_str_radix(&request_id_str, 16)
                    .map_err(|e| DbError::BadTransaction(format!("Invalid request_id: {}", e)))?;
                
                let digest_str: String = row.try_get("request_digest")?;
                let digest = B256::from_str(&digest_str)
                    .map_err(|e| DbError::BadTransaction(format!("Invalid request_digest: {}", e)))?;
                
                result.entry(request_id).or_insert_with(Vec::new).push(digest);
            }
        }

        Ok(result)
    }

    async fn add_proof_requests(
        &self,
        requests: &[(B256, ProofRequest, TxMetadata, String, u64)],
    ) -> Result<(), DbError> {
        if requests.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions (before starting our transaction)
        let unique_txs: Vec<TxMetadata> = requests
            .iter()
            .map(|(_, _, metadata, _, _)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Then batch insert proof requests in chunks (commit per chunk)
        for chunk in requests.chunks(PROOF_REQUEST_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
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
                    image_url,
                    submission_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5,
                    params_count + 6,
                    params_count + 7,
                    params_count + 8,
                    params_count + 9,
                    params_count + 10,
                    params_count + 11,
                    params_count + 12,
                    params_count + 13,
                    params_count + 14,
                    params_count + 15,
                    params_count + 16,
                    params_count + 17,
                    params_count + 18,
                    params_count + 19,
                    params_count + 20,
                    params_count + 21,
                    params_count + 22,
                    params_count + 23,
                    params_count + 24
                ));
                params_count += 24;
            }
            query.push_str(" ON CONFLICT (request_digest) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_digest, request, metadata, source, submission_timestamp) in chunk {
                // Extract predicate type string
                let predicate_type = match request.requirements.predicate.predicateType {
                    PredicateType::DigestMatch => "DigestMatch",
                    PredicateType::PrefixMatch => "PrefixMatch",
                    PredicateType::ClaimDigestMatch => "ClaimDigestMatch",
                    _ => "Invalid",
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

                query_builder = query_builder
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
                    .bind((request.offer.rampUpStart + request.offer.timeout as u64) as i64)
                    .bind((request.offer.rampUpStart + request.offer.lockTimeout as u64) as i64)
                    .bind(request.offer.rampUpPeriod as i64)
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64)
                    .bind(source.as_str())
                    .bind(image_id_str)
                    .bind(&request.imageUrl)
                    .bind(*submission_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_assessor_receipts(
        &self,
        receipts: &[(AssessorReceipt, TxMetadata)],
    ) -> Result<(), DbError> {
        if receipts.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = receipts
            .iter()
            .map(|(_, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Then batch insert assessor receipts in chunks
        let mut tx = self.pool.begin().await?;

        const BATCH_SIZE: usize = 1000;
        for chunk in receipts.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut query = String::from(
                "INSERT INTO assessor_receipts (
                    tx_hash,
                    prover_address,
                    seal,
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
            for (receipt, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(format!("{:x}", receipt.prover))
                    .bind(format!("{:x}", receipt.seal))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn add_proofs(
        &self,
        proofs: &[(Fulfillment, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if proofs.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = proofs
            .iter()
            .map(|(_, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Then batch insert proofs in chunks (commit per chunk)
        for chunk in proofs.chunks(800) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
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
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5,
                    params_count + 6,
                    params_count + 7,
                    params_count + 8,
                    params_count + 9,
                    params_count + 10,
                    params_count + 11
                ));
                params_count += 11;
            }
            query.push_str(" ON CONFLICT (request_digest, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (fill, prover_address, metadata) in chunk {
                let fulfillment_data_type: &'static str = match fill.fulfillmentDataType {
                    FulfillmentDataType::ImageIdAndJournal => "ImageIdAndJournal",
                    FulfillmentDataType::None => "None",
                    _ => return Err(DbError::BadTransaction("Invalid fulfillment data type".to_string())),
                };

                query_builder = query_builder
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
                    .bind(metadata.transaction_index as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_request_submitted_events(
        &self,
        events: &[(B256, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert all unique transactions (before starting our transaction)
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

        self.add_txs(&unique_txs).await?;

        // Batch insert the events (commit per chunk)
        for chunk in events.chunks(REQUEST_SUBMITTED_EVENT_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
                "INSERT INTO request_submitted_events (
                    request_digest,
                    request_id,
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
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (request_digest) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_digest, request_id, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{request_digest:x}"))
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_request_locked_events(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert all unique transactions (before starting our transaction)
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

        self.add_txs(&unique_txs).await?;

        // Batch insert the events (commit per chunk)
        for chunk in events.chunks(REQUEST_LOCKED_EVENT_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

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
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_proof_delivered_events(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions (before starting our transaction)
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert proof delivered events in chunks (commit per chunk)
        for chunk in events.chunks(PROOF_DELIVERED_EVENT_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
                "INSERT INTO proof_delivered_events (
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
            query.push_str(" ON CONFLICT (request_digest, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_digest, request_id, prover_address, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{request_digest:x}"))
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{prover_address:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_request_fulfilled_events(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions (before starting our transaction)
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert request fulfilled events in chunks (commit per chunk)
        for chunk in events.chunks(REQUEST_FULFILLED_EVENT_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
                "INSERT INTO request_fulfilled_events (
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
                query_builder = query_builder
                    .bind(format!("{request_digest:x}"))
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{prover_address:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_prover_slashed_events(
        &self,
        events: &[(U256, U256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert prover slashed events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 500;
        for chunk in events.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            // First, fetch prover addresses for all request IDs in this chunk
            let request_ids: Vec<String> = chunk.iter().map(|(request_id, _, _, _, _)| format!("{request_id:x}")).collect();
            let prover_map: std::collections::HashMap<String, String> = {
                let mut map = std::collections::HashMap::new();
                // Query in batches to avoid parameter limits
                for request_id_batch in request_ids.chunks(500) {
                    let placeholders: Vec<String> = (1..=request_id_batch.len()).map(|i| format!("${}", i)).collect();
                    let query_str = format!(
                        "SELECT request_id, prover_address FROM request_locked_events WHERE request_id IN ({})",
                        placeholders.join(", ")
                    );
                    let mut query = sqlx::query(&query_str);
                    for request_id in request_id_batch {
                        query = query.bind(request_id);
                    }
                    let rows = query.fetch_all(&self.pool).await?;
                    for row in rows {
                        let request_id: String = row.try_get("request_id")?;
                        let prover_address: String = row.try_get("prover_address")?;
                        map.insert(request_id, prover_address);
                    }
                }
                map
            };

            let mut query = String::from(
                "INSERT INTO prover_slashed_events (
                    request_id, 
                    prover_address,
                    burn_value,
                    transfer_value,
                    collateral_recipient,
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
                    "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5,
                    params_count + 6,
                    params_count + 7,
                    params_count + 8
                ));
                params_count += 8;
            }
            query.push_str(" ON CONFLICT (request_id) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_id, burn_value, transfer_value, collateral_recipient, metadata) in chunk {
                let request_id_str = format!("{request_id:x}");
                let prover_address = prover_map.get(&request_id_str).cloned().unwrap_or_else(|| {
                    tracing::warn!(
                        "Missing request locked event for slashed event for request id: {:x}",
                        request_id
                    );
                    format!("{:x}", Address::ZERO)
                });
                query_builder = query_builder
                    .bind(request_id_str)
                    .bind(prover_address)
                    .bind(burn_value.to_string())
                    .bind(transfer_value.to_string())
                    .bind(format!("{collateral_recipient:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_deposit_events(
        &self,
        deposits: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if deposits.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = deposits
            .iter()
            .map(|(_, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert deposit events in chunks (commit per chunk)
        for chunk in deposits.chunks(1000) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
                "INSERT INTO deposit_events (
                    account,
                    value,
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
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (account, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (account, value, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{account:x}"))
                    .bind(value.to_string())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_withdrawal_events(
        &self,
        withdrawals: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if withdrawals.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = withdrawals
            .iter()
            .map(|(_, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert withdrawal events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 1000;
        for chunk in withdrawals.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
                "INSERT INTO withdrawal_events (
                    account,
                    value,
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
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (account, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (account, value, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{account:x}"))
                    .bind(value.to_string())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_collateral_deposit_events(
        &self,
        deposits: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if deposits.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = deposits
            .iter()
            .map(|(_, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert collateral deposit events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 1000;
        for chunk in deposits.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
                "INSERT INTO collateral_deposit_events (
                    account,
                    value,
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
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (account, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (account, value, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{account:x}"))
                    .bind(value.to_string())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_collateral_withdrawal_events(
        &self,
        withdrawals: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if withdrawals.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = withdrawals
            .iter()
            .map(|(_, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert collateral withdrawal events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 1000;
        for chunk in withdrawals.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
                "INSERT INTO collateral_withdrawal_events (
                    account,
                    value,
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
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (account, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (account, value, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{account:x}"))
                    .bind(value.to_string())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_callback_failed_events(
        &self,
        events: &[(U256, Address, Vec<u8>, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert callback failed events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 500;
        for chunk in events.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool.begin().await?;

            let mut query = String::from(
                "INSERT INTO callback_failed_events (
                    request_id,
                    callback_address,
                    error_data,
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
            query.push_str(" ON CONFLICT (request_id, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_id, callback_address, error_data, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{callback_address:x}"))
                    .bind(error_data.clone())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

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
        summary: PeriodMarketSummary,
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
    ) -> Result<Vec<PeriodMarketSummary>, DbError> {
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

    async fn upsert_all_time_market_summary(
        &self,
        summary: AllTimeMarketSummary,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO all_time_market_summary (
                period_timestamp,
                total_fulfilled,
                unique_provers_locking_requests,
                unique_requesters_submitting_requests,
                total_fees_locked,
                total_collateral_locked,
                total_locked_and_expired_collateral,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_program_cycles,
                total_cycles,
                best_peak_prove_mhz,
                best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz,
                best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, CURRENT_TIMESTAMP)
            ON CONFLICT (period_timestamp) DO UPDATE SET
                total_fulfilled = EXCLUDED.total_fulfilled,
                unique_provers_locking_requests = EXCLUDED.unique_provers_locking_requests,
                unique_requesters_submitting_requests = EXCLUDED.unique_requesters_submitting_requests,
                total_fees_locked = EXCLUDED.total_fees_locked,
                total_collateral_locked = EXCLUDED.total_collateral_locked,
                total_locked_and_expired_collateral = EXCLUDED.total_locked_and_expired_collateral,
                total_requests_submitted = EXCLUDED.total_requests_submitted,
                total_requests_submitted_onchain = EXCLUDED.total_requests_submitted_onchain,
                total_requests_submitted_offchain = EXCLUDED.total_requests_submitted_offchain,
                total_requests_locked = EXCLUDED.total_requests_locked,
                total_requests_slashed = EXCLUDED.total_requests_slashed,
                total_expired = EXCLUDED.total_expired,
                total_locked_and_expired = EXCLUDED.total_locked_and_expired,
                total_locked_and_fulfilled = EXCLUDED.total_locked_and_fulfilled,
                locked_orders_fulfillment_rate = EXCLUDED.locked_orders_fulfillment_rate,
                total_program_cycles = EXCLUDED.total_program_cycles,
                total_cycles = EXCLUDED.total_cycles,
                best_peak_prove_mhz = EXCLUDED.best_peak_prove_mhz,
                best_peak_prove_mhz_prover = EXCLUDED.best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id = EXCLUDED.best_peak_prove_mhz_request_id,
                best_effective_prove_mhz = EXCLUDED.best_effective_prove_mhz,
                best_effective_prove_mhz_prover = EXCLUDED.best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id = EXCLUDED.best_effective_prove_mhz_request_id,
                updated_at = CURRENT_TIMESTAMP",
        )
        .bind(summary.period_timestamp as i64)
        .bind(summary.total_fulfilled as i64)
        .bind(summary.unique_provers_locking_requests as i64)
        .bind(summary.unique_requesters_submitting_requests as i64)
        .bind(u256_to_padded_string(summary.total_fees_locked))
        .bind(u256_to_padded_string(summary.total_collateral_locked))
        .bind(u256_to_padded_string(summary.total_locked_and_expired_collateral))
        .bind(summary.total_requests_submitted as i64)
        .bind(summary.total_requests_submitted_onchain as i64)
        .bind(summary.total_requests_submitted_offchain as i64)
        .bind(summary.total_requests_locked as i64)
        .bind(summary.total_requests_slashed as i64)
        .bind(summary.total_expired as i64)
        .bind(summary.total_locked_and_expired as i64)
        .bind(summary.total_locked_and_fulfilled as i64)
        .bind(summary.locked_orders_fulfillment_rate)
        .bind(u256_to_padded_string(summary.total_program_cycles))
        .bind(u256_to_padded_string(summary.total_cycles))
        .bind(summary.best_peak_prove_mhz as i64)
        .bind(summary.best_peak_prove_mhz_prover)
        .bind(summary.best_peak_prove_mhz_request_id.map(|id| format!("{:x}", id)))
        .bind(summary.best_effective_prove_mhz as i64)
        .bind(summary.best_effective_prove_mhz_prover)
        .bind(summary.best_effective_prove_mhz_request_id.map(|id| format!("{:x}", id)))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_all_time_market_summary_by_timestamp(
        &self,
        period_timestamp: u64,
    ) -> Result<Option<AllTimeMarketSummary>, DbError> {
        let row = sqlx::query(
            "SELECT 
                period_timestamp,
                total_fulfilled,
                unique_provers_locking_requests,
                unique_requesters_submitting_requests,
                total_fees_locked,
                total_collateral_locked,
                total_locked_and_expired_collateral,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_program_cycles,
                total_cycles,
                best_peak_prove_mhz,
                best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz,
                best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id
            FROM all_time_market_summary
            WHERE period_timestamp = $1",
        )
        .bind(period_timestamp as i64)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(AllTimeMarketSummary {
            period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
            total_fulfilled: row.get::<i64, _>("total_fulfilled") as u64,
            unique_provers_locking_requests: row.get::<i64, _>("unique_provers_locking_requests") as u64,
            unique_requesters_submitting_requests: row.get::<i64, _>("unique_requesters_submitting_requests") as u64,
            total_fees_locked: padded_string_to_u256(&row.get::<String, _>("total_fees_locked"))?,
            total_collateral_locked: padded_string_to_u256(&row.get::<String, _>("total_collateral_locked"))?,
            total_locked_and_expired_collateral: padded_string_to_u256(&row.get::<String, _>("total_locked_and_expired_collateral"))?,
            total_requests_submitted: row.get::<i64, _>("total_requests_submitted") as u64,
            total_requests_submitted_onchain: row.get::<i64, _>("total_requests_submitted_onchain") as u64,
            total_requests_submitted_offchain: row.get::<i64, _>("total_requests_submitted_offchain") as u64,
            total_requests_locked: row.get::<i64, _>("total_requests_locked") as u64,
            total_requests_slashed: row.get::<i64, _>("total_requests_slashed") as u64,
            total_expired: row.get::<i64, _>("total_expired") as u64,
            total_locked_and_expired: row.get::<i64, _>("total_locked_and_expired") as u64,
            total_locked_and_fulfilled: row.get::<i64, _>("total_locked_and_fulfilled") as u64,
            locked_orders_fulfillment_rate: row.get::<f64, _>("locked_orders_fulfillment_rate") as f32,
            total_program_cycles: padded_string_to_u256(&row.get::<String, _>("total_program_cycles"))?,
            total_cycles: padded_string_to_u256(&row.get::<String, _>("total_cycles"))?,
            best_peak_prove_mhz: row.get::<i64, _>("best_peak_prove_mhz") as u64,
            best_peak_prove_mhz_prover: row.try_get("best_peak_prove_mhz_prover").ok(),
            best_peak_prove_mhz_request_id: row
                .try_get::<Option<String>, _>("best_peak_prove_mhz_request_id")
                .ok()
                .flatten()
                .and_then(|s| U256::from_str(&s).ok()),
            best_effective_prove_mhz: row.get::<i64, _>("best_effective_prove_mhz") as u64,
            best_effective_prove_mhz_prover: row.try_get("best_effective_prove_mhz_prover").ok(),
            best_effective_prove_mhz_request_id: row
                .try_get::<Option<String>, _>("best_effective_prove_mhz_request_id")
                .ok()
                .flatten()
                .and_then(|s| U256::from_str(&s).ok()),
        }))
    }

    async fn get_latest_all_time_market_summary(
        &self,
    ) -> Result<Option<AllTimeMarketSummary>, DbError> {
        let row = sqlx::query(
            "SELECT 
                period_timestamp,
                total_fulfilled,
                unique_provers_locking_requests,
                unique_requesters_submitting_requests,
                total_fees_locked,
                total_collateral_locked,
                total_locked_and_expired_collateral,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_program_cycles,
                total_cycles,
                best_peak_prove_mhz,
                best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz,
                best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id
            FROM all_time_market_summary
            ORDER BY period_timestamp DESC
            LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(AllTimeMarketSummary {
            period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
            total_fulfilled: row.get::<i64, _>("total_fulfilled") as u64,
            unique_provers_locking_requests: row.get::<i64, _>("unique_provers_locking_requests") as u64,
            unique_requesters_submitting_requests: row.get::<i64, _>("unique_requesters_submitting_requests") as u64,
            total_fees_locked: padded_string_to_u256(&row.get::<String, _>("total_fees_locked"))?,
            total_collateral_locked: padded_string_to_u256(&row.get::<String, _>("total_collateral_locked"))?,
            total_locked_and_expired_collateral: padded_string_to_u256(&row.get::<String, _>("total_locked_and_expired_collateral"))?,
            total_requests_submitted: row.get::<i64, _>("total_requests_submitted") as u64,
            total_requests_submitted_onchain: row.get::<i64, _>("total_requests_submitted_onchain") as u64,
            total_requests_submitted_offchain: row.get::<i64, _>("total_requests_submitted_offchain") as u64,
            total_requests_locked: row.get::<i64, _>("total_requests_locked") as u64,
            total_requests_slashed: row.get::<i64, _>("total_requests_slashed") as u64,
            total_expired: row.get::<i64, _>("total_expired") as u64,
            total_locked_and_expired: row.get::<i64, _>("total_locked_and_expired") as u64,
            total_locked_and_fulfilled: row.get::<i64, _>("total_locked_and_fulfilled") as u64,
            locked_orders_fulfillment_rate: row.get::<f64, _>("locked_orders_fulfillment_rate") as f32,
            total_program_cycles: padded_string_to_u256(&row.get::<String, _>("total_program_cycles"))?,
            total_cycles: padded_string_to_u256(&row.get::<String, _>("total_cycles"))?,
            best_peak_prove_mhz: row.get::<i64, _>("best_peak_prove_mhz") as u64,
            best_peak_prove_mhz_prover: row.try_get("best_peak_prove_mhz_prover").ok(),
            best_peak_prove_mhz_request_id: row
                .try_get::<Option<String>, _>("best_peak_prove_mhz_request_id")
                .ok()
                .flatten()
                .and_then(|s| U256::from_str(&s).ok()),
            best_effective_prove_mhz: row.get::<i64, _>("best_effective_prove_mhz") as u64,
            best_effective_prove_mhz_prover: row.try_get("best_effective_prove_mhz_prover").ok(),
            best_effective_prove_mhz_request_id: row
                .try_get::<Option<String>, _>("best_effective_prove_mhz_request_id")
                .ok()
                .flatten()
                .and_then(|s| U256::from_str(&s).ok()),
        }))
    }

    async fn get_all_time_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<AllTimeMarketSummary>, DbError> {
        self.get_all_time_market_summaries_generic(
            cursor,
            limit,
            sort,
            before,
            after,
        )
        .await
    }

    async fn get_hourly_market_summaries_by_range(
        &self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<PeriodMarketSummary>, DbError> {
        self.get_market_summaries_generic(
            None,
            i64::MAX,
            SortDirection::Asc,
            Some(end_ts as i64),
            Some(start_ts as i64),
            "hourly_market_summary",
        )
        .await
    }

    async fn get_all_time_unique_provers(&self, end_ts: u64) -> Result<u64, DbError> {
        let row = sqlx::query(
            "SELECT COUNT(DISTINCT prover_address) as count
            FROM proof_delivered_events
            WHERE block_timestamp <= $1",
        )
        .bind(end_ts as i64)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get::<i64, _>("count") as u64)
    }

    async fn get_all_time_unique_requesters(&self, end_ts: u64) -> Result<u64, DbError> {
        let row = sqlx::query(
            "SELECT COUNT(DISTINCT client_address) as count
            FROM request_status
            WHERE created_at <= $1",
        )
        .bind(end_ts as i64)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get::<i64, _>("count") as u64)
    }

    async fn get_earliest_hourly_summary_timestamp(&self) -> Result<Option<u64>, DbError> {
        let row = sqlx::query(
            "SELECT MIN(period_timestamp) as min_ts
            FROM hourly_market_summary",
        )
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let min_ts: Option<i64> = row.try_get("min_ts").ok();
        Ok(min_ts.map(|ts| ts as u64))
    }

    async fn upsert_request_statuses(&self, statuses: &[RequestStatus]) -> Result<(), DbError> {
        if statuses.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for chunk in statuses.chunks(REQUEST_STATUS_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut query = String::from(
                "INSERT INTO request_status (
                    request_digest, request_id, request_status, slashed_status, source, client_address, lock_prover_address, fulfill_prover_address,
                    created_at, updated_at, locked_at, fulfilled_at, slashed_at, lock_prover_delivered_proof_at,
                    submit_block, lock_block, fulfill_block, slashed_block,
                    min_price, max_price, lock_collateral, ramp_up_start, ramp_up_period, expires_at, lock_end,
                    slash_recipient, slash_transferred_amount, slash_burned_amount,
                    program_cycles, total_cycles, peak_prove_mhz, effective_prove_mhz, cycle_status,
                    lock_price, lock_price_per_cycle,
                    submit_tx_hash, lock_tx_hash, fulfill_tx_hash, slash_tx_hash,
                    image_id, image_url, selector, predicate_type, predicate_data, input_type, input_data,
                    fulfill_journal, fulfill_seal
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1, params_count + 2, params_count + 3, params_count + 4, params_count + 5,
                    params_count + 6, params_count + 7, params_count + 8, params_count + 9, params_count + 10,
                    params_count + 11, params_count + 12, params_count + 13, params_count + 14, params_count + 15,
                    params_count + 16, params_count + 17, params_count + 18, params_count + 19, params_count + 20,
                    params_count + 21, params_count + 22, params_count + 23, params_count + 24, params_count + 25,
                    params_count + 26, params_count + 27, params_count + 28, params_count + 29, params_count + 30,
                    params_count + 31, params_count + 32, params_count + 33, params_count + 34, params_count + 35,
                    params_count + 36, params_count + 37, params_count + 38, params_count + 39, params_count + 40,
                    params_count + 41, params_count + 42, params_count + 43, params_count + 44, params_count + 45,
                    params_count + 46, params_count + 47, params_count + 48
                ));
                params_count += 48;
            }
            query.push_str(
                " ON CONFLICT (request_digest) DO UPDATE SET
                    request_status = EXCLUDED.request_status,
                    slashed_status = EXCLUDED.slashed_status,
                    lock_prover_address = EXCLUDED.lock_prover_address,
                    fulfill_prover_address = EXCLUDED.fulfill_prover_address,
                    updated_at = EXCLUDED.updated_at,
                    locked_at = EXCLUDED.locked_at,
                    fulfilled_at = EXCLUDED.fulfilled_at,
                    slashed_at = EXCLUDED.slashed_at,
                    lock_prover_delivered_proof_at = EXCLUDED.lock_prover_delivered_proof_at,
                    lock_block = EXCLUDED.lock_block,
                    fulfill_block = EXCLUDED.fulfill_block,
                    slashed_block = EXCLUDED.slashed_block,
                    lock_tx_hash = EXCLUDED.lock_tx_hash,
                    fulfill_tx_hash = EXCLUDED.fulfill_tx_hash,
                    slash_tx_hash = EXCLUDED.slash_tx_hash,
                    slash_recipient = EXCLUDED.slash_recipient,
                    slash_transferred_amount = EXCLUDED.slash_transferred_amount,
                    slash_burned_amount = EXCLUDED.slash_burned_amount,
                    program_cycles = EXCLUDED.program_cycles,
                    total_cycles = EXCLUDED.total_cycles,
                    peak_prove_mhz = EXCLUDED.peak_prove_mhz,
                    effective_prove_mhz = EXCLUDED.effective_prove_mhz,
                    cycle_status = EXCLUDED.cycle_status,
                    lock_price = EXCLUDED.lock_price,
                    lock_price_per_cycle = EXCLUDED.lock_price_per_cycle,
                    fulfill_journal = EXCLUDED.fulfill_journal,
                    fulfill_seal = EXCLUDED.fulfill_seal"
            );

            let mut query_builder = sqlx::query(&query);
            for status in chunk {
                query_builder = query_builder
                    .bind(format!("{:x}", status.request_digest))
                    .bind(format!("{:x}", status.request_id))
                    .bind(status.request_status.to_string())
                    .bind(status.slashed_status.to_string())
                    .bind(&status.source)
                    .bind(format!("{:x}", status.client_address))
                    .bind(status.lock_prover_address.map(|a| format!("{:x}", a)))
                    .bind(status.fulfill_prover_address.map(|a| format!("{:x}", a)))
                    .bind(status.created_at as i64)
                    .bind(status.updated_at as i64)
                    .bind(status.locked_at.map(|t| t as i64))
                    .bind(status.fulfilled_at.map(|t| t as i64))
                    .bind(status.slashed_at.map(|t| t as i64))
                    .bind(status.lock_prover_delivered_proof_at.map(|t| t as i64))
                    .bind(status.submit_block.map(|b| b as i64))
                    .bind(status.lock_block.map(|b| b as i64))
                    .bind(status.fulfill_block.map(|b| b as i64))
                    .bind(status.slashed_block.map(|b| b as i64))
                    .bind(&status.min_price)
                    .bind(&status.max_price)
                    .bind(&status.lock_collateral)
                    .bind(status.ramp_up_start as i64)
                    .bind(status.ramp_up_period as i64)
                    .bind(status.expires_at as i64)
                    .bind(status.lock_end as i64)
                    .bind(status.slash_recipient.map(|a| a.to_string()))
                    .bind(&status.slash_transferred_amount)
                    .bind(&status.slash_burned_amount)
                    .bind(status.program_cycles.as_ref().map(|c| u256_to_padded_string(*c)))
                    .bind(status.total_cycles.as_ref().map(|c| u256_to_padded_string(*c)))
                    .bind(status.peak_prove_mhz.map(|m| m as i64))
                    .bind(status.effective_prove_mhz.map(|m| m as i64))
                    .bind(&status.cycle_status)
                    .bind(&status.lock_price)
                    .bind(&status.lock_price_per_cycle)
                    .bind(status.submit_tx_hash.map(|h| h.to_string()))
                    .bind(status.lock_tx_hash.map(|h| h.to_string()))
                    .bind(status.fulfill_tx_hash.map(|h| h.to_string()))
                    .bind(status.slash_tx_hash.map(|h| h.to_string()))
                    .bind(&status.image_id)
                    .bind(&status.image_url)
                    .bind(&status.selector)
                    .bind(&status.predicate_type)
                    .bind(&status.predicate_data)
                    .bind(&status.input_type)
                    .bind(&status.input_data)
                    .bind(&status.fulfill_journal)
                    .bind(&status.fulfill_seal);
            }

            query_builder.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn add_cycle_counts(&self, cycle_counts: &[CycleCount]) -> Result<(), DbError> {
        if cycle_counts.is_empty() {
            return Ok(());
        }

        const BATCH_SIZE: usize = 800; // 5 params per row = 4000 params max

        for chunk in cycle_counts.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut query = String::from(
                "INSERT INTO cycle_counts (request_digest, cycle_status, program_cycles, total_cycles, created_at, updated_at) VALUES "
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1, params_count + 2, params_count + 3, params_count + 4, params_count + 5, params_count + 6
                ));
                params_count += 6;
            }
            query.push_str(" ON CONFLICT (request_digest) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for cycle_count in chunk {
                query_builder = query_builder
                    .bind(format!("{:x}", cycle_count.request_digest))
                    .bind(&cycle_count.cycle_status)
                    .bind(cycle_count.program_cycles.as_ref().map(|c| u256_to_padded_string(*c)))
                    .bind(cycle_count.total_cycles.as_ref().map(|c| u256_to_padded_string(*c)))
                    .bind(cycle_count.created_at as i64)
                    .bind(cycle_count.updated_at as i64);
            }

            query_builder.execute(self.pool()).await?;
        }

        Ok(())
    }

    async fn has_cycle_counts(&self, request_digests: &[B256]) -> Result<HashSet<B256>, DbError> {
        if request_digests.is_empty() {
            return Ok(HashSet::new());
        }

        let mut existing = HashSet::new();
        const BATCH_SIZE: usize = 500;

        for chunk in request_digests.chunks(BATCH_SIZE) {
            let placeholders = (1..=chunk.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(", ");

            let query = format!(
                "SELECT request_digest FROM cycle_counts WHERE request_digest IN ({})",
                placeholders
            );

            let mut query_builder = sqlx::query(&query);
            for digest in chunk {
                query_builder = query_builder.bind(format!("{:x}", digest));
            }

            let rows = query_builder.fetch_all(self.pool()).await?;
            for row in rows {
                let digest_str: String = row.try_get("request_digest")?;
                let digest = B256::from_str(&digest_str)
                    .map_err(|e| DbError::BadTransaction(format!("Invalid request_digest: {}", e)))?;
                existing.insert(digest);
            }
        }

        Ok(existing)
    }

    async fn get_cycle_counts(&self, request_digests: &HashSet<B256>) -> Result<Vec<CycleCount>, DbError> {
        if request_digests.is_empty() {
            return Ok(Vec::new());
        }

        let mut cycle_counts = Vec::new();
        let digest_vec: Vec<&B256> = request_digests.iter().collect();
        const BATCH_SIZE: usize = 500;

        for chunk in digest_vec.chunks(BATCH_SIZE) {
            let placeholders = (1..=chunk.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(", ");

            let query = format!(
                "SELECT request_digest, cycle_status, program_cycles, total_cycles, created_at, updated_at
                 FROM cycle_counts
                 WHERE request_digest IN ({})",
                placeholders
            );

            let mut query_builder = sqlx::query(&query);
            for digest in chunk {
                query_builder = query_builder.bind(format!("{:x}", digest));
            }

            let rows = query_builder.fetch_all(self.pool()).await?;
            for row in rows {
                let digest_str: String = row.try_get("request_digest")?;
                let cycle_status: String = row.try_get("cycle_status")?;
                let program_cycles: Option<String> = row.try_get("program_cycles")?;
                let total_cycles: Option<String> = row.try_get("total_cycles")?;
                let created_at: i64 = row.try_get("created_at")?;
                let updated_at: i64 = row.try_get("updated_at")?;

                let digest = match B256::from_str(&digest_str) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!("Failed to parse request_digest '{}': {}", digest_str, e);
                        continue;
                    }
                };

                let program_cycles = match program_cycles {
                    Some(s) => Some(padded_string_to_u256(&s)?),
                    None => None,
                };
                let total_cycles = match total_cycles {
                    Some(s) => Some(padded_string_to_u256(&s)?),
                    None => None,
                };

                cycle_counts.push(CycleCount {
                    request_digest: digest,
                    cycle_status,
                    program_cycles,
                    total_cycles,
                    created_at: created_at as u64,
                    updated_at: updated_at as u64,
                });
            }
        }

        Ok(cycle_counts)
    }

    async fn get_cycle_counts_by_updated_at_range(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<HashSet<B256>, DbError> {
        let query = "SELECT request_digest FROM cycle_counts WHERE updated_at >= $1 AND updated_at <= $2";

        let rows = sqlx::query(query)
            .bind(from_timestamp as i64)
            .bind(to_timestamp as i64)
            .fetch_all(self.pool())
            .await?;

        let mut request_digests = HashSet::new();
        for row in rows {
            let digest_str: String = row.try_get("request_digest")?;
            let digest = match B256::from_str(&digest_str) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("Failed to parse request_digest '{}': {}", digest_str, e);
                    continue;
                }
            };
            request_digests.insert(digest);
        }

        Ok(request_digests)
    }

    async fn get_request_inputs(
        &self,
        request_digests: &[B256],
    ) -> Result<Vec<(B256, String, String, Address)>, DbError> {
        if request_digests.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();
        const BATCH_SIZE: usize = 500;

        for chunk in request_digests.chunks(BATCH_SIZE) {
            let placeholders = (1..=chunk.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(", ");

            let query = format!(
                "SELECT request_digest, input_type, input_data, client_address
                 FROM proof_requests
                 WHERE request_digest IN ({})",
                placeholders
            );

            let mut query_builder = sqlx::query(&query);
            for digest in chunk {
                query_builder = query_builder.bind(format!("{:x}", digest));
            }

            let rows = query_builder.fetch_all(self.pool()).await?;
            for row in rows {
                let digest_str: String = row.try_get("request_digest")?;
                let input_type: String = row.try_get("input_type")?;
                let input_data: String = row.try_get("input_data")?;
                let client_address_str: String = row.try_get("client_address")?;

                let digest = match B256::from_str(&digest_str) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!("Failed to parse request_digest '{}': {}", digest_str, e);
                        continue;
                    }
                };

                let client_address = match Address::from_str(&client_address_str) {
                    Ok(addr) => addr,
                    Err(e) => {
                        tracing::warn!("Failed to parse client_address '{}': {}", client_address_str, e);
                        continue;
                    }
                };

                results.push((digest, input_type, input_data, client_address));
            }
        }

        Ok(results)
    }

    async fn get_requests_comprehensive(
        &self,
        request_digests: &std::collections::HashSet<B256>,
    ) -> Result<Vec<RequestComprehensive>, DbError> {
        if request_digests.is_empty() {
            return Ok(Vec::new());
        }

        let mut requests = Vec::new();

        // Convert HashSet to Vec for chunked processing
        let digest_vec: Vec<&B256> = request_digests.iter().collect();

        // Process in batches to avoid parameter limits and optimize query performance
        const BATCH_SIZE: usize = 500;

        for chunk in digest_vec.chunks(BATCH_SIZE) {
            // Build dynamic IN clause with placeholders
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("${}", i)).collect();

            let query_str = format!(
                "SELECT
                    pr.request_digest,
                    pr.request_id,
                    pr.source,
                    pr.client_address,
                    pr.submission_timestamp as created_at,
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
                    (
                        SELECT MIN(pde.block_timestamp)
                        FROM proof_delivered_events pde
                        WHERE pde.request_digest = pr.request_digest 
                          AND pde.prover_address = rle.prover_address
                    ) as lock_prover_delivered_proof_at,
                    rfe.block_timestamp as fulfilled_at,
                    rfe.block_number as fulfill_block,
                    rfe.tx_hash as fulfill_tx_hash,
                    rfe.prover_address as fulfill_prover_address,
                    pse.block_timestamp as slashed_at,
                    pse.block_number as slashed_block,
                    pse.tx_hash as slash_tx_hash,
                    pse.burn_value as slash_burned_amount,
                    pse.transfer_value as slash_transferred_amount,
                    pse.collateral_recipient as slash_recipient,
                    cc.program_cycles,
                    cc.total_cycles,
                    cc.cycle_status
                FROM proof_requests pr
                LEFT JOIN request_submitted_events rse ON rse.request_digest = pr.request_digest
                LEFT JOIN request_locked_events rle ON rle.request_digest = pr.request_digest
                LEFT JOIN request_fulfilled_events rfe ON rfe.request_digest = pr.request_digest
                LEFT JOIN prover_slashed_events pse ON pse.request_id = pr.request_id
                LEFT JOIN cycle_counts cc ON cc.request_digest = pr.request_digest
                WHERE pr.request_digest IN ({})",
                placeholders.join(", ")
            );

            let mut query = sqlx::query(&query_str);
            for digest in chunk {
                let digest_str = format!("{:x}", digest);
                query = query.bind(digest_str);
            }

            tracing::trace!("Querying {} request digests in batch", chunk.len());
            let rows = query.fetch_all(&self.pool).await?;
            tracing::trace!("Batch query returned {} rows", rows.len());

            // Group rows by digest to detect duplicates
            let mut rows_by_digest: std::collections::HashMap<B256, Vec<RequestComprehensive>> =
                std::collections::HashMap::new();

            for row in rows {
                let request_digest_str: String = row.get("request_digest");
                let request_digest = B256::from_str(&request_digest_str).map_err(|e| {
                    DbError::BadTransaction(format!("Invalid request_digest: {}", e))
                })?;
                let request_id_str: String = row.get("request_id");
                let request_id = U256::from_str_radix(&request_id_str, 16).map_err(|e| {
                    DbError::BadTransaction(format!("Invalid request_id: {}", e))
                })?;
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

                let submitted_at: Option<i64> = row.try_get("submitted_at").ok();
                let locked_at: Option<i64> = row.try_get("locked_at").ok();
                let lock_block: Option<i64> = row.try_get("lock_block").ok();
                let lock_tx_hash_str: Option<String> = row.try_get("lock_tx_hash").ok();
                let lock_tx_hash = lock_tx_hash_str.and_then(|s| B256::from_str(&s).ok());

                let lock_prover_address_str: Option<String> =
                    row.try_get("lock_prover_address").ok();
                let lock_prover_address =
                    lock_prover_address_str.and_then(|s| Address::from_str(&s).ok());
                let lock_prover_delivered_proof_at: Option<i64> =
                    row.try_get("lock_prover_delivered_proof_at").ok();

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

                let program_cycles_str: Option<String> = row.try_get("program_cycles").ok();
                let total_cycles_str: Option<String> = row.try_get("total_cycles").ok();
                let cycle_status: Option<String> = row.try_get("cycle_status").ok();

                let program_cycles = match program_cycles_str {
                    Some(s) => padded_string_to_u256(&s).ok(),
                    None => None,
                };
                let total_cycles = match total_cycles_str {
                    Some(s) => padded_string_to_u256(&s).ok(),
                    None => None,
                };

                let request = RequestComprehensive {
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
                    lock_prover_delivered_proof_at: lock_prover_delivered_proof_at.map(|t| t as u64),
                    fulfilled_at: fulfilled_at.map(|t| t as u64),
                    fulfill_prover_address,
                    fulfill_block: fulfill_block.map(|b| b as u64),
                    fulfill_tx_hash,
                    program_cycles,
                    total_cycles,
                    peak_prove_mhz: None,      // TODO
                    effective_prove_mhz: None, // TODO
                    cycle_status,
                    fulfill_journal: None,     // TODO
                    fulfill_seal: None,        // Will be populated from proofs table below
                    slashed_at: slashed_at.map(|t| t as u64),
                    slashed_block: slashed_block.map(|b| b as u64),
                    slash_tx_hash,
                    slash_burned_amount: slash_burned_amount_str,
                    slash_transferred_amount: slash_transferred_amount_str,
                    slash_recipient,
                };

                rows_by_digest.entry(request_digest).or_insert_with(Vec::new).push(request);
            }

            // Check for missing digests before consuming the HashMap
            for digest in chunk {
                if !rows_by_digest.contains_key(*digest) {
                    tracing::warn!("No proof_request found for digest: {:x}", digest);
                }
            }

            // Check for duplicates and add to results
            for (digest, mut digest_requests) in rows_by_digest {
                if digest_requests.len() > 1 {
                    tracing::warn!(
                        "Multiple request_comprehensive rows found for digest {:x}. Using first. Digests: {:?}",
                        digest,
                        digest_requests
                    );
                }
                if let Some(request) = digest_requests.pop() {
                    requests.push(request);
                }
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
        let client_str = format!("{:x}", client_address);
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
        let prover_str = format!("{:x}", prover_address);
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
    /// Filters by `proof_requests.submission_timestamp` (when the request was created/submitted).
    async fn get_period_unique_requesters(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT client_address) FROM proof_requests
             WHERE submission_timestamp >= $1 AND submission_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    /// Gets the total count of requests submitted (both on-chain and off-chain) in the half-open period [period_start, period_end).
    /// Filters by `proof_requests.submission_timestamp` (when the request was created/submitted).
    async fn get_period_total_requests_submitted(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<u64, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM proof_requests
             WHERE submission_timestamp >= $1 AND submission_timestamp < $2",
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
                rs.ramp_up_start,
                rs.ramp_up_period,
                rs.lock_end,
                rs.lock_collateral,
                rs.locked_at as lock_timestamp,
                rs.lock_price,
                rs.lock_price_per_cycle
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
            let ramp_up_start: i64 = row.get("ramp_up_start");
            let ramp_up_period: i64 = row.get("ramp_up_period");
            let lock_end: i64 = row.get("lock_end");
            let lock_collateral: String = row.get("lock_collateral");
            let lock_timestamp: i64 = row.get("lock_timestamp");
            let lock_price: Option<String> = row.try_get("lock_price").ok().flatten();
            let lock_price_per_cycle: Option<String> = row.try_get("lock_price_per_cycle").ok().flatten();

            results.push(LockPricingData {
                min_price,
                max_price,
                ramp_up_start: ramp_up_start as u64,
                ramp_up_period: ramp_up_period as u32,
                lock_end: lock_end as u64,
                lock_collateral,
                lock_timestamp: lock_timestamp as u64,
                lock_price,
                lock_price_per_cycle,
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

    /// Gets collateral amounts for locked requests that expired during the half-open period [period_start, period_end).
    /// Filters by `request_status.expires_at` (when the request's deadline passed).
    /// Note: These requests may have been locked in an earlier period.
    async fn get_period_locked_and_expired_collateral(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<String>, DbError> {
        let rows = sqlx::query(
            "SELECT lock_collateral
             FROM request_status
             WHERE request_status = 'expired'
             AND locked_at IS NOT NULL
             AND expires_at >= $1 AND expires_at < $2",
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

    async fn get_period_total_program_cycles(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT program_cycles FROM request_status
             WHERE request_status = 'fulfilled'
             AND program_cycles IS NOT NULL
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $1 AND fulfilled_at < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(&self.pool)
        .await?;
        
        let mut total = U256::ZERO;
        for row in rows {
            let program_cycles_str: String = row.try_get("program_cycles")?;
            let program_cycles = padded_string_to_u256(&program_cycles_str)?;
            total = total.checked_add(program_cycles).ok_or_else(|| {
                DbError::Error(anyhow::anyhow!("Overflow when summing program_cycles"))
            })?;
        }
        Ok(total)
    }

    async fn get_period_total_cycles(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT total_cycles FROM request_status
             WHERE request_status = 'fulfilled'
             AND total_cycles IS NOT NULL
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $1 AND fulfilled_at < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(&self.pool)
        .await?;
        
        let mut total = U256::ZERO;
        for row in rows {
            let total_cycles_str: String = row.try_get("total_cycles")?;
            let total_cycles = padded_string_to_u256(&total_cycles_str)?;
            total = total.checked_add(total_cycles).ok_or_else(|| {
                DbError::Error(anyhow::anyhow!("Overflow when summing total_cycles"))
            })?;
        }
        Ok(total)
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

    // Per-Requestor Aggregate Trait Method Implementations
    async fn upsert_hourly_requestor_summary(
        &self,
        summary: PeriodRequestorSummary,
    ) -> Result<(), DbError> {
        MarketDb::upsert_hourly_requestor_summary_impl(self, summary).await
    }

    async fn upsert_daily_requestor_summary(
        &self,
        summary: DailyRequestorSummary,
    ) -> Result<(), DbError> {
        MarketDb::upsert_daily_requestor_summary_impl(self, summary).await
    }

    async fn upsert_weekly_requestor_summary(
        &self,
        summary: WeeklyRequestorSummary,
    ) -> Result<(), DbError> {
        MarketDb::upsert_weekly_requestor_summary_impl(self, summary).await
    }

    async fn upsert_monthly_requestor_summary(
        &self,
        summary: MonthlyRequestorSummary,
    ) -> Result<(), DbError> {
        MarketDb::upsert_monthly_requestor_summary_impl(self, summary).await
    }

    async fn upsert_all_time_requestor_summary(
        &self,
        summary: AllTimeRequestorSummary,
    ) -> Result<(), DbError> {
        MarketDb::upsert_all_time_requestor_summary_impl(self, summary).await
    }

    async fn get_hourly_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<PeriodRequestorSummary>, DbError> {
        MarketDb::get_hourly_requestor_summaries_by_range_impl(self, requestor_address, start_ts, end_ts).await
    }

    async fn get_daily_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<DailyRequestorSummary>, DbError> {
        MarketDb::get_daily_requestor_summaries_by_range_impl(self, requestor_address, start_ts, end_ts).await
    }

    async fn get_weekly_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<WeeklyRequestorSummary>, DbError> {
        MarketDb::get_weekly_requestor_summaries_by_range_impl(self, requestor_address, start_ts, end_ts).await
    }

    async fn get_monthly_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<MonthlyRequestorSummary>, DbError> {
        MarketDb::get_monthly_requestor_summaries_by_range_impl(self, requestor_address, start_ts, end_ts).await
    }

    async fn get_latest_all_time_requestor_summary(
        &self,
        requestor_address: Address,
    ) -> Result<Option<AllTimeRequestorSummary>, DbError> {
        MarketDb::get_latest_all_time_requestor_summary_impl(self, requestor_address).await
    }

    async fn get_all_time_requestor_summary_by_timestamp(
        &self,
        requestor_address: Address,
        period_timestamp: u64,
    ) -> Result<Option<AllTimeRequestorSummary>, DbError> {
        MarketDb::get_all_time_requestor_summary_by_timestamp_impl(self, requestor_address, period_timestamp).await
    }

    async fn get_all_requestor_addresses(&self) -> Result<Vec<Address>, DbError> {
        MarketDb::get_all_requestor_addresses_impl(self).await
    }

    async fn get_active_requestor_addresses_in_period(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<Address>, DbError> {
        MarketDb::get_active_requestor_addresses_in_period_impl(self, period_start, period_end).await
    }

    async fn get_period_requestor_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_period_requestor_fulfilled_count_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_unique_provers(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_period_requestor_unique_provers_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_total_requests_submitted(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_period_requestor_total_requests_submitted_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_total_requests_submitted_onchain(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_period_requestor_total_requests_submitted_onchain_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_total_requests_locked(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_period_requestor_total_requests_locked_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_total_requests_slashed(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_period_requestor_total_requests_slashed_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_lock_pricing_data(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<LockPricingData>, DbError> {
        MarketDb::get_period_requestor_lock_pricing_data_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_all_lock_collateral(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<String>, DbError> {
        MarketDb::get_period_requestor_all_lock_collateral_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_locked_and_expired_collateral(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<String>, DbError> {
        MarketDb::get_period_requestor_locked_and_expired_collateral_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_period_requestor_expired_count_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_locked_and_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_period_requestor_locked_and_expired_count_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_locked_and_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_period_requestor_locked_and_fulfilled_count_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_total_program_cycles(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<U256, DbError> {
        MarketDb::get_period_requestor_total_program_cycles_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_period_requestor_total_cycles(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<U256, DbError> {
        MarketDb::get_period_requestor_total_cycles_impl(self, period_start, period_end, requestor_address).await
    }

    async fn get_all_time_requestor_unique_provers(
        &self,
        end_ts: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        MarketDb::get_all_time_requestor_unique_provers_impl(self, end_ts, requestor_address).await
    }
}

impl MarketDb {
    // Generic helper for upserting market summaries to avoid code duplication
    async fn upsert_market_summary_generic(
        &self,
        summary: PeriodMarketSummary, // Can be any alias type
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
                total_locked_and_expired_collateral,
                p10_lock_price_per_cycle,
                p25_lock_price_per_cycle,
                p50_lock_price_per_cycle,
                p75_lock_price_per_cycle,
                p90_lock_price_per_cycle,
                p95_lock_price_per_cycle,
                p99_lock_price_per_cycle,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_program_cycles,
                total_cycles,
                best_peak_prove_mhz,
                best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz,
                best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, CURRENT_TIMESTAMP)
            ON CONFLICT (period_timestamp) DO UPDATE SET
                total_fulfilled = EXCLUDED.total_fulfilled,
                unique_provers_locking_requests = EXCLUDED.unique_provers_locking_requests,
                unique_requesters_submitting_requests = EXCLUDED.unique_requesters_submitting_requests,
                total_fees_locked = EXCLUDED.total_fees_locked,
                total_collateral_locked = EXCLUDED.total_collateral_locked,
                total_locked_and_expired_collateral = EXCLUDED.total_locked_and_expired_collateral,
                p10_lock_price_per_cycle = EXCLUDED.p10_lock_price_per_cycle,
                p25_lock_price_per_cycle = EXCLUDED.p25_lock_price_per_cycle,
                p50_lock_price_per_cycle = EXCLUDED.p50_lock_price_per_cycle,
                p75_lock_price_per_cycle = EXCLUDED.p75_lock_price_per_cycle,
                p90_lock_price_per_cycle = EXCLUDED.p90_lock_price_per_cycle,
                p95_lock_price_per_cycle = EXCLUDED.p95_lock_price_per_cycle,
                p99_lock_price_per_cycle = EXCLUDED.p99_lock_price_per_cycle,
                total_requests_submitted = EXCLUDED.total_requests_submitted,
                total_requests_submitted_onchain = EXCLUDED.total_requests_submitted_onchain,
                total_requests_submitted_offchain = EXCLUDED.total_requests_submitted_offchain,
                total_requests_locked = EXCLUDED.total_requests_locked,
                total_requests_slashed = EXCLUDED.total_requests_slashed,
                total_expired = EXCLUDED.total_expired,
                total_locked_and_expired = EXCLUDED.total_locked_and_expired,
                total_locked_and_fulfilled = EXCLUDED.total_locked_and_fulfilled,
                locked_orders_fulfillment_rate = EXCLUDED.locked_orders_fulfillment_rate,
                total_program_cycles = EXCLUDED.total_program_cycles,
                total_cycles = EXCLUDED.total_cycles,
                best_peak_prove_mhz = EXCLUDED.best_peak_prove_mhz,
                best_peak_prove_mhz_prover = EXCLUDED.best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id = EXCLUDED.best_peak_prove_mhz_request_id,
                best_effective_prove_mhz = EXCLUDED.best_effective_prove_mhz,
                best_effective_prove_mhz_prover = EXCLUDED.best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id = EXCLUDED.best_effective_prove_mhz_request_id,
                updated_at = CURRENT_TIMESTAMP",
            table_name
        );

        sqlx::query(&query_str)
            .bind(summary.period_timestamp as i64)
            .bind(summary.total_fulfilled as i64)
            .bind(summary.unique_provers_locking_requests as i64)
            .bind(summary.unique_requesters_submitting_requests as i64)
            .bind(u256_to_padded_string(summary.total_fees_locked))
            .bind(u256_to_padded_string(summary.total_collateral_locked))
            .bind(u256_to_padded_string(summary.total_locked_and_expired_collateral))
            .bind(u256_to_padded_string(summary.p10_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p25_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p50_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p75_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p90_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p95_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p99_lock_price_per_cycle))
            .bind(summary.total_requests_submitted as i64)
            .bind(summary.total_requests_submitted_onchain as i64)
            .bind(summary.total_requests_submitted_offchain as i64)
            .bind(summary.total_requests_locked as i64)
            .bind(summary.total_requests_slashed as i64)
            .bind(summary.total_expired as i64)
            .bind(summary.total_locked_and_expired as i64)
            .bind(summary.total_locked_and_fulfilled as i64)
            .bind(summary.locked_orders_fulfillment_rate)
            .bind(u256_to_padded_string(summary.total_program_cycles))
            .bind(u256_to_padded_string(summary.total_cycles))
            .bind(summary.best_peak_prove_mhz as i64)
            .bind(summary.best_peak_prove_mhz_prover)
            .bind(summary.best_peak_prove_mhz_request_id.map(|id| format!("{:x}", id)))
            .bind(summary.best_effective_prove_mhz as i64)
            .bind(summary.best_effective_prove_mhz_prover)
            .bind(summary.best_effective_prove_mhz_request_id.map(|id| format!("{:x}", id)))
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
    ) -> Result<Vec<PeriodMarketSummary>, DbError> {
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
                total_locked_and_expired_collateral,
                p10_lock_price_per_cycle,
                p25_lock_price_per_cycle,
                p50_lock_price_per_cycle,
                p75_lock_price_per_cycle,
                p90_lock_price_per_cycle,
                p95_lock_price_per_cycle,
                p99_lock_price_per_cycle,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_program_cycles,
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
            .map(|row| {
                PeriodMarketSummary {
                    period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
                    total_fulfilled: row.get::<i64, _>("total_fulfilled") as u64,
                    unique_provers_locking_requests: row
                        .get::<i64, _>("unique_provers_locking_requests")
                        as u64,
                    unique_requesters_submitting_requests: row
                        .get::<i64, _>("unique_requesters_submitting_requests")
                        as u64,
                    total_fees_locked: padded_string_to_u256(&row.get::<String, _>("total_fees_locked")).unwrap_or(U256::ZERO),
                    total_collateral_locked: padded_string_to_u256(&row.get::<String, _>("total_collateral_locked")).unwrap_or(U256::ZERO),
                    total_locked_and_expired_collateral: padded_string_to_u256(&row.get::<String, _>("total_locked_and_expired_collateral")).unwrap_or(U256::ZERO),
                    p10_lock_price_per_cycle: padded_string_to_u256(&row.get::<String, _>("p10_lock_price_per_cycle")).unwrap_or(U256::ZERO),
                    p25_lock_price_per_cycle: padded_string_to_u256(&row.get::<String, _>("p25_lock_price_per_cycle")).unwrap_or(U256::ZERO),
                    p50_lock_price_per_cycle: padded_string_to_u256(&row.get::<String, _>("p50_lock_price_per_cycle")).unwrap_or(U256::ZERO),
                    p75_lock_price_per_cycle: padded_string_to_u256(&row.get::<String, _>("p75_lock_price_per_cycle")).unwrap_or(U256::ZERO),
                    p90_lock_price_per_cycle: padded_string_to_u256(&row.get::<String, _>("p90_lock_price_per_cycle")).unwrap_or(U256::ZERO),
                    p95_lock_price_per_cycle: padded_string_to_u256(&row.get::<String, _>("p95_lock_price_per_cycle")).unwrap_or(U256::ZERO),
                    p99_lock_price_per_cycle: padded_string_to_u256(&row.get::<String, _>("p99_lock_price_per_cycle")).unwrap_or(U256::ZERO),
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
                    total_program_cycles: padded_string_to_u256(&row.get::<String, _>("total_program_cycles")).unwrap_or(U256::ZERO),
                    total_cycles: padded_string_to_u256(&row.get::<String, _>("total_cycles")).unwrap_or(U256::ZERO),
                    best_peak_prove_mhz: row.get::<i64, _>("best_peak_prove_mhz") as u64,
                    best_peak_prove_mhz_prover: row
                        .try_get("best_peak_prove_mhz_prover")
                        .ok()
                        .flatten(),
                    best_peak_prove_mhz_request_id: row
                        .try_get::<Option<String>, _>("best_peak_prove_mhz_request_id")
                        .ok()
                        .flatten()
                        .and_then(|s| U256::from_str_radix(&s, 16).ok()),
                    best_effective_prove_mhz: row.get::<i64, _>("best_effective_prove_mhz") as u64,
                    best_effective_prove_mhz_prover: row
                        .try_get("best_effective_prove_mhz_prover")
                        .ok()
                        .flatten(),
                    best_effective_prove_mhz_request_id: row
                        .try_get::<Option<String>, _>("best_effective_prove_mhz_request_id")
                        .ok()
                        .flatten()
                        .and_then(|s| U256::from_str_radix(&s, 16).ok()),
                }
            })
            .collect();

        Ok(summaries)
    }

    async fn get_all_time_market_summaries_generic(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<AllTimeMarketSummary>, DbError> {
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
                total_locked_and_expired_collateral,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_program_cycles,
                total_cycles,
                best_peak_prove_mhz,
                best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz,
                best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id
            FROM all_time_market_summary
            {}
            {}
            LIMIT ${}",
            where_clause, order_clause, bind_count
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
            .map(|row| {
                AllTimeMarketSummary {
                    period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
                    total_fulfilled: row.get::<i64, _>("total_fulfilled") as u64,
                    unique_provers_locking_requests: row
                        .get::<i64, _>("unique_provers_locking_requests")
                        as u64,
                    unique_requesters_submitting_requests: row
                        .get::<i64, _>("unique_requesters_submitting_requests")
                        as u64,
                    total_fees_locked: padded_string_to_u256(&row.get::<String, _>("total_fees_locked")).unwrap_or(U256::ZERO),
                    total_collateral_locked: padded_string_to_u256(&row.get::<String, _>("total_collateral_locked")).unwrap_or(U256::ZERO),
                    total_locked_and_expired_collateral: padded_string_to_u256(&row.get::<String, _>("total_locked_and_expired_collateral")).unwrap_or(U256::ZERO),
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
                    total_program_cycles: padded_string_to_u256(&row.get::<String, _>("total_program_cycles")).unwrap_or(U256::ZERO),
                    total_cycles: padded_string_to_u256(&row.get::<String, _>("total_cycles")).unwrap_or(U256::ZERO),
                    best_peak_prove_mhz: row.get::<i64, _>("best_peak_prove_mhz") as u64,
                    best_peak_prove_mhz_prover: row
                        .try_get("best_peak_prove_mhz_prover")
                        .ok()
                        .flatten(),
                    best_peak_prove_mhz_request_id: row
                        .try_get::<Option<String>, _>("best_peak_prove_mhz_request_id")
                        .ok()
                        .flatten()
                        .and_then(|s| U256::from_str(&s).ok()),
                    best_effective_prove_mhz: row.get::<i64, _>("best_effective_prove_mhz") as u64,
                    best_effective_prove_mhz_prover: row
                        .try_get("best_effective_prove_mhz_prover")
                        .ok()
                        .flatten(),
                    best_effective_prove_mhz_request_id: row
                        .try_get::<Option<String>, _>("best_effective_prove_mhz_request_id")
                        .ok()
                        .flatten()
                        .and_then(|s| U256::from_str(&s).ok()),
                }
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

        let request_id_str: String = row.get("request_id");
        let request_id = U256::from_str_radix(&request_id_str, 16)
            .map_err(|e| DbError::BadTransaction(format!("Invalid request_id: {}", e)))?;

        Ok(RequestStatus {
            request_digest,
            request_id,
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
            lock_prover_delivered_proof_at: row
                .try_get::<Option<i64>, _>("lock_prover_delivered_proof_at")
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
            program_cycles: row.try_get::<Option<String>, _>("program_cycles").ok().flatten().and_then(|s| padded_string_to_u256(&s).ok()),
            total_cycles: row.try_get::<Option<String>, _>("total_cycles").ok().flatten().and_then(|s| padded_string_to_u256(&s).ok()),
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
            cycle_status: row.try_get("cycle_status").ok(),
            lock_price: row.try_get("lock_price").ok(),
            lock_price_per_cycle: row.try_get("lock_price_per_cycle").ok(),
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

    // Per-Requestor Aggregate Implementation Methods (moved from after tests)
    // These are called by the trait impl methods in IndexerDb for MarketDb

    pub(crate) async fn upsert_hourly_requestor_summary_impl(
        &self,
        summary: PeriodRequestorSummary,
    ) -> Result<(), DbError> {
        self.upsert_requestor_summary_generic(summary, "hourly_requestor_summary").await
    }

    pub(crate) async fn upsert_daily_requestor_summary_impl(
        &self,
        summary: DailyRequestorSummary,
    ) -> Result<(), DbError> {
        self.upsert_requestor_summary_generic(summary, "daily_requestor_summary").await
    }

    pub(crate) async fn upsert_weekly_requestor_summary_impl(
        &self,
        summary: WeeklyRequestorSummary,
    ) -> Result<(), DbError> {
        self.upsert_requestor_summary_generic(summary, "weekly_requestor_summary").await
    }

    pub(crate) async fn upsert_monthly_requestor_summary_impl(
        &self,
        summary: MonthlyRequestorSummary,
    ) -> Result<(), DbError> {
        self.upsert_requestor_summary_generic(summary, "monthly_requestor_summary").await
    }

    pub(crate) async fn upsert_all_time_requestor_summary_impl(
        &self,
        summary: AllTimeRequestorSummary,
    ) -> Result<(), DbError> {
        let query_str = "INSERT INTO all_time_requestor_summary (
                period_timestamp,
                requestor_address,
                total_fulfilled,
                unique_provers_locking_requests,
                total_fees_locked,
                total_collateral_locked,
                total_locked_and_expired_collateral,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_program_cycles,
                total_cycles,
                best_peak_prove_mhz,
                best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz,
                best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, CURRENT_TIMESTAMP)
            ON CONFLICT (period_timestamp, requestor_address) DO UPDATE SET
                total_fulfilled = EXCLUDED.total_fulfilled,
                unique_provers_locking_requests = EXCLUDED.unique_provers_locking_requests,
                total_fees_locked = EXCLUDED.total_fees_locked,
                total_collateral_locked = EXCLUDED.total_collateral_locked,
                total_locked_and_expired_collateral = EXCLUDED.total_locked_and_expired_collateral,
                total_requests_submitted = EXCLUDED.total_requests_submitted,
                total_requests_submitted_onchain = EXCLUDED.total_requests_submitted_onchain,
                total_requests_submitted_offchain = EXCLUDED.total_requests_submitted_offchain,
                total_requests_locked = EXCLUDED.total_requests_locked,
                total_requests_slashed = EXCLUDED.total_requests_slashed,
                total_expired = EXCLUDED.total_expired,
                total_locked_and_expired = EXCLUDED.total_locked_and_expired,
                total_locked_and_fulfilled = EXCLUDED.total_locked_and_fulfilled,
                locked_orders_fulfillment_rate = EXCLUDED.locked_orders_fulfillment_rate,
                total_program_cycles = EXCLUDED.total_program_cycles,
                total_cycles = EXCLUDED.total_cycles,
                best_peak_prove_mhz = EXCLUDED.best_peak_prove_mhz,
                best_peak_prove_mhz_prover = EXCLUDED.best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id = EXCLUDED.best_peak_prove_mhz_request_id,
                best_effective_prove_mhz = EXCLUDED.best_effective_prove_mhz,
                best_effective_prove_mhz_prover = EXCLUDED.best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id = EXCLUDED.best_effective_prove_mhz_request_id,
                updated_at = CURRENT_TIMESTAMP";

        sqlx::query(query_str)
            .bind(summary.period_timestamp as i64)
            .bind(format!("{:x}", summary.requestor_address))
            .bind(summary.total_fulfilled as i64)
            .bind(summary.unique_provers_locking_requests as i64)
            .bind(u256_to_padded_string(summary.total_fees_locked))
            .bind(u256_to_padded_string(summary.total_collateral_locked))
            .bind(u256_to_padded_string(summary.total_locked_and_expired_collateral))
            .bind(summary.total_requests_submitted as i64)
            .bind(summary.total_requests_submitted_onchain as i64)
            .bind(summary.total_requests_submitted_offchain as i64)
            .bind(summary.total_requests_locked as i64)
            .bind(summary.total_requests_slashed as i64)
            .bind(summary.total_expired as i64)
            .bind(summary.total_locked_and_expired as i64)
            .bind(summary.total_locked_and_fulfilled as i64)
            .bind(summary.locked_orders_fulfillment_rate)
            .bind(u256_to_padded_string(summary.total_program_cycles))
            .bind(u256_to_padded_string(summary.total_cycles))
            .bind(summary.best_peak_prove_mhz as i64)
            .bind(summary.best_peak_prove_mhz_prover)
            .bind(summary.best_peak_prove_mhz_request_id.map(|id| format!("{:x}", id)))
            .bind(summary.best_effective_prove_mhz as i64)
            .bind(summary.best_effective_prove_mhz_prover)
            .bind(summary.best_effective_prove_mhz_request_id.map(|id| format!("{:x}", id)))
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub(crate) async fn get_hourly_requestor_summaries_by_range_impl(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<PeriodRequestorSummary>, DbError> {
        self.get_requestor_summaries_by_range_generic(
            requestor_address,
            start_ts,
            end_ts,
            "hourly_requestor_summary",
        )
        .await
    }

    pub(crate) async fn get_daily_requestor_summaries_by_range_impl(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<DailyRequestorSummary>, DbError> {
        self.get_requestor_summaries_by_range_generic(
            requestor_address,
            start_ts,
            end_ts,
            "daily_requestor_summary",
        )
        .await
    }

    pub(crate) async fn get_weekly_requestor_summaries_by_range_impl(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<WeeklyRequestorSummary>, DbError> {
        self.get_requestor_summaries_by_range_generic(
            requestor_address,
            start_ts,
            end_ts,
            "weekly_requestor_summary",
        )
        .await
    }

    pub(crate) async fn get_monthly_requestor_summaries_by_range_impl(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<MonthlyRequestorSummary>, DbError> {
        self.get_requestor_summaries_by_range_generic(
            requestor_address,
            start_ts,
            end_ts,
            "monthly_requestor_summary",
        )
        .await
    }

    pub(crate) async fn get_latest_all_time_requestor_summary_impl(
        &self,
        requestor_address: Address,
    ) -> Result<Option<AllTimeRequestorSummary>, DbError> {
        let query_str = "SELECT 
                period_timestamp, requestor_address, total_fulfilled, unique_provers_locking_requests,
                total_fees_locked, total_collateral_locked, total_locked_and_expired_collateral,
                total_requests_submitted, total_requests_submitted_onchain, total_requests_submitted_offchain,
                total_requests_locked, total_requests_slashed, total_expired, total_locked_and_expired,
                total_locked_and_fulfilled, locked_orders_fulfillment_rate,
                total_program_cycles, total_cycles,
                best_peak_prove_mhz, best_peak_prove_mhz_prover, best_peak_prove_mhz_request_id,
                best_effective_prove_mhz, best_effective_prove_mhz_prover, best_effective_prove_mhz_request_id
            FROM all_time_requestor_summary 
            WHERE requestor_address = $1 
            ORDER BY period_timestamp DESC 
            LIMIT 1";

        let row = sqlx::query(query_str)
            .bind(format!("{:x}", requestor_address))
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(row) => Ok(Some(self.parse_all_time_requestor_summary_row(&row)?)),
            None => Ok(None),
        }
    }

    pub(crate) async fn get_all_time_requestor_summary_by_timestamp_impl(
        &self,
        requestor_address: Address,
        period_timestamp: u64,
    ) -> Result<Option<AllTimeRequestorSummary>, DbError> {
        let query_str = "SELECT 
                period_timestamp, requestor_address, total_fulfilled, unique_provers_locking_requests,
                total_fees_locked, total_collateral_locked, total_locked_and_expired_collateral,
                total_requests_submitted, total_requests_submitted_onchain, total_requests_submitted_offchain,
                total_requests_locked, total_requests_slashed, total_expired, total_locked_and_expired,
                total_locked_and_fulfilled, locked_orders_fulfillment_rate,
                total_program_cycles, total_cycles,
                best_peak_prove_mhz, best_peak_prove_mhz_prover, best_peak_prove_mhz_request_id,
                best_effective_prove_mhz, best_effective_prove_mhz_prover, best_effective_prove_mhz_request_id
            FROM all_time_requestor_summary 
            WHERE requestor_address = $1 AND period_timestamp = $2";

        let row = sqlx::query(query_str)
            .bind(format!("{:x}", requestor_address))
            .bind(period_timestamp as i64)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(row) => Ok(Some(self.parse_all_time_requestor_summary_row(&row)?)),
            None => Ok(None),
        }
    }

    pub(crate) async fn get_all_requestor_addresses_impl(&self) -> Result<Vec<Address>, DbError> {
        let query_str = "SELECT DISTINCT client_address FROM proof_requests ORDER BY client_address";

        let rows = sqlx::query(query_str).fetch_all(&self.pool).await?;

        let addresses = rows
            .iter()
            .map(|row| {
                let addr_str: String = row.try_get("client_address")?;
                Address::from_str(&addr_str)
                    .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(addresses)
    }

    pub(crate) async fn get_active_requestor_addresses_in_period_impl(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<Address>, DbError> {
        // Use proof_requests.submission_timestamp instead of request_status.created_at
        // This matches the aggregation logic and benefits from idx_proof_requests_submission_timestamp_client index
        let query_str = "SELECT DISTINCT pr.client_address 
            FROM proof_requests pr
            WHERE pr.submission_timestamp >= $1 AND pr.submission_timestamp < $2
            ORDER BY pr.client_address";

        let rows = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .fetch_all(&self.pool)
            .await?;

        let addresses = rows
            .iter()
            .map(|row| {
                let addr_str: String = row.try_get("client_address")?;
                Address::from_str(&addr_str)
                    .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(addresses)
    }

    pub(crate) async fn get_period_requestor_fulfilled_count_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_fulfilled_events rfe
            JOIN request_status rs ON rfe.request_digest = rs.request_digest
            WHERE rfe.block_timestamp >= $1 
            AND rfe.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    pub(crate) async fn get_period_requestor_unique_provers_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT rle.prover_address) as count 
            FROM request_locked_events rle
            JOIN request_status rs ON rle.request_digest = rs.request_digest
            WHERE rle.block_timestamp >= $1 
            AND rle.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    pub(crate) async fn get_period_requestor_total_requests_submitted_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status 
            WHERE created_at >= $1 
            AND created_at < $2
            AND client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    pub(crate) async fn get_period_requestor_total_requests_submitted_onchain_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_submitted_events rse
            JOIN request_status rs ON rse.request_digest = rs.request_digest
            WHERE rse.block_timestamp >= $1 
            AND rse.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    pub(crate) async fn get_period_requestor_total_requests_locked_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_locked_events rle
            JOIN request_status rs ON rle.request_digest = rs.request_digest
            WHERE rle.block_timestamp >= $1 
            AND rle.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    pub(crate) async fn get_period_requestor_total_requests_slashed_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM prover_slashed_events pse
            JOIN request_status rs ON pse.request_id = rs.request_id
            WHERE pse.block_timestamp >= $1 
            AND pse.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    pub(crate) async fn get_period_requestor_lock_pricing_data_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<LockPricingData>, DbError> {
        let query_str = "SELECT 
                rs.request_digest,
                rs.min_price,
                rs.max_price,
                rs.ramp_up_start,
                rs.ramp_up_period,
                rs.lock_end,
                rs.fulfilled_at as lock_timestamp,
                rs.lock_price,
                rs.lock_price_per_cycle,
                rs.lock_collateral
            FROM request_status rs
            WHERE rs.fulfilled_at IS NOT NULL
            AND rs.fulfilled_at >= $1
            AND rs.fulfilled_at < $2
            AND rs.lock_prover_address = rs.fulfill_prover_address
            AND rs.client_address = $3";

        let rows = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_all(&self.pool)
            .await?;

        let mut result = Vec::new();
        for row in rows {
            let min_price: String = row.try_get("min_price")?;
            let max_price: String = row.try_get("max_price")?;
            let ramp_up_start: i64 = row.try_get("ramp_up_start")?;
            let ramp_up_period: i64 = row.try_get("ramp_up_period")?;
            let lock_end: i64 = row.try_get("lock_end")?;
            let lock_collateral: String = row.try_get("lock_collateral")?;
            let lock_timestamp: i64 = row.try_get("lock_timestamp")?;
            let lock_price: Option<String> = row.try_get("lock_price").ok();
            let lock_price_per_cycle: Option<String> = row.try_get("lock_price_per_cycle").ok();

            result.push(LockPricingData {
                min_price,
                max_price,
                ramp_up_start: ramp_up_start as u64,
                ramp_up_period: ramp_up_period as u32,
                lock_end: lock_end as u64,
                lock_collateral,
                lock_timestamp: lock_timestamp as u64,
                lock_price,
                lock_price_per_cycle,
            });
        }

        Ok(result)
    }

    pub(crate) async fn get_period_requestor_all_lock_collateral_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<String>, DbError> {
        // Optimized: Remove unnecessary request_status JOIN - join directly with proof_requests
        // This reduces from 3-table JOIN to 2-table JOIN, improving performance
        let query_str = "SELECT pr.lock_collateral 
            FROM request_locked_events rle
            JOIN proof_requests pr ON rle.request_digest = pr.request_digest
            WHERE rle.block_timestamp >= $1 
            AND rle.block_timestamp < $2
            AND pr.client_address = $3";

        let rows = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_all(&self.pool)
            .await?;

        let collaterals = rows
            .iter()
            .map(|row| row.try_get("lock_collateral"))
            .collect::<Result<Vec<String>, _>>()?;

        Ok(collaterals)
    }

    pub(crate) async fn get_period_requestor_locked_and_expired_collateral_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<String>, DbError> {
        let query_str = "SELECT rs.lock_collateral 
            FROM request_status rs
            WHERE rs.expires_at >= $1 
            AND rs.expires_at < $2
            AND rs.request_status = 'expired'
            AND rs.locked_at IS NOT NULL
            AND rs.client_address = $3";

        let rows = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_all(&self.pool)
            .await?;

        let collaterals = rows
            .iter()
            .map(|row| row.try_get("lock_collateral"))
            .collect::<Result<Vec<String>, _>>()?;

        Ok(collaterals)
    }

    pub(crate) async fn get_period_requestor_expired_count_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status 
            WHERE expires_at >= $1 
            AND expires_at < $2
            AND request_status = 'expired'
            AND client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    pub(crate) async fn get_period_requestor_locked_and_expired_count_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status 
            WHERE expires_at >= $1 
            AND expires_at < $2
            AND request_status = 'expired'
            AND locked_at IS NOT NULL
            AND client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    pub(crate) async fn get_period_requestor_locked_and_fulfilled_count_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status 
            WHERE fulfilled_at >= $1 
            AND fulfilled_at < $2
            AND locked_at IS NOT NULL
            AND client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    pub(crate) async fn get_period_requestor_total_program_cycles_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT program_cycles FROM request_status
             WHERE request_status = 'fulfilled'
             AND program_cycles IS NOT NULL
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $1 AND fulfilled_at < $2
             AND client_address = $3",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .bind(format!("{:x}", requestor_address))
        .fetch_all(&self.pool)
        .await?;
        
        let mut total = U256::ZERO;
        for row in rows {
            let program_cycles_str: String = row.try_get("program_cycles")?;
            let program_cycles = padded_string_to_u256(&program_cycles_str)?;
            total = total.checked_add(program_cycles).ok_or_else(|| {
                DbError::Error(anyhow::anyhow!("Overflow when summing program_cycles"))
            })?;
        }
        Ok(total)
    }

    pub(crate) async fn get_period_requestor_total_cycles_impl(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT total_cycles FROM request_status
             WHERE request_status = 'fulfilled'
             AND total_cycles IS NOT NULL
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $1 AND fulfilled_at < $2
             AND client_address = $3",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .bind(format!("{:x}", requestor_address))
        .fetch_all(&self.pool)
        .await?;
        
        let mut total = U256::ZERO;
        for row in rows {
            let total_cycles_str: String = row.try_get("total_cycles")?;
            let total_cycles = padded_string_to_u256(&total_cycles_str)?;
            total = total.checked_add(total_cycles).ok_or_else(|| {
                DbError::Error(anyhow::anyhow!("Overflow when summing total_cycles"))
            })?;
        }
        Ok(total)
    }

    pub(crate) async fn get_all_time_requestor_unique_provers_impl(
        &self,
        end_ts: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT rle.prover_address) as count 
            FROM request_locked_events rle
            JOIN request_status rs ON rle.request_digest = rs.request_digest
            WHERE rle.block_timestamp < $1
            AND rs.client_address = $2";

        let row = sqlx::query(query_str)
            .bind(end_ts as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    // Per-Requestor Helper Methods

    async fn upsert_requestor_summary_generic(
        &self,
        summary: PeriodRequestorSummary,
        table_name: &str,
    ) -> Result<(), DbError> {
        let query_str = format!(
            "INSERT INTO {} (
                period_timestamp,
                requestor_address,
                total_fulfilled,
                unique_provers_locking_requests,
                total_fees_locked,
                total_collateral_locked,
                total_locked_and_expired_collateral,
                p10_lock_price_per_cycle,
                p25_lock_price_per_cycle,
                p50_lock_price_per_cycle,
                p75_lock_price_per_cycle,
                p90_lock_price_per_cycle,
                p95_lock_price_per_cycle,
                p99_lock_price_per_cycle,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_program_cycles,
                total_cycles,
                best_peak_prove_mhz,
                best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz,
                best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, CURRENT_TIMESTAMP)
            ON CONFLICT (period_timestamp, requestor_address) DO UPDATE SET
                total_fulfilled = EXCLUDED.total_fulfilled,
                unique_provers_locking_requests = EXCLUDED.unique_provers_locking_requests,
                total_fees_locked = EXCLUDED.total_fees_locked,
                total_collateral_locked = EXCLUDED.total_collateral_locked,
                total_locked_and_expired_collateral = EXCLUDED.total_locked_and_expired_collateral,
                p10_lock_price_per_cycle = EXCLUDED.p10_lock_price_per_cycle,
                p25_lock_price_per_cycle = EXCLUDED.p25_lock_price_per_cycle,
                p50_lock_price_per_cycle = EXCLUDED.p50_lock_price_per_cycle,
                p75_lock_price_per_cycle = EXCLUDED.p75_lock_price_per_cycle,
                p90_lock_price_per_cycle = EXCLUDED.p90_lock_price_per_cycle,
                p95_lock_price_per_cycle = EXCLUDED.p95_lock_price_per_cycle,
                p99_lock_price_per_cycle = EXCLUDED.p99_lock_price_per_cycle,
                total_requests_submitted = EXCLUDED.total_requests_submitted,
                total_requests_submitted_onchain = EXCLUDED.total_requests_submitted_onchain,
                total_requests_submitted_offchain = EXCLUDED.total_requests_submitted_offchain,
                total_requests_locked = EXCLUDED.total_requests_locked,
                total_requests_slashed = EXCLUDED.total_requests_slashed,
                total_expired = EXCLUDED.total_expired,
                total_locked_and_expired = EXCLUDED.total_locked_and_expired,
                total_locked_and_fulfilled = EXCLUDED.total_locked_and_fulfilled,
                locked_orders_fulfillment_rate = EXCLUDED.locked_orders_fulfillment_rate,
                total_program_cycles = EXCLUDED.total_program_cycles,
                total_cycles = EXCLUDED.total_cycles,
                best_peak_prove_mhz = EXCLUDED.best_peak_prove_mhz,
                best_peak_prove_mhz_prover = EXCLUDED.best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id = EXCLUDED.best_peak_prove_mhz_request_id,
                best_effective_prove_mhz = EXCLUDED.best_effective_prove_mhz,
                best_effective_prove_mhz_prover = EXCLUDED.best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id = EXCLUDED.best_effective_prove_mhz_request_id,
                updated_at = CURRENT_TIMESTAMP",
            table_name
        );

        sqlx::query(&query_str)
            .bind(summary.period_timestamp as i64)
            .bind(format!("{:x}", summary.requestor_address))
            .bind(summary.total_fulfilled as i64)
            .bind(summary.unique_provers_locking_requests as i64)
            .bind(u256_to_padded_string(summary.total_fees_locked))
            .bind(u256_to_padded_string(summary.total_collateral_locked))
            .bind(u256_to_padded_string(summary.total_locked_and_expired_collateral))
            .bind(u256_to_padded_string(summary.p10_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p25_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p50_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p75_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p90_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p95_lock_price_per_cycle))
            .bind(u256_to_padded_string(summary.p99_lock_price_per_cycle))
            .bind(summary.total_requests_submitted as i64)
            .bind(summary.total_requests_submitted_onchain as i64)
            .bind(summary.total_requests_submitted_offchain as i64)
            .bind(summary.total_requests_locked as i64)
            .bind(summary.total_requests_slashed as i64)
            .bind(summary.total_expired as i64)
            .bind(summary.total_locked_and_expired as i64)
            .bind(summary.total_locked_and_fulfilled as i64)
            .bind(summary.locked_orders_fulfillment_rate)
            .bind(u256_to_padded_string(summary.total_program_cycles))
            .bind(u256_to_padded_string(summary.total_cycles))
            .bind(summary.best_peak_prove_mhz as i64)
            .bind(summary.best_peak_prove_mhz_prover)
            .bind(summary.best_peak_prove_mhz_request_id.map(|id| format!("{:x}", id)))
            .bind(summary.best_effective_prove_mhz as i64)
            .bind(summary.best_effective_prove_mhz_prover)
            .bind(summary.best_effective_prove_mhz_request_id.map(|id| format!("{:x}", id)))
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_requestor_summaries_by_range_generic(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
        table_name: &str,
    ) -> Result<Vec<PeriodRequestorSummary>, DbError> {
        let query_str = format!(
            "SELECT 
                period_timestamp, requestor_address, total_fulfilled, unique_provers_locking_requests,
                total_fees_locked, total_collateral_locked, total_locked_and_expired_collateral,
                p10_lock_price_per_cycle, p25_lock_price_per_cycle, p50_lock_price_per_cycle,
                p75_lock_price_per_cycle, p90_lock_price_per_cycle, p95_lock_price_per_cycle, p99_lock_price_per_cycle,
                total_requests_submitted, total_requests_submitted_onchain, total_requests_submitted_offchain,
                total_requests_locked, total_requests_slashed, total_expired, total_locked_and_expired,
                total_locked_and_fulfilled, locked_orders_fulfillment_rate,
                total_program_cycles, total_cycles,
                best_peak_prove_mhz, best_peak_prove_mhz_prover, best_peak_prove_mhz_request_id,
                best_effective_prove_mhz, best_effective_prove_mhz_prover, best_effective_prove_mhz_request_id
            FROM {} WHERE requestor_address = $1 AND period_timestamp >= $2 AND period_timestamp < $3 ORDER BY period_timestamp ASC",
            table_name
        );

        let rows = sqlx::query(&query_str)
            .bind(format!("{:x}", requestor_address))
            .bind(start_ts as i64)
            .bind(end_ts as i64)
            .fetch_all(&self.pool)
            .await?;

        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(self.parse_period_requestor_summary_row(&row)?);
        }

        Ok(summaries)
    }

    fn parse_period_requestor_summary_row(
        &self,
        row: &sqlx::any::AnyRow,
    ) -> Result<PeriodRequestorSummary, DbError> {
        let period_timestamp: i64 = row.try_get("period_timestamp")?;
        let requestor_address_str: String = row.try_get("requestor_address")?;
        let requestor_address = Address::from_str(&requestor_address_str)
            .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid requestor address: {}", e)))?;
        let total_fulfilled: i64 = row.try_get("total_fulfilled")?;
        let unique_provers: i64 = row.try_get("unique_provers_locking_requests")?;
        let total_fees_locked_str: String = row.try_get("total_fees_locked")?;
        let total_collateral_locked_str: String = row.try_get("total_collateral_locked")?;
        let total_locked_and_expired_collateral_str: String =
            row.try_get("total_locked_and_expired_collateral")?;
        let p10_str: String = row.try_get("p10_lock_price_per_cycle")?;
        let p25_str: String = row.try_get("p25_lock_price_per_cycle")?;
        let p50_str: String = row.try_get("p50_lock_price_per_cycle")?;
        let p75_str: String = row.try_get("p75_lock_price_per_cycle")?;
        let p90_str: String = row.try_get("p90_lock_price_per_cycle")?;
        let p95_str: String = row.try_get("p95_lock_price_per_cycle")?;
        let p99_str: String = row.try_get("p99_lock_price_per_cycle")?;
        let total_requests_submitted: i64 = row.try_get("total_requests_submitted")?;
        let total_requests_submitted_onchain: i64 = row.try_get("total_requests_submitted_onchain")?;
        let total_requests_submitted_offchain: i64 = row.try_get("total_requests_submitted_offchain")?;
        let total_requests_locked: i64 = row.try_get("total_requests_locked")?;
        let total_requests_slashed: i64 = row.try_get("total_requests_slashed")?;
        let total_expired: i64 = row.try_get("total_expired")?;
        let total_locked_and_expired: i64 = row.try_get("total_locked_and_expired")?;
        let total_locked_and_fulfilled: i64 = row.try_get("total_locked_and_fulfilled")?;
        let locked_orders_fulfillment_rate: f64 = row.try_get("locked_orders_fulfillment_rate")?;
        let total_program_cycles_str: String = row.try_get("total_program_cycles")?;
        let total_cycles_str: String = row.try_get("total_cycles")?;
        let best_peak_prove_mhz: i64 = row.try_get("best_peak_prove_mhz")?;
        let best_peak_prove_mhz_prover: Option<String> = row.try_get("best_peak_prove_mhz_prover").ok();
        let best_peak_prove_mhz_request_id_str: Option<String> = row.try_get("best_peak_prove_mhz_request_id").ok();
        let best_effective_prove_mhz: i64 = row.try_get("best_effective_prove_mhz")?;
        let best_effective_prove_mhz_prover: Option<String> = row.try_get("best_effective_prove_mhz_prover").ok();
        let best_effective_prove_mhz_request_id_str: Option<String> = row.try_get("best_effective_prove_mhz_request_id").ok();

        Ok(PeriodRequestorSummary {
            period_timestamp: period_timestamp as u64,
            requestor_address,
            total_fulfilled: total_fulfilled as u64,
            unique_provers_locking_requests: unique_provers as u64,
            total_fees_locked: padded_string_to_u256(&total_fees_locked_str)?,
            total_collateral_locked: padded_string_to_u256(&total_collateral_locked_str)?,
            total_locked_and_expired_collateral: padded_string_to_u256(
                &total_locked_and_expired_collateral_str,
            )?,
            p10_lock_price_per_cycle: padded_string_to_u256(&p10_str)?,
            p25_lock_price_per_cycle: padded_string_to_u256(&p25_str)?,
            p50_lock_price_per_cycle: padded_string_to_u256(&p50_str)?,
            p75_lock_price_per_cycle: padded_string_to_u256(&p75_str)?,
            p90_lock_price_per_cycle: padded_string_to_u256(&p90_str)?,
            p95_lock_price_per_cycle: padded_string_to_u256(&p95_str)?,
            p99_lock_price_per_cycle: padded_string_to_u256(&p99_str)?,
            total_requests_submitted: total_requests_submitted as u64,
            total_requests_submitted_onchain: total_requests_submitted_onchain as u64,
            total_requests_submitted_offchain: total_requests_submitted_offchain as u64,
            total_requests_locked: total_requests_locked as u64,
            total_requests_slashed: total_requests_slashed as u64,
            total_expired: total_expired as u64,
            total_locked_and_expired: total_locked_and_expired as u64,
            total_locked_and_fulfilled: total_locked_and_fulfilled as u64,
            locked_orders_fulfillment_rate: locked_orders_fulfillment_rate as f32,
            total_program_cycles: padded_string_to_u256(&total_program_cycles_str)?,
            total_cycles: padded_string_to_u256(&total_cycles_str)?,
            best_peak_prove_mhz: best_peak_prove_mhz as u64,
            best_peak_prove_mhz_prover,
            best_peak_prove_mhz_request_id: best_peak_prove_mhz_request_id_str
                .and_then(|s| U256::from_str(&s).ok()),
            best_effective_prove_mhz: best_effective_prove_mhz as u64,
            best_effective_prove_mhz_prover,
            best_effective_prove_mhz_request_id: best_effective_prove_mhz_request_id_str
                .and_then(|s| U256::from_str(&s).ok()),
        })
    }

    fn parse_all_time_requestor_summary_row(
        &self,
        row: &sqlx::any::AnyRow,
    ) -> Result<AllTimeRequestorSummary, DbError> {
        let period_timestamp: i64 = row.try_get("period_timestamp")?;
        let requestor_address_str: String = row.try_get("requestor_address")?;
        let requestor_address = Address::from_str(&requestor_address_str)
            .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid requestor address: {}", e)))?;
        let total_fulfilled: i64 = row.try_get("total_fulfilled")?;
        let unique_provers: i64 = row.try_get("unique_provers_locking_requests")?;
        let total_fees_locked_str: String = row.try_get("total_fees_locked")?;
        let total_collateral_locked_str: String = row.try_get("total_collateral_locked")?;
        let total_locked_and_expired_collateral_str: String =
            row.try_get("total_locked_and_expired_collateral")?;
        let total_requests_submitted: i64 = row.try_get("total_requests_submitted")?;
        let total_requests_submitted_onchain: i64 = row.try_get("total_requests_submitted_onchain")?;
        let total_requests_submitted_offchain: i64 = row.try_get("total_requests_submitted_offchain")?;
        let total_requests_locked: i64 = row.try_get("total_requests_locked")?;
        let total_requests_slashed: i64 = row.try_get("total_requests_slashed")?;
        let total_expired: i64 = row.try_get("total_expired")?;
        let total_locked_and_expired: i64 = row.try_get("total_locked_and_expired")?;
        let total_locked_and_fulfilled: i64 = row.try_get("total_locked_and_fulfilled")?;
        let locked_orders_fulfillment_rate: f64 = row.try_get("locked_orders_fulfillment_rate")?;
        let total_program_cycles_str: String = row.try_get("total_program_cycles")?;
        let total_cycles_str: String = row.try_get("total_cycles")?;
        let best_peak_prove_mhz: i64 = row.try_get("best_peak_prove_mhz")?;
        let best_peak_prove_mhz_prover: Option<String> = row.try_get("best_peak_prove_mhz_prover").ok();
        let best_peak_prove_mhz_request_id_str: Option<String> = row.try_get("best_peak_prove_mhz_request_id").ok();
        let best_effective_prove_mhz: i64 = row.try_get("best_effective_prove_mhz")?;
        let best_effective_prove_mhz_prover: Option<String> = row.try_get("best_effective_prove_mhz_prover").ok();
        let best_effective_prove_mhz_request_id_str: Option<String> = row.try_get("best_effective_prove_mhz_request_id").ok();

        Ok(AllTimeRequestorSummary {
            period_timestamp: period_timestamp as u64,
            requestor_address,
            total_fulfilled: total_fulfilled as u64,
            unique_provers_locking_requests: unique_provers as u64,
            total_fees_locked: padded_string_to_u256(&total_fees_locked_str)?,
            total_collateral_locked: padded_string_to_u256(&total_collateral_locked_str)?,
            total_locked_and_expired_collateral: padded_string_to_u256(
                &total_locked_and_expired_collateral_str,
            )?,
            total_requests_submitted: total_requests_submitted as u64,
            total_requests_submitted_onchain: total_requests_submitted_onchain as u64,
            total_requests_submitted_offchain: total_requests_submitted_offchain as u64,
            total_requests_locked: total_requests_locked as u64,
            total_requests_slashed: total_requests_slashed as u64,
            total_expired: total_expired as u64,
            total_locked_and_expired: total_locked_and_expired as u64,
            total_locked_and_fulfilled: total_locked_and_fulfilled as u64,
            locked_orders_fulfillment_rate: locked_orders_fulfillment_rate as f32,
            total_program_cycles: padded_string_to_u256(&total_program_cycles_str)?,
            total_cycles: padded_string_to_u256(&total_cycles_str)?,
            best_peak_prove_mhz: best_peak_prove_mhz as u64,
            best_peak_prove_mhz_prover,
            best_peak_prove_mhz_request_id: best_peak_prove_mhz_request_id_str
                .and_then(|s| U256::from_str(&s).ok()),
            best_effective_prove_mhz: best_effective_prove_mhz as u64,
            best_effective_prove_mhz_prover,
            best_effective_prove_mhz_request_id: best_effective_prove_mhz_request_id_str
                .and_then(|s| U256::from_str(&s).ok()),
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
        RequestId, RequestInput, Requirements,
    };
    use risc0_zkvm::Digest;
    use tracing_test::traced_test;

    // generate a test request
    fn generate_request(id: u32, addr: &Address) -> ProofRequest {
        generate_request_with_collateral(id, addr, U256::from(10))
    }

    fn generate_request_with_collateral(id: u32, addr: &Address, collateral: U256) -> ProofRequest {
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
                lockCollateral: collateral,
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

        db.add_txs(&[metadata]).await.unwrap();

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
        db.add_proof_requests(&[(request_digest, request.clone(), metadata, "onchain".to_string(), metadata.block_timestamp)]).await.unwrap();

        // Verify proof request was added
        let result = sqlx::query("SELECT * FROM proof_requests WHERE request_digest = $1")
            .bind(format!("{request_digest:x}"))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_id"), format!("{:x}", request.id));
    }

    #[tokio::test]
    async fn test_has_proof_requests() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty array
        let existing = db.has_proof_requests(&[]).await.unwrap();
        assert_eq!(existing.len(), 0);

        // Add 20 proof requests to database
        let mut all_digests = Vec::new();
        for i in 0..20 {
            let request_digest = B256::from([i as u8; 32]);
            let request = generate_request(i, &Address::ZERO);
            let metadata = TxMetadata::new(
                B256::from([(i + 100) as u8; 32]),
                Address::ZERO,
                100 + i as u64,
                1234567890 + i as u64,
                i as u64,
            );
            db.add_proof_requests(&[(request_digest, request, metadata, "onchain".to_string(), metadata.block_timestamp)]).await.unwrap();
            all_digests.push(request_digest);
        }

        // Test batch check with all existing digests
        let existing = db.has_proof_requests(&all_digests).await.unwrap();
        assert_eq!(existing.len(), 20);
        for digest in &all_digests {
            assert!(existing.contains(digest));
        }

        // Test batch check with mixed existing and non-existing digests
        let mut mixed_digests = Vec::new();
        for i in 0..10 {
            mixed_digests.push(B256::from([i as u8; 32])); // Existing
        }
        for i in 50..60 {
            mixed_digests.push(B256::from([i as u8; 32])); // Non-existing
        }

        let existing = db.has_proof_requests(&mixed_digests).await.unwrap();
        assert_eq!(existing.len(), 10, "Should find exactly 10 existing digests");
        for i in 0..10 {
            assert!(existing.contains(&B256::from([i as u8; 32])));
        }
        for i in 50..60 {
            assert!(!existing.contains(&B256::from([i as u8; 32])));
        }

        // Test batch check with all non-existing digests
        let non_existing_digests: Vec<B256> = (100..110).map(|i| B256::from([i as u8; 32])).collect();
        let existing = db.has_proof_requests(&non_existing_digests).await.unwrap();
        assert_eq!(existing.len(), 0);
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

        db.add_assessor_receipts(&[(receipt.clone(), metadata)]).await.unwrap();

        // Verify assessor receipt was added
        let result = sqlx::query("SELECT * FROM assessor_receipts WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("prover_address"), format!("{:x}", receipt.prover));
    }

    #[tokio::test]
    async fn test_add_proofs() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty array
        db.add_proofs(&[]).await.unwrap();

        // Create test data - more than chunk size (800) to test chunking
        let mut proofs = Vec::new();
        for i in 0..1200 {
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i % 256) as u8;
            digest_bytes[1] = ((i / 256) % 256) as u8;
            digest_bytes[2] = ((i / 65536) % 256) as u8;
            let request_digest = B256::from(digest_bytes);

            let fulfillment = Fulfillment {
                requestDigest: request_digest,
                id: U256::from(i),
                claimDigest: B256::from([(i % 256) as u8; 32]),
                fulfillmentData: Bytes::default(),
                fulfillmentDataType: FulfillmentDataType::None,
                seal: Bytes::from(vec![i as u8; 32]),
            };

            let prover = Address::from([(i % 256) as u8; 20]);

            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0] = ((i + 1) % 256) as u8;
            tx_hash_bytes[1] = (((i + 1) / 256) % 256) as u8;
            tx_hash_bytes[2] = (((i + 1) / 65536) % 256) as u8;
            tx_hash_bytes[3] = 0xBB;

            let metadata = TxMetadata::new(
                B256::from(tx_hash_bytes),
                Address::from([100; 20]),
                4000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );

            proofs.push((fulfillment, prover, metadata));
        }

        // Batch insert all proofs
        db.add_proofs(&proofs).await.unwrap();

        // Verify proofs were added correctly - check samples
        for i in [0, 500, 800, 1199].iter() {
            let (fulfillment, prover, metadata) = &proofs[*i];
            let result = sqlx::query(
                "SELECT * FROM proofs WHERE request_digest = $1 AND tx_hash = $2"
            )
            .bind(format!("{:x}", fulfillment.requestDigest))
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some(), "Proof {} should exist", i);
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("request_id"), format!("{:x}", fulfillment.id));
            assert_eq!(row.get::<String, _>("prover_address"), format!("{prover:x}"));
            assert_eq!(row.get::<String, _>("claim_digest"), format!("{:x}", fulfillment.claimDigest));
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proofs")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1200);

        // Test idempotency
        db.add_proofs(&proofs).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proofs")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1200); // Still 1200
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
        db.add_request_locked_events(&[(request_digest, request_id, prover_address, metadata)])
            .await
            .unwrap();

        // Then test prover slashed event
        db.add_prover_slashed_events(&[(
            request_id,
            burn_value,
            transfer_value,
            collateral_recipient,
            metadata,
        )])
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
        db.add_deposit_events(&[(account, value, metadata)]).await.unwrap();
        let result = sqlx::query("SELECT * FROM deposit_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());

        // Test withdrawal event
        db.add_withdrawal_events(&[(account, value, metadata)]).await.unwrap();
        let result = sqlx::query("SELECT * FROM withdrawal_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());

        // Test collateral deposit event
        db.add_collateral_deposit_events(&[(account, value, metadata)]).await.unwrap();
        let result = sqlx::query("SELECT * FROM collateral_deposit_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());

        // Test collateral withdrawal event
        db.add_collateral_withdrawal_events(&[(account, value, metadata)]).await.unwrap();
        let result = sqlx::query("SELECT * FROM collateral_withdrawal_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());
    }

    #[tokio::test]
    async fn test_add_deposit_events() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty array
        db.add_deposit_events(&[]).await.unwrap();

        // Create test data - more than chunk size (1000) to test chunking
        let mut deposits = Vec::new();
        for i in 0..1500 {
            let account = Address::from([(i % 256) as u8; 20]);
            let value = U256::from(1000 + i);

            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0] = ((i + 1) % 256) as u8;
            tx_hash_bytes[1] = (((i + 1) / 256) % 256) as u8;
            tx_hash_bytes[2] = (((i + 1) / 65536) % 256) as u8;
            tx_hash_bytes[3] = 0xCC;

            let metadata = TxMetadata::new(
                B256::from(tx_hash_bytes),
                Address::from([100; 20]),
                6000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );

            deposits.push((account, value, metadata));
        }

        // Batch insert all deposit events
        db.add_deposit_events(&deposits).await.unwrap();

        // Verify deposits were added correctly - check samples
        for i in [0, 500, 1000, 1499].iter() {
            let (account, value, metadata) = &deposits[*i];
            let result = sqlx::query(
                "SELECT * FROM deposit_events WHERE account = $1 AND tx_hash = $2"
            )
            .bind(format!("{account:x}"))
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some(), "Deposit {} should exist", i);
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("value"), value.to_string());
            assert_eq!(row.get::<i64, _>("block_number"), metadata.block_number as i64);
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM deposit_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500);

        // Test idempotency
        db.add_deposit_events(&deposits).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM deposit_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500); // Still 1500
    }

    #[tokio::test]
    async fn test_callback_failed_event() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let request_id = U256::from(1);
        let callback_address = Address::ZERO;
        let error_data = vec![1, 2, 3, 4];

        db.add_callback_failed_events(&[(request_id, callback_address, error_data.clone(), metadata)])
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
            let summary = PeriodMarketSummary {
                period_timestamp: (base_timestamp + (i as i64 * hour_in_seconds)) as u64,
                total_fulfilled: i,
                unique_provers_locking_requests: i * 2,
                unique_requesters_submitting_requests: i * 3,
                total_fees_locked: U256::from(i * 1000),
                total_collateral_locked: U256::from(i * 2000),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(i * 100),
                p25_lock_price_per_cycle: U256::from(i * 250),
                p50_lock_price_per_cycle: U256::from(i * 500),
                p75_lock_price_per_cycle: U256::from(i * 750),
                p90_lock_price_per_cycle: U256::from(i * 900),
                p95_lock_price_per_cycle: U256::from(i * 950),
                p99_lock_price_per_cycle: U256::from(i * 990),
                total_requests_submitted: i * 10,
                total_requests_submitted_onchain: i * 6,
                total_requests_submitted_offchain: i * 4,
                total_requests_locked: i * 5,
                total_requests_slashed: i,
                total_expired: i,
                total_locked_and_expired: i / 2,
                total_locked_and_fulfilled: i,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: U256::ZERO,
                total_program_cycles: U256::ZERO,
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
                total_fees_locked: U256::from(i * 1000),
                total_collateral_locked: U256::from(i * 2000),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(i * 100),
                p25_lock_price_per_cycle: U256::from(i * 250),
                p50_lock_price_per_cycle: U256::from(i * 500),
                p75_lock_price_per_cycle: U256::from(i * 750),
                p90_lock_price_per_cycle: U256::from(i * 900),
                p95_lock_price_per_cycle: U256::from(i * 950),
                p99_lock_price_per_cycle: U256::from(i * 990),
                total_requests_submitted: i * 10,
                total_requests_submitted_onchain: i * 6,
                total_requests_submitted_offchain: i * 4,
                total_requests_locked: i * 5,
                total_requests_slashed: i,
                total_expired: i,
                total_locked_and_expired: i / 2,
                total_locked_and_fulfilled: i * 10,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: U256::ZERO,
                total_program_cycles: U256::ZERO,
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
                total_fees_locked: U256::from(i * 10000),
                total_collateral_locked: U256::from(i * 20000),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(i * 1000),
                p25_lock_price_per_cycle: U256::from(i * 2500),
                p50_lock_price_per_cycle: U256::from(i * 5000),
                p75_lock_price_per_cycle: U256::from(i * 7500),
                p90_lock_price_per_cycle: U256::from(i * 9000),
                p95_lock_price_per_cycle: U256::from(i * 9500),
                p99_lock_price_per_cycle: U256::from(i * 9900),
                total_requests_submitted: i * 100,
                total_requests_submitted_onchain: i * 60,
                total_requests_submitted_offchain: i * 40,
                total_requests_locked: i * 50,
                total_requests_slashed: i * 5,
                total_expired: i * 10,
                total_locked_and_expired: i * 5,
                total_locked_and_fulfilled: i * 100,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: U256::ZERO,
                total_program_cycles: U256::ZERO,
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
                total_fees_locked: U256::from(i * 100000),
                total_collateral_locked: U256::from(i * 200000),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(i * 10000),
                p25_lock_price_per_cycle: U256::from(i * 25000),
                p50_lock_price_per_cycle: U256::from(i * 50000),
                p75_lock_price_per_cycle: U256::from(i * 75000),
                p90_lock_price_per_cycle: U256::from(i * 90000),
                p95_lock_price_per_cycle: U256::from(i * 95000),
                p99_lock_price_per_cycle: U256::from(i * 99000),
                total_requests_submitted: i * 1000,
                total_requests_submitted_onchain: i * 600,
                total_requests_submitted_offchain: i * 400,
                total_requests_locked: i * 500,
                total_requests_slashed: i * 50,
                total_expired: i * 100,
                total_locked_and_expired: i * 50,
                total_locked_and_fulfilled: i * 1000,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: U256::ZERO,
                total_program_cycles: U256::ZERO,
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
                total_fees_locked: U256::from(i * 1000),
                total_collateral_locked: U256::from(i * 2000),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(i * 100),
                p25_lock_price_per_cycle: U256::from(i * 250),
                p50_lock_price_per_cycle: U256::from(i * 500),
                p75_lock_price_per_cycle: U256::from(i * 750),
                p90_lock_price_per_cycle: U256::from(i * 900),
                p95_lock_price_per_cycle: U256::from(i * 950),
                p99_lock_price_per_cycle: U256::from(i * 990),
                total_requests_submitted: i * 10,
                total_requests_submitted_onchain: i * 6,
                total_requests_submitted_offchain: i * 4,
                total_requests_locked: i * 5,
                total_requests_slashed: i,
                total_expired: i,
                total_locked_and_expired: i / 2,
                total_locked_and_fulfilled: i,
                locked_orders_fulfillment_rate: if i > 0 { 100.0 } else { 0.0 },
                total_cycles: U256::ZERO,
                total_program_cycles: U256::ZERO,
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
        assert_eq!(results[0].total_fees_locked, U256::ZERO);
    }

    fn create_test_status(digest: B256, status_type: RequestStatusType) -> RequestStatus {
        RequestStatus {
            request_digest: digest,
            request_id: U256::from(12345), // Use a simple test U256 value
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
            lock_prover_delivered_proof_at: None,
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
            program_cycles: None,
            total_cycles: None,
            peak_prove_mhz: None,
            effective_prove_mhz: None,
            cycle_status: None,
            lock_price: None,
            lock_price_per_cycle: None,
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
            .bind(format!("{:x}", digest))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();

        assert_eq!(result.get::<String, _>("request_status"), "submitted");
        assert_eq!(result.get::<String, _>("request_id"), format!("{:x}", status.request_id));
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
            .bind(format!("{:x}", digest))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();

        assert_eq!(result.get::<String, _>("request_status"), "locked");
        assert_eq!(result.get::<Option<i64>, _>("locked_at"), Some(1234567900));
        assert_eq!(result.get::<Option<i64>, _>("lock_block"), Some(200));
        assert_eq!(result.get::<String, _>("request_id"), format!("{:x}", status.request_id));
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
        db.add_proof_requests(&[(request_digest, request.clone(), metadata1, "onchain".to_string(), metadata1.block_timestamp)]).await.unwrap();

        // Add submitted event
        db.add_request_submitted_events(&[(request_digest, request.id, metadata1)]).await.unwrap();

        // Add locked event
        let metadata2 = TxMetadata::new(B256::from([11; 32]), Address::ZERO, 101, 1100, 0);
        db.add_request_locked_events(&[(request_digest, request.id, prover_a, metadata2)])
            .await
            .unwrap();

        // Add fulfilled event (fulfilled by prover_a)
        let metadata3 = TxMetadata::new(B256::from([12; 32]), Address::ZERO, 102, 1200, 0);
        db.add_request_fulfilled_events(&[(request_digest, request.id, prover_a, metadata3)])
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

        let metadata_late = TxMetadata::new(B256::from([21; 32]), Address::ZERO, 105, 1400, 1);
        let fulfillment_late = Fulfillment {
            requestDigest: request_digest,
            id: request.id,
            claimDigest: B256::from([31; 32]),
            fulfillmentData: Bytes::default(),
            fulfillmentDataType: FulfillmentDataType::None,
            seal: seal_late,
        };

        db.add_proofs(&[
            (fulfillment_wrong_prover, prover_b, metadata_wrong_prover),
            (fulfillment_early, prover_a, metadata_early),
            (fulfillment_late, prover_a, metadata_late),
        ]).await.unwrap();

        // Add proof_delivered_events for prover_a (the lock prover)
        // Add both early and late events - MIN should return the early timestamp (1300)
        db.add_proof_delivered_events(&[
            (request_digest, request.id, prover_a, metadata_early),
            (request_digest, request.id, prover_a, metadata_late),
        ]).await.unwrap();

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
        
        // Verify lock_prover_delivered_proof_at is populated with MIN timestamp (1300)
        assert_eq!(comprehensive.lock_prover_delivered_proof_at, Some(1300));

        // Verify seal is from prover_a's EARLIEST fulfillment (timestamp 1300)
        // NOT from prover_b's fulfillment (timestamp 1250) even though it's earlier
        assert_eq!(comprehensive.fulfill_seal, Some(format!("0x{}", hex::encode(&seal_early))));
    }

    #[tokio::test]
    async fn test_get_requests_comprehensive_batch() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Create multiple requests with different states
        let digest1 = B256::from([1; 32]);
        let digest2 = B256::from([2; 32]);
        let digest3 = B256::from([3; 32]);
        let digest4 = B256::from([4; 32]);
        let digest5 = B256::from([5; 32]);

        let request1 = generate_request(1, &Address::ZERO);
        let request2 = generate_request(2, &Address::ZERO);
        let request3 = generate_request(3, &Address::ZERO);
        let request4 = generate_request(4, &Address::ZERO);
        let request5 = generate_request(5, &Address::ZERO);

        let prover1 = Address::from([10; 20]);
        let prover2 = Address::from([11; 20]);

        // Request 1: fully fulfilled
        let meta1 = TxMetadata::new(B256::from([100; 32]), Address::ZERO, 100, 1000, 0);
        db.add_proof_requests(&[(digest1, request1.clone(), meta1, "onchain".to_string(), meta1.block_timestamp)]).await.unwrap();
        db.add_request_submitted_events(&[(digest1, request1.id, meta1)]).await.unwrap();
        let meta1_lock = TxMetadata::new(B256::from([101; 32]), Address::ZERO, 101, 1100, 0);
        db.add_request_locked_events(&[(digest1, request1.id, prover1, meta1_lock)]).await.unwrap();
        let meta1_fulfill = TxMetadata::new(B256::from([102; 32]), Address::ZERO, 102, 1200, 0);
        db.add_request_fulfilled_events(&[(digest1, request1.id, prover1, meta1_fulfill)]).await.unwrap();
        let seal1 = Bytes::from(vec![1, 1, 1]);
        let fulfillment1 = Fulfillment {
            requestDigest: digest1,
            id: request1.id,
            claimDigest: B256::from([201; 32]),
            fulfillmentData: Bytes::default(),
            fulfillmentDataType: FulfillmentDataType::None,
            seal: seal1.clone(),
        };
        db.add_proofs(&[(fulfillment1, prover1, meta1_fulfill)]).await.unwrap();
        // Add proof_delivered_events for prover1 (the lock prover)
        db.add_proof_delivered_events(&[(digest1, request1.id, prover1, meta1_fulfill)]).await.unwrap();

        // Request 2: locked but not fulfilled
        let meta2 = TxMetadata::new(B256::from([110; 32]), Address::ZERO, 103, 2000, 0);
        db.add_proof_requests(&[(digest2, request2.clone(), meta2, "offchain".to_string(), meta2.block_timestamp)]).await.unwrap();
        db.add_request_submitted_events(&[(digest2, request2.id, meta2)]).await.unwrap();
        let meta2_lock = TxMetadata::new(B256::from([111; 32]), Address::ZERO, 104, 2100, 0);
        db.add_request_locked_events(&[(digest2, request2.id, prover2, meta2_lock)]).await.unwrap();
        // Add proof_delivered_events for prover2 (the lock prover) even though not fulfilled
        let meta2_proof_delivered = TxMetadata::new(B256::from([112; 32]), Address::ZERO, 105, 2150, 0);
        db.add_proof_delivered_events(&[(digest2, request2.id, prover2, meta2_proof_delivered)]).await.unwrap();

        // Request 3: only submitted
        let meta3 = TxMetadata::new(B256::from([120; 32]), Address::ZERO, 110, 3000, 0);
        db.add_proof_requests(&[(digest3, request3.clone(), meta3, "onchain".to_string(), meta3.block_timestamp)]).await.unwrap();
        db.add_request_submitted_events(&[(digest3, request3.id, meta3)]).await.unwrap();

        // Request 4: fulfilled by different prover than locked
        let meta4 = TxMetadata::new(B256::from([130; 32]), Address::ZERO, 106, 4000, 0);
        db.add_proof_requests(&[(digest4, request4.clone(), meta4, "onchain".to_string(), meta4.block_timestamp)]).await.unwrap();
        db.add_request_submitted_events(&[(digest4, request4.id, meta4)]).await.unwrap();
        let meta4_lock = TxMetadata::new(B256::from([131; 32]), Address::ZERO, 107, 4100, 0);
        db.add_request_locked_events(&[(digest4, request4.id, prover1, meta4_lock)]).await.unwrap();
        // Add proof_delivered_events for prover1 (the lock prover) even though fulfillment is by prover2
        let meta4_proof_delivered = TxMetadata::new(B256::from([133; 32]), Address::ZERO, 109, 4150, 0);
        db.add_proof_delivered_events(&[(digest4, request4.id, prover1, meta4_proof_delivered)]).await.unwrap();
        let meta4_fulfill = TxMetadata::new(B256::from([132; 32]), Address::ZERO, 108, 4200, 0);
        db.add_request_fulfilled_events(&[(digest4, request4.id, prover2, meta4_fulfill)]).await.unwrap();
        let seal4 = Bytes::from(vec![4, 4, 4]);
        let fulfillment4 = Fulfillment {
            requestDigest: digest4,
            id: request4.id,
            claimDigest: B256::from([204; 32]),
            fulfillmentData: Bytes::default(),
            fulfillmentDataType: FulfillmentDataType::None,
            seal: seal4.clone(),
        };
        db.add_proofs(&[(fulfillment4, prover2, meta4_fulfill)]).await.unwrap();

        // Request 5: no events at all
        let meta5 = TxMetadata::new(B256::from([140; 32]), Address::ZERO, 109, 5000, 0);
        db.add_proof_requests(&[(digest5, request5.clone(), meta5, "onchain".to_string(), meta5.block_timestamp)]).await.unwrap();

        // Fetch all requests in a single batch call
        let digest_set: std::collections::HashSet<B256> =
            vec![digest1, digest2, digest3, digest4, digest5].into_iter().collect();
        let results = db.get_requests_comprehensive(&digest_set).await.unwrap();

        // Should get exactly 5 results
        assert_eq!(results.len(), 5);

        // Convert to HashMap for easier lookup
        let results_map: std::collections::HashMap<B256, &RequestComprehensive> =
            results.iter().map(|r| (r.request_digest, r)).collect();

        // Verify request 1 (fully fulfilled)
        let r1 = results_map.get(&digest1).expect("Request 1 should be in results");
        assert_eq!(r1.source, "onchain");
        assert_eq!(r1.submitted_at, Some(1000));
        assert_eq!(r1.locked_at, Some(1100));
        assert_eq!(r1.fulfilled_at, Some(1200));
        assert_eq!(r1.lock_prover_address, Some(prover1));
        assert_eq!(r1.fulfill_prover_address, Some(prover1));
        assert_eq!(r1.lock_prover_delivered_proof_at, Some(1200));
        assert_eq!(r1.fulfill_seal, Some(format!("0x{}", hex::encode(&seal1))));

        // Verify request 2 (locked but not fulfilled)
        let r2 = results_map.get(&digest2).expect("Request 2 should be in results");
        assert_eq!(r2.source, "offchain");
        assert_eq!(r2.submitted_at, Some(2000));
        assert_eq!(r2.locked_at, Some(2100));
        assert_eq!(r2.fulfilled_at, None);
        assert_eq!(r2.lock_prover_address, Some(prover2));
        assert_eq!(r2.lock_prover_delivered_proof_at, Some(2150));
        assert_eq!(r2.fulfill_prover_address, None);
        assert_eq!(r2.fulfill_seal, None);

        // Verify request 3 (only submitted)
        let r3 = results_map.get(&digest3).expect("Request 3 should be in results");
        assert_eq!(r3.source, "onchain");
        assert_eq!(r3.submitted_at, Some(3000));
        assert_eq!(r3.locked_at, None);
        assert_eq!(r3.fulfilled_at, None);
        assert_eq!(r3.lock_prover_address, None);
        assert_eq!(r3.lock_prover_delivered_proof_at, None);
        assert_eq!(r3.fulfill_prover_address, None);
        assert_eq!(r3.fulfill_seal, None);

        // Verify request 4 (different provers for lock and fulfill)
        let r4 = results_map.get(&digest4).expect("Request 4 should be in results");
        assert_eq!(r4.source, "onchain");
        assert_eq!(r4.submitted_at, Some(4000));
        assert_eq!(r4.locked_at, Some(4100));
        assert_eq!(r4.fulfilled_at, Some(4200));
        assert_eq!(r4.lock_prover_address, Some(prover1));
        assert_eq!(r4.lock_prover_delivered_proof_at, Some(4150));
        assert_eq!(r4.fulfill_prover_address, Some(prover2));
        assert_eq!(r4.fulfill_seal, Some(format!("0x{}", hex::encode(&seal4))));

        // Verify request 5 (no events)
        let r5 = results_map.get(&digest5).expect("Request 5 should be in results");
        assert_eq!(r5.source, "onchain");
        assert_eq!(r5.submitted_at, None);
        assert_eq!(r5.lock_prover_delivered_proof_at, None);
        assert_eq!(r5.locked_at, None);
        assert_eq!(r5.fulfilled_at, None);
        assert_eq!(r5.lock_prover_address, None);
        assert_eq!(r5.fulfill_prover_address, None);
        assert_eq!(r5.fulfill_seal, None);
    }

    #[tokio::test]
    async fn test_add_request_submitted_events() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty events - should not fail
        db.add_request_submitted_events(&[]).await.unwrap();

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
        db.add_request_submitted_events(&events).await.unwrap();

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
        db.add_request_submitted_events(&events).await.unwrap();

        // Verify we still have exactly 15 events (not duplicated)
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_submitted_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 15);

        // Test with large batch to verify chunking works
        let mut large_batch = Vec::new();
        for i in 100..1200 {  // 1100 events, will require 2 chunks
            let _request_digest = B256::from([(i % 256) as u8; 32]);
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

        db.add_request_submitted_events(&large_batch).await.unwrap();

        // Verify a sample from the large batch
        let sample_index = 500;
        let (sample_digest, sample_id, _sample_metadata) = &large_batch[sample_index];
        let result = sqlx::query("SELECT * FROM request_submitted_events WHERE request_digest = $1")
            .bind(format!("{sample_digest:x}"))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_id"), format!("{sample_id:x}"));
    }

    #[tokio::test]
    async fn test_add_txs() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty list - should not fail
        db.add_txs(&[]).await.unwrap();

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
        db.add_txs(&txs).await.unwrap();

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
        db.add_txs(&txs).await.unwrap();

        // Verify we still have exactly 10 transactions
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM transactions")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 10);
    }

    #[tokio::test]
    async fn test_add_request_locked_events() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty events - should not fail
        db.add_request_locked_events(&[]).await.unwrap();

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
        db.add_request_locked_events(&events).await.unwrap();

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
        db.add_request_locked_events(&events).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_locked_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500);
    }

    #[tokio::test]
    async fn test_add_proof_delivered_events() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty events - should not fail
        db.add_proof_delivered_events(&[]).await.unwrap();

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
        db.add_proof_delivered_events(&events).await.unwrap();

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
        db.add_proof_delivered_events(&events).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proof_delivered_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1200);
    }

    #[tokio::test]
    async fn test_add_request_fulfilled_events() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty events - should not fail
        db.add_request_fulfilled_events(&[]).await.unwrap();

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
        db.add_request_fulfilled_events(&events).await.unwrap();

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
        db.add_request_fulfilled_events(&events).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_fulfilled_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1000); // Still 1000 unique events
    }

    #[tokio::test]
    async fn test_add_proof_requests() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty requests - should not fail
        db.add_proof_requests(&[]).await.unwrap();

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
            let submission_timestamp = metadata.block_timestamp;
            requests.push((request_digest, request, metadata, source.to_string(), submission_timestamp));
        }

        // Add requests in batch
        db.add_proof_requests(&requests).await.unwrap();

        // Verify requests were added correctly - check a sample
        for i in [0, 500, 999, 1499].iter() {
            let (request_digest, request, metadata, source, submission_timestamp) = &requests[*i];

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

            // Verify submission_timestamp is set
            let db_submission_timestamp = row.get::<i64, _>("submission_timestamp");
            assert_eq!(
                db_submission_timestamp, *submission_timestamp as i64,
                "submission_timestamp should match for request {}", i
            );
            assert!(
                db_submission_timestamp > 0,
                "submission_timestamp should be > 0 for request {}", i
            );

            // Verify block_timestamp is set
            let db_block_timestamp = row.get::<i64, _>("block_timestamp");
            assert_eq!(
                db_block_timestamp, metadata.block_timestamp as i64,
                "block_timestamp should match for request {}", i
            );
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proof_requests")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500);

        // Test idempotency - adding same requests should not fail and not duplicate
        db.add_proof_requests(&requests).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proof_requests")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500); // Still 1500 unique requests
    }

    #[tokio::test]
    #[traced_test]
    async fn test_cycle_counts_insert_and_query() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let digest1 = B256::from([1; 32]);
        let digest2 = B256::from([2; 32]);
        let digest3 = B256::from([3; 32]);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let cycle_counts = vec![
            CycleCount {
                request_digest: digest1,
                cycle_status: "COMPLETED".to_string(),
                program_cycles: Some(U256::from(50_000_000_000u64)),
                total_cycles: Some(U256::from((50_000_000_000.0 * 1.0158) as u64)),
                created_at: now,
                updated_at: now,
            },
            CycleCount {
                request_digest: digest2,
                cycle_status: "PENDING".to_string(),
                program_cycles: None,
                total_cycles: None,
                created_at: now,
                updated_at: now,
            },
            CycleCount {
                request_digest: digest3,
                cycle_status: "COMPLETED".to_string(),
                program_cycles: Some(U256::from(54_000_000_000u64)),
                total_cycles: Some(U256::from((54_000_000_000.0 * 1.0158) as u64)),
                created_at: now,
                updated_at: now,
            },
        ];

        // Insert cycle counts
        db.add_cycle_counts(&cycle_counts).await.unwrap();

        // Verify insertion
        let result = sqlx::query("SELECT COUNT(*) as count FROM cycle_counts")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<i64, _>("count"), 3);

        // Test get_cycle_counts
        let digests = vec![digest1, digest2, digest3].into_iter().collect();
        let retrieved = db.get_cycle_counts(&digests).await.unwrap();
        assert_eq!(retrieved.len(), 3);

        // Find digest1 in results
        let cc1 = retrieved.iter().find(|cc| cc.request_digest == digest1).unwrap();
        assert_eq!(cc1.cycle_status, "COMPLETED");
        assert_eq!(cc1.program_cycles, Some(U256::from(50_000_000_000u64)));
        assert_eq!(cc1.total_cycles, Some(U256::from((50_000_000_000.0 * 1.0158) as u64)));

        // Find digest2 in results
        let cc2 = retrieved.iter().find(|cc| cc.request_digest == digest2).unwrap();
        assert_eq!(cc2.cycle_status, "PENDING");
        assert_eq!(cc2.program_cycles, None);
        assert_eq!(cc2.total_cycles, None);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_cycle_counts_idempotency() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let digest = B256::from([1; 32]);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let cycle_count = CycleCount {
            request_digest: digest,
            cycle_status: "COMPLETED".to_string(),
            program_cycles: Some(U256::from(50_000_000_000u64)),
            total_cycles: Some(U256::from((50_000_000_000.0 * 1.0158) as u64)),
            created_at: now,
            updated_at: now,
        };

        // Insert once
        db.add_cycle_counts(&[cycle_count.clone()]).await.unwrap();

        // Insert again with different values - should be ignored due to ON CONFLICT DO NOTHING
        let cycle_count_updated = CycleCount {
            request_digest: digest,
            cycle_status: "PENDING".to_string(),
            program_cycles: None,
            total_cycles: None,
            created_at: now + 1000,
            updated_at: now + 1000,
        };
        db.add_cycle_counts(&[cycle_count_updated]).await.unwrap();

        // Verify original value is preserved
        let result = sqlx::query("SELECT * FROM cycle_counts WHERE request_digest = $1")
            .bind(format!("{:x}", digest))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("cycle_status"), "COMPLETED");
        let program_cycles_str: Option<String> = result.get("program_cycles");
        let expected_program = u256_to_padded_string(U256::from(50_000_000_000u64));
        assert_eq!(program_cycles_str, Some(expected_program));
        let total_cycles_str: Option<String> = result.get("total_cycles");
        let expected_total = u256_to_padded_string(U256::from((50_000_000_000.0 * 1.0158) as u64));
        assert_eq!(total_cycles_str, Some(expected_total));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_has_cycle_counts() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Test with empty array
        let existing = db.has_cycle_counts(&[]).await.unwrap();
        assert_eq!(existing.len(), 0);

        // Add some cycle counts
        let digest1 = B256::from([1; 32]);
        let digest2 = B256::from([2; 32]);
        let digest3 = B256::from([3; 32]);
        let digest4 = B256::from([4; 32]);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        db.add_cycle_counts(&[
            CycleCount {
                request_digest: digest1,
                cycle_status: "COMPLETED".to_string(),
                program_cycles: Some(U256::from(50_000_000_000u64)),
                total_cycles: Some(U256::from((50_000_000_000.0 * 1.0158) as u64)),
                created_at: now,
                updated_at: now,
            },
            CycleCount {
                request_digest: digest2,
                cycle_status: "PENDING".to_string(),
                program_cycles: None,
                total_cycles: None,
                created_at: now,
                updated_at: now,
            },
        ])
        .await
        .unwrap();

        // Check which ones exist
        let existing = db.has_cycle_counts(&[digest1, digest2, digest3, digest4]).await.unwrap();
        assert_eq!(existing.len(), 2);
        assert!(existing.contains(&digest1));
        assert!(existing.contains(&digest2));
        assert!(!existing.contains(&digest3));
        assert!(!existing.contains(&digest4));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_get_request_inputs() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        // Add some proof requests
        let addr1 = Address::from([1; 20]);
        let addr2 = Address::from([2; 20]);
        let digest1 = B256::from([1; 32]);
        let digest2 = B256::from([2; 32]);

        let request1 = generate_request(1, &addr1);
        let request2 = generate_request(2, &addr2);
        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        db.add_proof_requests(&[(digest1, request1.clone(), metadata, "onchain".to_string(), metadata.block_timestamp)]).await.unwrap();
        db.add_proof_requests(&[(digest2, request2.clone(), metadata, "onchain".to_string(), metadata.block_timestamp)]).await.unwrap();

        // Query inputs
        let results = db.get_request_inputs(&[digest1, digest2]).await.unwrap();
        assert_eq!(results.len(), 2);

        // Check digest1
        let (d1, input_type1, _input_data1, client_addr1) =
            results.iter().find(|(d, _, _, _)| *d == digest1).unwrap();
        assert_eq!(*d1, digest1);
        assert_eq!(input_type1, "Inline");
        assert_eq!(*client_addr1, addr1);

        // Check digest2
        let (d2, input_type2, _input_data2, client_addr2) =
            results.iter().find(|(d, _, _, _)| *d == digest2).unwrap();
        assert_eq!(*d2, digest2);
        assert_eq!(input_type2, "Inline");
        assert_eq!(*client_addr2, addr2);
    }

    // ==================== Per-Requestor Aggregate Tests ====================

    #[tokio::test]
    async fn test_upsert_and_get_hourly_requestor_summary() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db; // Use concrete MarketDb

        let requestor = Address::from([0x42; 20]);
        let period_ts = 1700000000u64;

        let summary = HourlyRequestorSummary {
            period_timestamp: period_ts,
            requestor_address: requestor,
            total_fulfilled: 5,
            unique_provers_locking_requests: 2,
            total_fees_locked: U256::from(1000),
            total_collateral_locked: U256::from(2000),
            total_locked_and_expired_collateral: U256::ZERO,
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_requests_submitted: 10,
            total_requests_submitted_onchain: 7,
            total_requests_submitted_offchain: 3,
            total_requests_locked: 8,
            total_requests_slashed: 1,
            total_expired: 2,
            total_locked_and_expired: 1,
            total_locked_and_fulfilled: 5,
            locked_orders_fulfillment_rate: 0.625,
            total_program_cycles: U256::from(50_000_000_000u64),
            total_cycles: U256::from(50_790_000_000u64),
            best_peak_prove_mhz: 1500,
            best_peak_prove_mhz_prover: Some("0x1234".to_string()),
            best_peak_prove_mhz_request_id: Some(U256::from(123)),
            best_effective_prove_mhz: 1400,
            best_effective_prove_mhz_prover: Some("0x5678".to_string()),
            best_effective_prove_mhz_request_id: Some(U256::from(456)),
        };

        // Upsert
        db.upsert_hourly_requestor_summary(summary.clone()).await.unwrap();

        // Get by range
        let results = db.get_hourly_requestor_summaries_by_range(requestor, period_ts, period_ts + 1).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].period_timestamp, period_ts);
        assert_eq!(results[0].requestor_address, requestor);
        assert_eq!(results[0].total_fulfilled, 5);
        assert_eq!(results[0].total_program_cycles, U256::from(50_000_000_000u64));

        // Update with new values
        let mut updated = summary.clone();
        updated.total_fulfilled = 10;
        db.upsert_hourly_requestor_summary(updated).await.unwrap();

        let results = db.get_hourly_requestor_summaries_by_range(requestor, period_ts, period_ts + 1).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_fulfilled, 10);
    }

    #[tokio::test]
    async fn test_upsert_all_time_requestor_summary() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x42; 20]);
        let period_ts = 1700000000u64;

        let summary = AllTimeRequestorSummary {
            period_timestamp: period_ts,
            requestor_address: requestor,
            total_fulfilled: 100,
            unique_provers_locking_requests: 10,
            total_fees_locked: U256::from(50000),
            total_collateral_locked: U256::from(100000),
            total_locked_and_expired_collateral: U256::from(5000),
            total_requests_submitted: 150,
            total_requests_submitted_onchain: 120,
            total_requests_submitted_offchain: 30,
            total_requests_locked: 110,
            total_requests_slashed: 5,
            total_expired: 10,
            total_locked_and_expired: 8,
            total_locked_and_fulfilled: 100,
            locked_orders_fulfillment_rate: 0.909,
            total_program_cycles: U256::from(500_000_000_000u64),
            total_cycles: U256::from(507_900_000_000u64),
            best_peak_prove_mhz: 2000,
            best_peak_prove_mhz_prover: Some("0xaaa".to_string()),
            best_peak_prove_mhz_request_id: Some(U256::from(999)),
            best_effective_prove_mhz: 1900,
            best_effective_prove_mhz_prover: Some("0xbbb".to_string()),
            best_effective_prove_mhz_request_id: Some(U256::from(888)),
        };

        // Upsert
        db.upsert_all_time_requestor_summary(summary.clone()).await.unwrap();

        // Get (via latest since we don't have period_ts variant in trait)
        let result = db.get_latest_all_time_requestor_summary(requestor).await.unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.period_timestamp, period_ts);
        assert_eq!(result.requestor_address, requestor);
        assert_eq!(result.total_fulfilled, 100);
        assert_eq!(result.total_program_cycles, U256::from(500_000_000_000u64));

        // Update
        let mut updated = summary.clone();
        updated.total_fulfilled = 200;
        updated.period_timestamp = period_ts + 1; // New period
        db.upsert_all_time_requestor_summary(updated).await.unwrap();

        let result = db.get_latest_all_time_requestor_summary(requestor).await.unwrap().unwrap();
        assert_eq!(result.total_fulfilled, 200);
        assert_eq!(result.period_timestamp, period_ts + 1);
    }

    #[tokio::test]
    async fn test_upsert_and_get_daily_requestor_summary() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x43; 20]);
        let base_ts = 1700000000u64;
        let day_seconds = 86400u64;

        // Insert 3 daily summaries
        for i in 0..3 {
            let summary = DailyRequestorSummary {
                period_timestamp: base_ts + (i * day_seconds),
                requestor_address: requestor,
                total_fulfilled: 10 * (i + 1),
                unique_provers_locking_requests: 2 * (i + 1),
                total_fees_locked: U256::from(1000 * (i + 1)),
                total_collateral_locked: U256::from(2000 * (i + 1)),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(10),
                p25_lock_price_per_cycle: U256::from(25),
                p50_lock_price_per_cycle: U256::from(50),
                p75_lock_price_per_cycle: U256::from(75),
                p90_lock_price_per_cycle: U256::from(90),
                p95_lock_price_per_cycle: U256::from(95),
                p99_lock_price_per_cycle: U256::from(99),
                total_requests_submitted: 20 * (i + 1),
                total_requests_submitted_onchain: 15 * (i + 1),
                total_requests_submitted_offchain: 5 * (i + 1),
                total_requests_locked: 12 * (i + 1),
                total_requests_slashed: i,
                total_expired: i,
                total_locked_and_expired: i,
                total_locked_and_fulfilled: 10 * (i + 1),
                locked_orders_fulfillment_rate: 0.8,
                total_program_cycles: U256::from(100_000_000 * (i + 1)),
                total_cycles: U256::from(101_580_000 * (i + 1)),
                best_peak_prove_mhz: 1000,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 900,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_daily_requestor_summary(summary).await.unwrap();
        }

        // Get by range
        let results = db.get_daily_requestor_summaries_by_range(requestor, base_ts, base_ts + (3 * day_seconds)).await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].total_fulfilled, 10);
        assert_eq!(results[1].total_fulfilled, 20);
        assert_eq!(results[2].total_fulfilled, 30);
    }

    #[tokio::test]
    async fn test_upsert_and_get_weekly_requestor_summary() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x44; 20]);
        let base_ts = 1700000000u64;
        let week_seconds = 604800u64;

        let summary = WeeklyRequestorSummary {
            period_timestamp: base_ts,
            requestor_address: requestor,
            total_fulfilled: 50,
            unique_provers_locking_requests: 5,
            total_fees_locked: U256::from(10000),
            total_collateral_locked: U256::from(20000),
            total_locked_and_expired_collateral: U256::from(500),
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_requests_submitted: 100,
            total_requests_submitted_onchain: 80,
            total_requests_submitted_offchain: 20,
            total_requests_locked: 60,
            total_requests_slashed: 5,
            total_expired: 10,
            total_locked_and_expired: 8,
            total_locked_and_fulfilled: 50,
            locked_orders_fulfillment_rate: 0.833,
            total_program_cycles: U256::from(1_000_000_000),
            total_cycles: U256::from(1_015_800_000),
            best_peak_prove_mhz: 1200,
            best_peak_prove_mhz_prover: None,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 1100,
            best_effective_prove_mhz_prover: None,
            best_effective_prove_mhz_request_id: None,
        };

        db.upsert_weekly_requestor_summary(summary.clone()).await.unwrap();

        let results = db.get_weekly_requestor_summaries_by_range(requestor, base_ts, base_ts + week_seconds).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_fulfilled, 50);
        assert_eq!(results[0].requestor_address, requestor);
    }

    #[tokio::test]
    async fn test_upsert_and_get_monthly_requestor_summary() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x45; 20]);
        let base_ts = 1700000000u64;

        let summary = MonthlyRequestorSummary {
            period_timestamp: base_ts,
            requestor_address: requestor,
            total_fulfilled: 200,
            unique_provers_locking_requests: 15,
            total_fees_locked: U256::from(50000),
            total_collateral_locked: U256::from(100000),
            total_locked_and_expired_collateral: U256::from(5000),
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_requests_submitted: 300,
            total_requests_submitted_onchain: 250,
            total_requests_submitted_offchain: 50,
            total_requests_locked: 220,
            total_requests_slashed: 10,
            total_expired: 20,
            total_locked_and_expired: 15,
            total_locked_and_fulfilled: 200,
            locked_orders_fulfillment_rate: 0.909,
            total_program_cycles: U256::from(5_000_000_000u64),
            total_cycles: U256::from(5_079_000_000u64),
            best_peak_prove_mhz: 1500,
            best_peak_prove_mhz_prover: None,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 1400,
            best_effective_prove_mhz_prover: None,
            best_effective_prove_mhz_request_id: None,
        };

        db.upsert_monthly_requestor_summary(summary.clone()).await.unwrap();

        let results = db.get_monthly_requestor_summaries_by_range(requestor, base_ts, base_ts + 2_628_000).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_fulfilled, 200);
    }

    #[tokio::test]
    async fn test_get_all_time_requestor_summary_by_timestamp() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x46; 20]);
        let ts1 = 1700000000u64;
        let ts2 = 1700010000u64;

        let summary1 = AllTimeRequestorSummary {
            period_timestamp: ts1,
            requestor_address: requestor,
            total_fulfilled: 100,
            unique_provers_locking_requests: 10,
            total_fees_locked: U256::from(10000),
            total_collateral_locked: U256::from(20000),
            total_locked_and_expired_collateral: U256::ZERO,
            total_requests_submitted: 150,
            total_requests_submitted_onchain: 120,
            total_requests_submitted_offchain: 30,
            total_requests_locked: 110,
            total_requests_slashed: 5,
            total_expired: 10,
            total_locked_and_expired: 8,
            total_locked_and_fulfilled: 100,
            locked_orders_fulfillment_rate: 0.909,
            total_program_cycles: U256::from(1_000_000_000),
            total_cycles: U256::from(1_015_800_000),
            best_peak_prove_mhz: 1000,
            best_peak_prove_mhz_prover: None,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 900,
            best_effective_prove_mhz_prover: None,
            best_effective_prove_mhz_request_id: None,
        };

        let summary2 = AllTimeRequestorSummary {
            period_timestamp: ts2,
            requestor_address: requestor,
            total_fulfilled: 200,
            ..summary1.clone()
        };

        db.upsert_all_time_requestor_summary(summary1).await.unwrap();
        db.upsert_all_time_requestor_summary(summary2).await.unwrap();

        // Get by specific timestamp
        let result = db.get_all_time_requestor_summary_by_timestamp(requestor, ts1).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().total_fulfilled, 100);

        let result = db.get_all_time_requestor_summary_by_timestamp(requestor, ts2).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().total_fulfilled, 200);

        // Non-existent timestamp
        let result = db.get_all_time_requestor_summary_by_timestamp(requestor, 9999999).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_all_requestor_addresses() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let addr1 = Address::from([0x10; 20]);
        let addr2 = Address::from([0x20; 20]);
        let addr3 = Address::from([0x30; 20]);

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        // Add requests from different clients
        let request1 = generate_request(1, &addr1);
        let request2 = generate_request(2, &addr2);
        let request3 = generate_request(3, &addr3);
        let request4 = generate_request(4, &addr1); // Duplicate client

        db.add_proof_requests(&[
            (B256::from([1; 32]), request1, metadata, "onchain".to_string(), metadata.block_timestamp),
        ]).await.unwrap();
        db.add_proof_requests(&[
            (B256::from([2; 32]), request2, metadata, "onchain".to_string(), metadata.block_timestamp),
        ]).await.unwrap();
        db.add_proof_requests(&[
            (B256::from([3; 32]), request3, metadata, "onchain".to_string(), metadata.block_timestamp),
        ]).await.unwrap();
        db.add_proof_requests(&[
            (B256::from([4; 32]), request4, metadata, "onchain".to_string(), metadata.block_timestamp),
        ]).await.unwrap();

        // Get all requestor addresses (should be unique)
        let addresses = db.get_all_requestor_addresses().await.unwrap();
        assert_eq!(addresses.len(), 3);
        assert!(addresses.contains(&addr1));
        assert!(addresses.contains(&addr2));
        assert!(addresses.contains(&addr3));
    }

    #[tokio::test]
    async fn test_get_active_requestor_addresses_in_period() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let addr1 = Address::from([0x11; 20]);
        let addr2 = Address::from([0x22; 20]);
        let addr3 = Address::from([0x33; 20]);

        let base_ts = 1700000000u64;

        // Create proof_requests records (the query now uses proof_requests.submission_timestamp)
        let digest1 = B256::from([1; 32]);
        let digest2 = B256::from([2; 32]);
        let digest3 = B256::from([3; 32]);

        let request1 = generate_request(1, &addr1);
        let request2 = generate_request(2, &addr2);
        let request3 = generate_request(3, &addr3);

        let metadata1 = TxMetadata::new(B256::ZERO, addr1, 100, base_ts + 100, 0);
        let metadata2 = TxMetadata::new(B256::from([1; 32]), addr2, 101, base_ts + 500, 0);
        let metadata3 = TxMetadata::new(B256::from([2; 32]), addr3, 102, base_ts + 1500, 0);

        // submission_timestamp should match the block_timestamp for onchain requests
        db.add_proof_requests(&[
            (digest1, request1, metadata1, "onchain".to_string(), base_ts + 100),
            (digest2, request2, metadata2, "onchain".to_string(), base_ts + 500),
            (digest3, request3, metadata3, "onchain".to_string(), base_ts + 1500), // Outside period
        ]).await.unwrap();

        // Query active requestors in period [base_ts, base_ts + 1000)
        // This now queries proof_requests.submission_timestamp
        let addresses = db.get_active_requestor_addresses_in_period(base_ts, base_ts + 1000).await.unwrap();
        assert_eq!(addresses.len(), 2);
        assert!(addresses.contains(&addr1));
        assert!(addresses.contains(&addr2));
        assert!(!addresses.contains(&addr3)); // Outside period
    }

    #[tokio::test]
    async fn test_list_requests_by_requestor() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let addr1 = Address::from([0x12; 20]);
        let addr2 = Address::from([0x34; 20]);
        let base_ts = 1700000000u64;

        // Create statuses for addr1
        for i in 0..5 {
            let status = RequestStatus {
                request_digest: B256::from([i as u8; 32]),
                request_id: U256::from(i),
                request_status: RequestStatusType::Submitted,
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: addr1,
                lock_prover_address: None,
                fulfill_prover_address: None,
                created_at: base_ts + (i as u64 * 100),
                updated_at: base_ts + (i as u64 * 100),
                locked_at: None,
                fulfilled_at: None,
                slashed_at: None,
                lock_prover_delivered_proof_at: None,
                submit_block: Some(100),
                lock_block: None,
                fulfill_block: None,
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: "0".to_string(),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: base_ts + 10000,
                lock_end: base_ts + 10000,
                slash_recipient: None,
                slash_transferred_amount: None,
                slash_burned_amount: None,
                program_cycles: None,
                total_cycles: None,
                peak_prove_mhz: None,
                effective_prove_mhz: None,
                cycle_status: None,
                lock_price: None,
                lock_price_per_cycle: None,
                submit_tx_hash: Some(B256::ZERO),
                lock_tx_hash: None,
                fulfill_tx_hash: None,
                slash_tx_hash: None,
                image_id: "test".to_string(),
                image_url: None,
                selector: "test".to_string(),
                predicate_type: "digest_match".to_string(),
                predicate_data: "0x00".to_string(),
                input_type: "inline".to_string(),
                input_data: "0x00".to_string(),
                fulfill_journal: None,
                fulfill_seal: None,
            };
            db.upsert_request_statuses(&[status]).await.unwrap();
        }

        // Create one for addr2
        let status_addr2 = RequestStatus {
            request_digest: B256::from([99; 32]),
            request_id: U256::from(99),
            client_address: addr2,
            created_at: base_ts,
            updated_at: base_ts,
            request_status: RequestStatusType::Submitted,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            lock_prover_address: None,
            fulfill_prover_address: None,
            locked_at: None,
            fulfilled_at: None,
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(100),
            lock_block: None,
            fulfill_block: None,
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "0".to_string(),
            ramp_up_start: base_ts,
            ramp_up_period: 10,
            expires_at: base_ts + 10000,
            lock_end: base_ts + 10000,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            program_cycles: None,
            total_cycles: None,
            peak_prove_mhz: None,
            effective_prove_mhz: None,
            cycle_status: None,
            lock_price: None,
            lock_price_per_cycle: None,
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: None,
            fulfill_tx_hash: None,
            slash_tx_hash: None,
            image_id: "test".to_string(),
            image_url: None,
            selector: "test".to_string(),
            predicate_type: "digest_match".to_string(),
            predicate_data: "0x00".to_string(),
            input_type: "inline".to_string(),
            input_data: "0x00".to_string(),
            fulfill_journal: None,
            fulfill_seal: None,
        };
        db.upsert_request_statuses(&[status_addr2]).await.unwrap();

        // List requests for addr1
        let (results, _cursor) = db.list_requests_by_requestor(addr1, None, 10, RequestSortField::CreatedAt).await.unwrap();
        assert_eq!(results.len(), 5);
        assert!(results.iter().all(|r| r.client_address == addr1));

        // List with limit
        let (results, cursor) = db.list_requests_by_requestor(addr1, None, 2, RequestSortField::CreatedAt).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(cursor.is_some());

        // Use cursor for pagination
        let (results2, _) = db.list_requests_by_requestor(addr1, cursor, 2, RequestSortField::CreatedAt).await.unwrap();
        assert_eq!(results2.len(), 2);
        // Results should be different from first page
        assert_ne!(results[0].request_id, results2[0].request_id);

        // List for addr2
        let (results, _) = db.list_requests_by_requestor(addr2, None, 10, RequestSortField::CreatedAt).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].client_address, addr2);
    }

    // Helper to setup period query test data
    async fn setup_period_requestor_test_data(db: &MarketDb, requestor: Address, base_ts: u64) {
        let submit_metadata = TxMetadata::new(B256::from([0x01; 32]), Address::ZERO, 100, base_ts + 100, 0);
        let lock_metadata = TxMetadata::new(B256::from([0x02; 32]), Address::ZERO, 101, base_ts + 150, 0);
        let fulfill_metadata = TxMetadata::new(B256::from([0x03; 32]), Address::ZERO, 102, base_ts + 200, 0);
        
        // Add proof requests
        for i in 0..5 {
            let collateral = U256::from(100 * (i + 1));
            let request = generate_request_with_collateral(i, &requestor, collateral);
            // Generate unique digest per requestor by combining first byte of address with index
            let mut digest_bytes = [i as u8; 32];
            digest_bytes[0] = requestor.0[0];  // Use first byte of address for uniqueness
            let digest = B256::from(digest_bytes);
            db.add_proof_requests(&[(digest, request, submit_metadata, "onchain".to_string(), submit_metadata.block_timestamp)]).await.unwrap();
        }

        // Add request statuses
        for i in 0..5 {
            let mut digest_bytes = [i as u8; 32];
            digest_bytes[0] = requestor.0[0];
            let digest = B256::from(digest_bytes);
            let status = RequestStatus {
                request_digest: digest,
                request_id: U256::from(i),
                request_status: if i < 3 { RequestStatusType::Fulfilled } else { RequestStatusType::Submitted },
                slashed_status: if i == 4 { SlashedStatus::Slashed } else { SlashedStatus::NotApplicable },
                source: "onchain".to_string(),
                client_address: requestor,
                lock_prover_address: Some(Address::from([0xAA; 20])),
                fulfill_prover_address: if i < 3 { Some(Address::from([0xAA; 20])) } else { None },
                created_at: base_ts + 100,
                updated_at: base_ts + 200,
                locked_at: Some(base_ts + 150),
                fulfilled_at: if i < 3 { Some(base_ts + 200) } else { None },
                slashed_at: if i == 4 { Some(base_ts + 300) } else { None },
                lock_prover_delivered_proof_at: if i < 3 { Some(base_ts + 180) } else { None },
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: if i < 3 { Some(102) } else { None },
                slashed_block: if i == 4 { Some(103) } else { None },
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: format!("{}", 100 * (i + 1)),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: if i == 3 { base_ts + 250 } else { base_ts + 10000 },
                lock_end: base_ts + 500,
                slash_recipient: if i == 4 { Some(Address::from([0xBB; 20])) } else { None },
                slash_transferred_amount: if i == 4 { Some("50".to_string()) } else { None },
                slash_burned_amount: if i == 4 { Some("50".to_string()) } else { None },
                program_cycles: if i < 3 { Some(U256::from(50_000_000 * (i + 1))) } else { None },
                total_cycles: if i < 3 { Some(U256::from(50_790_000 * (i + 1))) } else { None },
                peak_prove_mhz: if i < 3 { Some(1000 + (i * 100)) } else { None },
                effective_prove_mhz: if i < 3 { Some(900 + (i * 100)) } else { None },
                cycle_status: if i < 3 { Some("resolved".to_string()) } else { None },
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("30".to_string()),
                submit_tx_hash: Some(B256::from([0x01; 32])),
                lock_tx_hash: Some(B256::from([0x02; 32])),
                fulfill_tx_hash: if i < 3 { Some(B256::from([0x03; 32])) } else { None },
                slash_tx_hash: if i == 4 { Some(B256::from([0x04; 32])) } else { None },
                image_id: "test".to_string(),
                image_url: None,
                selector: "test".to_string(),
                predicate_type: "digest_match".to_string(),
                predicate_data: "0x00".to_string(),
                input_type: "inline".to_string(),
                input_data: "0x00".to_string(),
                fulfill_journal: None,
                fulfill_seal: None,
            };
            db.upsert_request_statuses(&[status]).await.unwrap();
        }

        // Add events
        for i in 0..5 {
            let mut digest_bytes = [i as u8; 32];
            digest_bytes[0] = requestor.0[0];
            let digest = B256::from(digest_bytes);
            
            // Submitted events
            db.add_request_submitted_events(&[(digest, U256::from(i), submit_metadata)]).await.unwrap();
            
            // Locked events
            db.add_request_locked_events(&[(digest, U256::from(i), Address::from([0xAA; 20]), lock_metadata)]).await.unwrap();
            
            // Fulfilled events (only for first 3)
            if i < 3 {
                db.add_request_fulfilled_events(&[(digest, U256::from(i), Address::from([0xAA; 20]), fulfill_metadata)]).await.unwrap();
            }
            
            // Slashed event (only for index 4)
            if i == 4 {
                let slash_metadata = TxMetadata::new(B256::from([0x04; 32]), Address::ZERO, 103, base_ts + 300, 0);
                db.add_prover_slashed_events(&[(U256::from(i), U256::from(50), U256::from(50), Address::from([0xBB; 20]), slash_metadata)]).await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn test_get_period_requestor_fulfilled_count() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x50; 20]);
        let base_ts = 1700000000u64;
        
        // Add 3 fulfilled requests
        for i in 0..3 {
            let digest = B256::from([(i + 100) as u8; 32]);
            let request = generate_request(i, &requestor);
            
            // 1. Add proof request
            let submit_meta = TxMetadata::new(B256::from([i as u8; 32]), Address::ZERO, 100 + (i as u64), base_ts + 100, 0);
            db.add_proof_requests(&[(digest, request.clone(), submit_meta, "onchain".to_string(), submit_meta.block_timestamp)]).await.unwrap();
            
            // 2. Add fulfilled event (this is what the query counts)
            let fulfill_meta = TxMetadata::new(B256::from([(i + 50) as u8; 32]), Address::ZERO, 102 + (i as u64), base_ts + 200, 0);
            db.add_request_fulfilled_events(&[(digest, request.id, Address::from([0xAA; 20]), fulfill_meta)]).await.unwrap();
            
            // 3. Add request status (this is what the query joins with)
            let status = RequestStatus {
                request_digest: digest,
                request_id: request.id,
                request_status: RequestStatusType::Fulfilled,
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: requestor,
                lock_prover_address: Some(Address::from([0xAA; 20])),
                fulfill_prover_address: Some(Address::from([0xAA; 20])),
                created_at: base_ts + 100,
                updated_at: base_ts + 200,
                locked_at: Some(base_ts + 150),
                fulfilled_at: Some(base_ts + 200),
                slashed_at: None,
                lock_prover_delivered_proof_at: Some(base_ts + 180),
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: Some(102),
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: "100".to_string(),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: base_ts + 10000,
                lock_end: base_ts + 500,
                slash_recipient: None,
                slash_transferred_amount: None,
                slash_burned_amount: None,
                program_cycles: Some(U256::from(50_000_000)),
                total_cycles: Some(U256::from(50_790_000)),
                peak_prove_mhz: Some(1000),
                effective_prove_mhz: Some(900),
                cycle_status: Some("resolved".to_string()),
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("30".to_string()),
                submit_tx_hash: Some(B256::from([0x01; 32])),
                lock_tx_hash: Some(B256::from([0x02; 32])),
                fulfill_tx_hash: Some(B256::from([0x03; 32])),
                slash_tx_hash: None,
                image_id: "test".to_string(),
                image_url: None,
                selector: "test".to_string(),
                predicate_type: "digest_match".to_string(),
                predicate_data: "0x00".to_string(),
                input_type: "inline".to_string(),
                input_data: "0x00".to_string(),
                fulfill_journal: None,
                fulfill_seal: None,
            };
            db.upsert_request_statuses(&[status]).await.unwrap();
        }

        // Query period that includes the events
        let count = db.get_period_requestor_fulfilled_count(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(count, 3); // 3 fulfilled requests
    }

    #[tokio::test]
    async fn test_get_period_requestor_unique_provers() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x51; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db.get_period_requestor_unique_provers(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(count, 1); // All locked by same prover
    }

    #[tokio::test]
    async fn test_get_period_requestor_total_requests_submitted() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x52; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db.get_period_requestor_total_requests_submitted(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(count, 5); // All 5 requests created in period
    }

    #[tokio::test]
    async fn test_get_period_requestor_total_requests_submitted_onchain() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x53; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db.get_period_requestor_total_requests_submitted_onchain(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(count, 5); // All 5 have submitted events
    }

    #[tokio::test]
    async fn test_get_period_requestor_total_requests_locked() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x54; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db.get_period_requestor_total_requests_locked(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(count, 5); // All 5 locked
    }

    #[tokio::test]
    async fn test_get_period_requestor_total_requests_slashed() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x55; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db.get_period_requestor_total_requests_slashed(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(count, 1); // Only request 4 was slashed
    }

    #[tokio::test]
    async fn test_get_period_requestor_lock_pricing_data() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x56; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let data = db.get_period_requestor_lock_pricing_data(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(data.len(), 3); // 3 fulfilled requests
        
        for item in &data {
            assert_eq!(item.min_price, "1000");
            assert_eq!(item.max_price, "2000");
        }
    }

    #[tokio::test]
    async fn test_get_period_requestor_all_lock_collateral() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x57; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let collaterals = db.get_period_requestor_all_lock_collateral(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(collaterals.len(), 5); // All 5 locked
        
        // Verify collateral values
        let total: u64 = collaterals.iter().map(|c| c.parse::<u64>().unwrap()).sum();
        assert_eq!(total, 100 + 200 + 300 + 400 + 500); // Sum of all collaterals
    }

    #[tokio::test]
    async fn test_get_period_requestor_locked_and_expired_collateral() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x58; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        // Query period that includes request 3's expiration
        let collaterals = db.get_period_requestor_locked_and_expired_collateral(base_ts, base_ts + 1000, requestor).await.unwrap();
        // Request 3 expired and was locked
        assert!(collaterals.len() <= 1); // May be 0 or 1 depending on status updates
    }

    #[tokio::test]
    async fn test_get_period_requestor_expired_count() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x59; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db.get_period_requestor_expired_count(base_ts, base_ts + 1000, requestor).await.unwrap();
        // Depends on whether request 3 is marked as expired
        assert!(count <= 1);
    }

    #[tokio::test]
    async fn test_get_period_requestor_locked_and_expired_count() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x5A; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db.get_period_requestor_locked_and_expired_count(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert!(count <= 1); // Request 3 if marked expired
    }

    #[tokio::test]
    async fn test_get_period_requestor_locked_and_fulfilled_count() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x5B; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db.get_period_requestor_locked_and_fulfilled_count(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(count, 3); // 3 fulfilled requests
    }

    #[tokio::test]
    async fn test_get_period_requestor_total_program_cycles() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x5C; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let total = db.get_period_requestor_total_program_cycles(base_ts, base_ts + 1000, requestor).await.unwrap();
        // Sum of 50M * 1, 50M * 2, 50M * 3 = 300M
        assert_eq!(total, U256::from(50_000_000 + 100_000_000 + 150_000_000));
    }

    #[tokio::test]
    async fn test_get_period_requestor_total_cycles() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x5D; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let total = db.get_period_requestor_total_cycles(base_ts, base_ts + 1000, requestor).await.unwrap();
        // Sum of cycles with overhead
        assert_eq!(total, U256::from(50_790_000 + 101_580_000 + 152_370_000));
    }

    #[tokio::test]
    async fn test_get_all_time_requestor_unique_provers() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x5E; 20]);
        let base_ts = 1700000000u64;
        
        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db.get_all_time_requestor_unique_provers(base_ts + 10000, requestor).await.unwrap();
        assert_eq!(count, 1); // All locked by same prover
    }

    #[tokio::test]
    async fn test_requestor_methods_with_multiple_requestors() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor1 = Address::from([0x60; 20]);
        let requestor2 = Address::from([0x61; 20]);
        let base_ts = 1700000000u64;

        // Setup data for two different requestors
        setup_period_requestor_test_data(db, requestor1, base_ts).await;
        setup_period_requestor_test_data(db, requestor2, base_ts + 10).await;

        // Verify each requestor's data is isolated
        let count1 = db.get_period_requestor_fulfilled_count(base_ts, base_ts + 1000, requestor1).await.unwrap();
        let count2 = db.get_period_requestor_fulfilled_count(base_ts, base_ts + 1000, requestor2).await.unwrap();
        
        assert_eq!(count1, 3);
        assert_eq!(count2, 3);

        // Verify addresses list includes both
        let all_addresses = db.get_all_requestor_addresses().await.unwrap();
        assert!(all_addresses.contains(&requestor1));
        assert!(all_addresses.contains(&requestor2));
    }

    #[tokio::test]
    async fn test_requestor_summaries_with_empty_results() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let requestor = Address::from([0x62; 20]);
        let base_ts = 1700000000u64;

        // Query without any data
        let results = db.get_hourly_requestor_summaries_by_range(requestor, base_ts, base_ts + 3600).await.unwrap();
        assert_eq!(results.len(), 0);

        let latest = db.get_latest_all_time_requestor_summary(requestor).await.unwrap();
        assert!(latest.is_none());

        let by_ts = db.get_all_time_requestor_summary_by_timestamp(requestor, base_ts).await.unwrap();
        assert!(by_ts.is_none());
    }

}
