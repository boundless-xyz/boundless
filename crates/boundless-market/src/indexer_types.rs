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

//! Shared data types for the Boundless Indexer market API.
//!
//! These types represent the request parameters and response bodies for the
//! market endpoints of the Boundless Indexer API. They are used by both the
//! API server (indexer-api) and the SDK client ([`crate::indexer_client::IndexerClient`]).
//!
//! When the `openapi` feature is enabled, these types additionally derive
//! `utoipa::ToSchema` and (for query parameter types) `utoipa::IntoParams`
//! for OpenAPI documentation generation.

// These are data transfer types with many self-documenting fields; requiring
// doc comments on every field adds noise without improving clarity.
#![allow(missing_docs)]

use serde::{Deserialize, Serialize};

// ─── Enums ───────────────────────────────────────────────────────────────────

/// Granularity level for aggregating market data over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum AggregationGranularity {
    /// Aggregate data by hour.
    Hourly,
    /// Aggregate data by day.
    Daily,
    /// Aggregate data by week.
    Weekly,
    /// Aggregate data by month.
    Monthly,
    /// Aggregate data by epoch.
    Epoch,
}

impl Default for AggregationGranularity {
    fn default() -> Self {
        Self::Monthly
    }
}

impl std::fmt::Display for AggregationGranularity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hourly => write!(f, "hourly"),
            Self::Daily => write!(f, "daily"),
            Self::Weekly => write!(f, "weekly"),
            Self::Monthly => write!(f, "monthly"),
            Self::Epoch => write!(f, "epoch"),
        }
    }
}

/// Time period for leaderboard filtering.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LeaderboardPeriod {
    /// Last 1 hour
    #[serde(rename = "1h")]
    OneHour,
    /// Last 1 day (24 hours)
    #[serde(rename = "1d")]
    OneDay,
    /// Last 3 days
    #[serde(rename = "3d")]
    ThreeDays,
    /// Last 7 days
    #[serde(rename = "7d")]
    SevenDays,
    /// All time
    #[default]
    #[serde(rename = "all")]
    AllTime,
}

impl std::fmt::Display for LeaderboardPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OneHour => write!(f, "1h"),
            Self::OneDay => write!(f, "1d"),
            Self::ThreeDays => write!(f, "3d"),
            Self::SevenDays => write!(f, "7d"),
            Self::AllTime => write!(f, "all"),
        }
    }
}

// ─── Query Parameter Types ───────────────────────────────────────────────────

/// Parameters for querying market aggregate data.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct MarketAggregatesParams {
    /// Aggregation granularity: hourly, daily, weekly, or monthly
    #[serde(default)]
    pub aggregation: AggregationGranularity,

    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    pub cursor: Option<String>,

    /// Maximum number of results to return (max 500, default 50)
    #[serde(default)]
    pub limit: Option<u64>,

    /// Sort order: "asc" or "desc" (default "desc")
    #[serde(default)]
    pub sort: Option<String>,

    /// Unix timestamp (in seconds) — only return data before this time
    #[serde(default)]
    pub before: Option<i64>,

    /// Unix timestamp (in seconds) — only return data after this time
    #[serde(default)]
    pub after: Option<i64>,
}

/// Parameters for querying market cumulative data.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct MarketCumulativesParams {
    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    pub cursor: Option<String>,

    /// Maximum number of results to return (max 500, default 50)
    #[serde(default)]
    pub limit: Option<u64>,

    /// Sort order: "asc" or "desc" (default "desc" — newest first)
    #[serde(default)]
    pub sort: Option<String>,

    /// Unix timestamp to fetch cumulatives before this time
    #[serde(default)]
    pub before: Option<i64>,

    /// Unix timestamp to fetch cumulatives after this time
    #[serde(default)]
    pub after: Option<i64>,
}

/// Parameters for querying requestor aggregate data.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct RequestorAggregatesParams {
    /// Aggregation granularity: hourly, daily, or weekly (monthly not supported)
    #[serde(default)]
    pub aggregation: AggregationGranularity,

    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    pub cursor: Option<String>,

    /// Maximum number of results to return (max 500, default 50)
    #[serde(default)]
    pub limit: Option<u64>,

    /// Sort order: "asc" or "desc" (default "desc")
    #[serde(default)]
    pub sort: Option<String>,

    /// Unix timestamp to fetch aggregates before this time
    #[serde(default)]
    pub before: Option<i64>,

    /// Unix timestamp to fetch aggregates after this time
    #[serde(default)]
    pub after: Option<i64>,
}

/// Parameters for querying requestor cumulative data.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct RequestorCumulativesParams {
    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    pub cursor: Option<String>,

    /// Maximum number of results to return (max 500, default 50)
    #[serde(default)]
    pub limit: Option<u64>,

    /// Sort order: "asc" or "desc" (default "desc" — newest first)
    #[serde(default)]
    pub sort: Option<String>,

    /// Unix timestamp to fetch cumulatives before this time
    #[serde(default)]
    pub before: Option<i64>,

    /// Unix timestamp to fetch cumulatives after this time
    #[serde(default)]
    pub after: Option<i64>,
}

/// Parameters for querying prover aggregate data.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct ProverAggregatesParams {
    /// Aggregation granularity: hourly, daily, weekly, or monthly
    #[serde(default)]
    pub aggregation: AggregationGranularity,

    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    pub cursor: Option<String>,

    /// Maximum number of results to return (max 500, default 50)
    #[serde(default)]
    pub limit: Option<u64>,

    /// Sort order: "asc" or "desc" (default "desc")
    #[serde(default)]
    pub sort: Option<String>,

    /// Unix timestamp to fetch aggregates before this time
    #[serde(default)]
    pub before: Option<i64>,

    /// Unix timestamp to fetch aggregates after this time
    #[serde(default)]
    pub after: Option<i64>,
}

/// Parameters for querying prover cumulative data.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct ProverCumulativesParams {
    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    pub cursor: Option<String>,

    /// Maximum number of results to return (max 500, default 50)
    #[serde(default)]
    pub limit: Option<u64>,

    /// Sort order: "asc" or "desc" (default "desc")
    #[serde(default)]
    pub sort: Option<String>,

    /// Unix timestamp to fetch cumulatives before this time
    #[serde(default)]
    pub before: Option<i64>,

    /// Unix timestamp to fetch cumulatives after this time
    #[serde(default)]
    pub after: Option<i64>,
}

/// Parameters for querying the requestor leaderboard.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct RequestorLeaderboardParams {
    /// Time period: 1h, 1d, 3d, 7d, or all (default: all)
    #[serde(default)]
    pub period: LeaderboardPeriod,

    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    pub cursor: Option<String>,

    /// Limit of results returned, max 100 (default 50)
    #[serde(default)]
    pub limit: Option<u64>,
}

/// Parameters for listing requests.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams, utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct RequestListParams {
    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    pub cursor: Option<String>,

    /// Limit of requests returned, max 500 (default 50)
    #[serde(default)]
    pub limit: Option<u32>,

    /// Sort field: "updated_at" or "created_at" (default "created_at")
    #[serde(default)]
    pub sort_by: Option<String>,
}

// ─── Market Response Types ───────────────────────────────────────────────────

/// Current indexing status response.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct IndexingStatusResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Last indexed block number
    pub last_indexed_block: u64,
    /// Last indexed block timestamp (Unix timestamp)
    pub last_indexed_block_timestamp: i64,
    /// Last indexed block timestamp (ISO 8601)
    pub last_indexed_block_timestamp_iso: String,
}

/// A single entry in market aggregate data, representing aggregated statistics for a time period.
#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarketAggregateEntry {
    /// The chain ID.
    pub chain_id: u64,
    /// Unix timestamp (in seconds) for this aggregate period.
    pub timestamp: i64,
    /// ISO 8601 formatted timestamp for this aggregate period.
    pub timestamp_iso: String,
    /// Total number of requests fulfilled in this period.
    pub total_fulfilled: i64,
    /// Number of unique provers who locked requests in this period.
    pub unique_provers_locking_requests: i64,
    /// Number of unique requesters who submitted requests in this period.
    pub unique_requesters_submitting_requests: i64,
    /// Total fees locked in this period (as string, in wei).
    pub total_fees_locked: String,
    /// Total fees locked in this period (formatted for display).
    pub total_fees_locked_formatted: String,
    /// Total collateral locked in this period (as string, in wei).
    pub total_collateral_locked: String,
    /// Total collateral locked in this period (formatted for display).
    pub total_collateral_locked_formatted: String,
    /// Total collateral from locked requests that expired (as string, in wei).
    pub total_locked_and_expired_collateral: String,
    /// Total collateral from locked requests that expired (formatted for display).
    pub total_locked_and_expired_collateral_formatted: String,
    /// 10th percentile lock price per cycle (as string, in wei).
    pub p10_lock_price_per_cycle: String,
    /// 10th percentile lock price per cycle (formatted for display).
    pub p10_lock_price_per_cycle_formatted: String,
    /// 25th percentile lock price per cycle (as string, in wei).
    pub p25_lock_price_per_cycle: String,
    /// 25th percentile lock price per cycle (formatted for display).
    pub p25_lock_price_per_cycle_formatted: String,
    /// 50th percentile (median) lock price per cycle (as string, in wei).
    pub p50_lock_price_per_cycle: String,
    /// 50th percentile (median) lock price per cycle (formatted for display).
    pub p50_lock_price_per_cycle_formatted: String,
    /// 75th percentile lock price per cycle (as string, in wei).
    pub p75_lock_price_per_cycle: String,
    /// 75th percentile lock price per cycle (formatted for display).
    pub p75_lock_price_per_cycle_formatted: String,
    /// 90th percentile lock price per cycle (as string, in wei).
    pub p90_lock_price_per_cycle: String,
    /// 90th percentile lock price per cycle (formatted for display).
    pub p90_lock_price_per_cycle_formatted: String,
    /// 95th percentile lock price per cycle (as string, in wei).
    pub p95_lock_price_per_cycle: String,
    /// 95th percentile lock price per cycle (formatted for display).
    pub p95_lock_price_per_cycle_formatted: String,
    /// 99th percentile lock price per cycle (as string, in wei).
    pub p99_lock_price_per_cycle: String,
    /// 99th percentile lock price per cycle (formatted for display).
    pub p99_lock_price_per_cycle_formatted: String,
    /// Total number of requests submitted in this period.
    pub total_requests_submitted: i64,
    /// Total number of requests submitted on-chain in this period.
    pub total_requests_submitted_onchain: i64,
    /// Total number of requests submitted off-chain in this period.
    pub total_requests_submitted_offchain: i64,
    /// Total number of requests locked in this period.
    pub total_requests_locked: i64,
    /// Total number of requests slashed in this period.
    pub total_requests_slashed: i64,
    /// Total number of requests expired in this period.
    pub total_expired: i64,
    /// Total number of locked requests that expired in this period.
    pub total_locked_and_expired: i64,
    /// Total number of locked requests that were fulfilled in this period.
    pub total_locked_and_fulfilled: i64,
    /// Total number of secondary fulfillments in this period.
    pub total_secondary_fulfillments: i64,
    /// Fulfillment rate for locked orders (as a percentage, 0.0–100.0).
    pub locked_orders_fulfillment_rate: f32,
    /// Adjusted fulfillment rate for locked orders, deduplicated by (input_data, image_url) (percentage).
    #[serde(default)]
    pub locked_orders_fulfillment_rate_adjusted: f32,
    /// Total program cycles executed across all fulfilled requests in this period (as string).
    pub total_program_cycles: String,
    /// Total cycles (program + overhead) across all fulfilled requests in this period (as string).
    pub total_cycles: String,
    /// Total fixed cost (gas cost) across all locked requests in this period (as string in wei).
    #[serde(default)]
    pub total_fixed_cost: String,
    /// Total fixed cost (formatted for display).
    #[serde(default)]
    pub total_fixed_cost_formatted: String,
    /// Total variable cost (proving cost) across all locked requests in this period (as string in wei).
    #[serde(default)]
    pub total_variable_cost: String,
    /// Total variable cost (formatted for display).
    #[serde(default)]
    pub total_variable_cost_formatted: String,
    /// Epoch number at the start of this period (None if timestamp is before epoch 0).
    #[serde(default)]
    pub epoch_number_start: Option<i64>,
}

/// Response containing market aggregate data.
#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarketAggregatesResponse {
    /// The chain ID.
    pub chain_id: u64,
    /// The aggregation granularity used.
    pub aggregation: AggregationGranularity,
    /// List of aggregate entries.
    pub data: Vec<MarketAggregateEntry>,
    /// Cursor for pagination to retrieve the next page.
    pub next_cursor: Option<String>,
    /// Whether there are more results available.
    pub has_more: bool,
}

/// A single entry in market cumulative data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarketCumulativeEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Timestamp for this cumulative snapshot (Unix timestamp)
    pub timestamp: i64,
    /// ISO 8601 formatted timestamp
    pub timestamp_iso: String,
    /// Total number of fulfilled orders (cumulative)
    pub total_fulfilled: i64,
    /// Unique provers who locked requests (cumulative)
    pub unique_provers_locking_requests: i64,
    /// Unique requesters who submitted requests (cumulative)
    pub unique_requesters_submitting_requests: i64,
    /// Total fees locked (as string)
    pub total_fees_locked: String,
    /// Total fees locked (formatted for display)
    pub total_fees_locked_formatted: String,
    /// Total collateral locked (as string)
    pub total_collateral_locked: String,
    /// Total collateral locked (formatted for display)
    pub total_collateral_locked_formatted: String,
    /// Total collateral from locked requests that expired (as string)
    pub total_locked_and_expired_collateral: String,
    /// Total collateral from locked requests that expired (formatted for display)
    pub total_locked_and_expired_collateral_formatted: String,
    /// Total number of requests submitted (cumulative)
    pub total_requests_submitted: i64,
    /// Total number of requests submitted onchain (cumulative)
    pub total_requests_submitted_onchain: i64,
    /// Total number of requests submitted offchain (cumulative)
    pub total_requests_submitted_offchain: i64,
    /// Total number of requests locked (cumulative)
    pub total_requests_locked: i64,
    /// Total number of requests slashed (cumulative)
    pub total_requests_slashed: i64,
    /// Total number of requests that expired (cumulative)
    pub total_expired: i64,
    /// Total number of locked requests that expired (cumulative)
    pub total_locked_and_expired: i64,
    /// Total number of locked requests that were fulfilled (cumulative)
    pub total_locked_and_fulfilled: i64,
    /// Total number of secondary fulfillments (cumulative)
    pub total_secondary_fulfillments: i64,
    /// Fulfillment rate for locked orders (percentage)
    pub locked_orders_fulfillment_rate: f32,
    /// Adjusted fulfillment rate for locked orders (percentage)
    #[serde(default)]
    pub locked_orders_fulfillment_rate_adjusted: f32,
    /// Total program cycles computed across all fulfilled requests (cumulative)
    pub total_program_cycles: String,
    /// Total cycles (program + overhead) computed across all fulfilled requests (cumulative)
    pub total_cycles: String,
    /// Total fixed cost (gas cost) across all locked requests (cumulative, as string in wei)
    #[serde(default)]
    pub total_fixed_cost: String,
    /// Total fixed cost (formatted for display)
    #[serde(default)]
    pub total_fixed_cost_formatted: String,
    /// Total variable cost (proving cost) across all locked requests (cumulative, as string in wei)
    #[serde(default)]
    pub total_variable_cost: String,
    /// Total variable cost (formatted for display)
    #[serde(default)]
    pub total_variable_cost_formatted: String,
}

/// Response containing market cumulative data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarketCumulativesResponse {
    /// Chain ID
    pub chain_id: u64,
    pub data: Vec<MarketCumulativeEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

// ─── Requestor Response Types ────────────────────────────────────────────────

/// A single entry in requestor aggregate data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestorAggregateEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address
    pub requestor_address: String,
    /// Timestamp for this aggregate period (Unix timestamp)
    pub timestamp: i64,
    /// ISO 8601 formatted timestamp
    pub timestamp_iso: String,
    /// Total number of fulfilled orders in this period
    pub total_fulfilled: i64,
    /// Unique provers who locked requests in this period
    pub unique_provers_locking_requests: i64,
    /// Total fees locked (as string)
    pub total_fees_locked: String,
    /// Total fees locked (formatted for display)
    pub total_fees_locked_formatted: String,
    /// Total collateral locked (as string)
    pub total_collateral_locked: String,
    /// Total collateral locked (formatted for display)
    pub total_collateral_locked_formatted: String,
    /// Total collateral from locked requests that expired (as string)
    pub total_locked_and_expired_collateral: String,
    /// Total collateral from locked requests that expired (formatted for display)
    pub total_locked_and_expired_collateral_formatted: String,
    /// 10th percentile lock price per cycle (as string)
    pub p10_lock_price_per_cycle: String,
    /// 10th percentile lock price per cycle (formatted for display)
    pub p10_lock_price_per_cycle_formatted: String,
    /// 25th percentile lock price per cycle (as string)
    pub p25_lock_price_per_cycle: String,
    /// 25th percentile lock price per cycle (formatted for display)
    pub p25_lock_price_per_cycle_formatted: String,
    /// Median (p50) lock price per cycle (as string)
    pub p50_lock_price_per_cycle: String,
    /// Median (p50) lock price per cycle (formatted for display)
    pub p50_lock_price_per_cycle_formatted: String,
    /// 75th percentile lock price per cycle (as string)
    pub p75_lock_price_per_cycle: String,
    /// 75th percentile lock price per cycle (formatted for display)
    pub p75_lock_price_per_cycle_formatted: String,
    /// 90th percentile lock price per cycle (as string)
    pub p90_lock_price_per_cycle: String,
    /// 90th percentile lock price per cycle (formatted for display)
    pub p90_lock_price_per_cycle_formatted: String,
    /// 95th percentile lock price per cycle (as string)
    pub p95_lock_price_per_cycle: String,
    /// 95th percentile lock price per cycle (formatted for display)
    pub p95_lock_price_per_cycle_formatted: String,
    /// 99th percentile lock price per cycle (as string)
    pub p99_lock_price_per_cycle: String,
    /// 99th percentile lock price per cycle (formatted for display)
    pub p99_lock_price_per_cycle_formatted: String,
    /// Total number of requests submitted in this period
    pub total_requests_submitted: i64,
    /// Total number of requests submitted onchain in this period
    pub total_requests_submitted_onchain: i64,
    /// Total number of requests submitted offchain in this period
    pub total_requests_submitted_offchain: i64,
    /// Total number of requests locked in this period
    pub total_requests_locked: i64,
    /// Total number of requests slashed in this period
    pub total_requests_slashed: i64,
    /// Total number of requests that expired in this period
    pub total_expired: i64,
    /// Total number of locked requests that expired in this period
    pub total_locked_and_expired: i64,
    /// Total number of locked requests that were fulfilled in this period
    pub total_locked_and_fulfilled: i64,
    /// Total number of secondary fulfillments in this period
    pub total_secondary_fulfillments: i64,
    /// Fulfillment rate for locked orders (percentage)
    pub locked_orders_fulfillment_rate: f32,
    /// Adjusted fulfillment rate for locked orders (percentage)
    #[serde(default)]
    pub locked_orders_fulfillment_rate_adjusted: f32,
    /// Total program cycles computed across all fulfilled requests in this period
    pub total_program_cycles: String,
    /// Total cycles (program + overhead) computed across all fulfilled requests in this period
    pub total_cycles: String,
    /// Total fixed cost (gas cost) across all locked requests in this period (as string in wei)
    #[serde(default)]
    pub total_fixed_cost: String,
    /// Total fixed cost (formatted for display)
    #[serde(default)]
    pub total_fixed_cost_formatted: String,
    /// Total variable cost (proving cost) across all locked requests in this period (as string in wei)
    #[serde(default)]
    pub total_variable_cost: String,
    /// Total variable cost (formatted for display)
    #[serde(default)]
    pub total_variable_cost_formatted: String,
    /// Epoch number at the start of this period (None if timestamp is before epoch 0)
    #[serde(default)]
    pub epoch_number_start: Option<i64>,
}

/// Response containing requestor aggregate data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestorAggregatesResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address
    pub requestor_address: String,
    /// The aggregation granularity used: hourly, daily, or weekly
    pub aggregation: AggregationGranularity,
    pub data: Vec<RequestorAggregateEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// A single entry in requestor cumulative data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestorCumulativeEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address
    pub requestor_address: String,
    /// Timestamp for this cumulative snapshot (Unix timestamp)
    pub timestamp: i64,
    /// ISO 8601 formatted timestamp
    pub timestamp_iso: String,
    /// Total number of fulfilled orders (cumulative)
    pub total_fulfilled: i64,
    /// Unique provers who locked requests (cumulative)
    pub unique_provers_locking_requests: i64,
    /// Total fees locked (as string)
    pub total_fees_locked: String,
    /// Total fees locked (formatted for display)
    pub total_fees_locked_formatted: String,
    /// Total collateral locked (as string)
    pub total_collateral_locked: String,
    /// Total collateral locked (formatted for display)
    pub total_collateral_locked_formatted: String,
    /// Total collateral from locked requests that expired (as string)
    pub total_locked_and_expired_collateral: String,
    /// Total collateral from locked requests that expired (formatted for display)
    pub total_locked_and_expired_collateral_formatted: String,
    /// Total number of requests submitted (cumulative)
    pub total_requests_submitted: i64,
    /// Total number of requests submitted onchain (cumulative)
    pub total_requests_submitted_onchain: i64,
    /// Total number of requests submitted offchain (cumulative)
    pub total_requests_submitted_offchain: i64,
    /// Total number of requests locked (cumulative)
    pub total_requests_locked: i64,
    /// Total number of requests slashed (cumulative)
    pub total_requests_slashed: i64,
    /// Total number of requests that expired (cumulative)
    pub total_expired: i64,
    /// Total number of locked requests that expired (cumulative)
    pub total_locked_and_expired: i64,
    /// Total number of locked requests that were fulfilled (cumulative)
    pub total_locked_and_fulfilled: i64,
    /// Total number of secondary fulfillments (cumulative)
    pub total_secondary_fulfillments: i64,
    /// Fulfillment rate for locked orders (percentage)
    pub locked_orders_fulfillment_rate: f32,
    /// Adjusted fulfillment rate for locked orders (percentage)
    #[serde(default)]
    pub locked_orders_fulfillment_rate_adjusted: f32,
    /// Total program cycles computed across all fulfilled requests (cumulative)
    pub total_program_cycles: String,
    /// Total cycles (program + overhead) computed across all fulfilled requests (cumulative)
    pub total_cycles: String,
    /// Total fixed cost (gas cost) across all locked requests (cumulative, as string in wei)
    #[serde(default)]
    pub total_fixed_cost: String,
    /// Total fixed cost (formatted for display)
    #[serde(default)]
    pub total_fixed_cost_formatted: String,
    /// Total variable cost (proving cost) across all locked requests (cumulative, as string in wei)
    #[serde(default)]
    pub total_variable_cost: String,
    /// Total variable cost (formatted for display)
    #[serde(default)]
    pub total_variable_cost_formatted: String,
}

/// Response containing requestor cumulative data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestorCumulativesResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address
    pub requestor_address: String,
    pub data: Vec<RequestorCumulativeEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Cursor for requestor leaderboard pagination.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestorLeaderboardCursor {
    pub orders_requested: u64,
    pub address: String,
}

/// A single entry in the requestor leaderboard.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestorLeaderboardEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address (checksummed)
    pub requestor_address: String,
    /// Total orders requested in the period
    pub orders_requested: u64,
    /// Total orders locked in the period
    pub orders_locked: u64,
    /// Total orders fulfilled (locked orders that were successfully proved)
    pub orders_fulfilled: u64,
    /// Total orders expired (locked orders that expired without fulfillment)
    pub orders_expired: u64,
    /// Total orders that expired without ever being locked
    pub orders_not_locked_and_expired: u64,
    /// Total cycles requested (as string)
    pub cycles_requested: String,
    /// Total cycles requested (formatted for display)
    pub cycles_requested_formatted: String,
    /// Median lock price per cycle (as string, null if no locked orders)
    pub median_lock_price_per_cycle: Option<String>,
    /// Median lock price per cycle (formatted for display)
    pub median_lock_price_per_cycle_formatted: Option<String>,
    /// P50 fixed cost (gas cost) per request (as string in wei)
    pub p50_fixed_cost: String,
    /// P50 fixed cost (formatted for display)
    pub p50_fixed_cost_formatted: String,
    /// P50 variable cost (proving cost) per cycle (as string in wei)
    pub p50_variable_cost_per_cycle: String,
    /// P50 variable cost per cycle (formatted for display)
    pub p50_variable_cost_per_cycle_formatted: String,
    /// Acceptance rate (locked / (locked + not_locked_and_expired)) as percentage
    pub acceptance_rate: f32,
    /// Locked order fulfillment rate as percentage
    pub locked_order_fulfillment_rate: f32,
    /// Locked order fulfillment rate adjusted (deduplicated by input_data and image_url) as percentage
    pub locked_orders_fulfillment_rate_adjusted: f32,
    /// Last activity timestamp (Unix)
    pub last_activity_time: i64,
    /// Last activity timestamp (ISO 8601)
    pub last_activity_time_iso: String,
}

/// Response containing the requestor leaderboard.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestorLeaderboardResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Time period for the leaderboard
    pub period: String,
    /// Start timestamp of the query period (Unix)
    pub period_start: i64,
    /// End timestamp of the query period (Unix)
    pub period_end: i64,
    /// Leaderboard entries sorted by cycles
    pub data: Vec<RequestorLeaderboardEntry>,
    /// Cursor for next page, null if no more results
    pub next_cursor: Option<String>,
    /// Whether there are more results available
    pub has_more: bool,
}

// ─── Prover Response Types ───────────────────────────────────────────────────

/// A single entry in prover aggregate data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProverAggregateEntry {
    pub chain_id: u64,
    pub prover_address: String,
    pub timestamp: i64,
    pub timestamp_iso: String,
    pub total_requests_locked: i64,
    pub total_requests_fulfilled: i64,
    pub total_unique_requestors: i64,
    pub total_fees_earned: String,
    pub total_fees_earned_formatted: String,
    pub total_collateral_locked: String,
    pub total_collateral_locked_formatted: String,
    pub total_collateral_slashed: String,
    pub total_collateral_slashed_formatted: String,
    pub total_collateral_earned: String,
    pub total_collateral_earned_formatted: String,
    pub total_requests_locked_and_expired: i64,
    pub total_requests_locked_and_fulfilled: i64,
    pub locked_orders_fulfillment_rate: f32,
    pub p10_lock_price_per_cycle: String,
    pub p10_lock_price_per_cycle_formatted: String,
    pub p25_lock_price_per_cycle: String,
    pub p25_lock_price_per_cycle_formatted: String,
    pub p50_lock_price_per_cycle: String,
    pub p50_lock_price_per_cycle_formatted: String,
    pub p75_lock_price_per_cycle: String,
    pub p75_lock_price_per_cycle_formatted: String,
    pub p90_lock_price_per_cycle: String,
    pub p90_lock_price_per_cycle_formatted: String,
    pub p95_lock_price_per_cycle: String,
    pub p95_lock_price_per_cycle_formatted: String,
    pub p99_lock_price_per_cycle: String,
    pub p99_lock_price_per_cycle_formatted: String,
    pub total_program_cycles: String,
    pub total_cycles: String,
    /// Epoch number at the start of this period (None if timestamp is before epoch 0)
    #[serde(default)]
    pub epoch_number_start: Option<i64>,
}

/// Response containing prover aggregate data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProverAggregatesResponse {
    pub chain_id: u64,
    pub prover_address: String,
    pub aggregation: AggregationGranularity,
    pub data: Vec<ProverAggregateEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// A single entry in prover cumulative data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProverCumulativeEntry {
    pub chain_id: u64,
    pub prover_address: String,
    pub timestamp: i64,
    pub timestamp_iso: String,
    pub total_requests_locked: i64,
    pub total_requests_fulfilled: i64,
    pub total_unique_requestors: i64,
    pub total_fees_earned: String,
    pub total_fees_earned_formatted: String,
    pub total_collateral_locked: String,
    pub total_collateral_locked_formatted: String,
    pub total_collateral_slashed: String,
    pub total_collateral_slashed_formatted: String,
    pub total_collateral_earned: String,
    pub total_collateral_earned_formatted: String,
    pub total_requests_locked_and_expired: i64,
    pub total_requests_locked_and_fulfilled: i64,
    pub locked_orders_fulfillment_rate: f32,
    pub total_program_cycles: String,
    pub total_cycles: String,
}

/// Response containing prover cumulative data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProverCumulativesResponse {
    pub chain_id: u64,
    pub prover_address: String,
    pub data: Vec<ProverCumulativeEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Cursor for prover leaderboard pagination.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProverLeaderboardCursor {
    pub fees_earned: String,
    pub address: String,
}

/// A single entry in the prover leaderboard.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProverLeaderboardEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Prover address (checksummed)
    pub prover_address: String,
    /// Total orders locked in the period
    pub orders_locked: u64,
    /// Total orders fulfilled in the period
    pub orders_fulfilled: u64,
    /// Total cycles proven (as string)
    pub cycles: String,
    /// Total cycles proven (formatted for display)
    pub cycles_formatted: String,
    /// Total fees earned (as string)
    pub fees_earned: String,
    /// Total fees earned (formatted for display)
    pub fees_earned_formatted: String,
    /// Total collateral earned from slashing (as string)
    pub collateral_earned: String,
    /// Total collateral earned (formatted for display)
    pub collateral_earned_formatted: String,
    /// Median lock price per cycle (as string, null if no locked orders)
    pub median_lock_price_per_cycle: Option<String>,
    /// Median lock price per cycle (formatted for display)
    pub median_lock_price_per_cycle_formatted: Option<String>,
    /// Best effective proving speed in MHz
    pub best_effective_prove_mhz: f64,
    /// Locked order fulfillment rate as percentage (0-100)
    pub locked_order_fulfillment_rate: f32,
    /// Last activity timestamp (Unix)
    pub last_activity_time: i64,
    /// Last activity timestamp (ISO 8601)
    pub last_activity_time_iso: String,
}

/// Response containing the prover leaderboard.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProverLeaderboardResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Time period for the leaderboard
    pub period: String,
    /// Start timestamp of the query period (Unix)
    pub period_start: i64,
    /// End timestamp of the query period (Unix)
    pub period_end: i64,
    /// Leaderboard entries sorted by fees earned
    pub data: Vec<ProverLeaderboardEntry>,
    /// Cursor for next page, null if no more results
    pub next_cursor: Option<String>,
    /// Whether there are more results available
    pub has_more: bool,
}

// ─── Request Response Types ──────────────────────────────────────────────────

/// Detailed status of a single proof request.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestStatusResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Request digest (unique identifier)
    pub request_digest: String,
    /// Request ID (can be non-unique)
    pub request_id: String,
    /// Current status: submitted, locked, fulfilled, slashed, expired
    pub request_status: String,
    /// Source: onchain, offchain, unknown
    pub source: String,
    /// Client address
    pub client_address: String,
    /// Prover address (if locked)
    pub lock_prover_address: Option<String>,
    /// Fulfill prover address (if fulfilled)
    pub fulfill_prover_address: Option<String>,
    /// Created timestamp (Unix)
    pub created_at: i64,
    /// Created timestamp (ISO 8601)
    pub created_at_iso: String,
    /// Last updated timestamp (Unix)
    pub updated_at: i64,
    /// Last updated timestamp (ISO 8601)
    pub updated_at_iso: String,
    /// Locked timestamp (Unix)
    pub locked_at: Option<i64>,
    /// Locked timestamp (ISO 8601)
    pub locked_at_iso: Option<String>,
    /// Fulfilled timestamp (Unix)
    pub fulfilled_at: Option<i64>,
    /// Fulfilled timestamp (ISO 8601)
    pub fulfilled_at_iso: Option<String>,
    /// Slashed timestamp (Unix)
    pub slashed_at: Option<i64>,
    /// Slashed timestamp (ISO 8601)
    pub slashed_at_iso: Option<String>,
    /// Lock prover delivered proof timestamp (Unix)
    pub lock_prover_delivered_proof_at: Option<i64>,
    /// Lock prover delivered proof timestamp (ISO 8601)
    pub lock_prover_delivered_proof_at_iso: Option<String>,
    /// Submit block number
    pub submit_block: Option<i64>,
    /// Lock block number
    pub lock_block: Option<i64>,
    /// Fulfill block number
    pub fulfill_block: Option<i64>,
    /// Slashed block number
    pub slashed_block: Option<i64>,
    /// Minimum price (wei)
    pub min_price: String,
    /// Minimum price (formatted)
    pub min_price_formatted: String,
    /// Maximum price (wei)
    pub max_price: String,
    /// Maximum price (formatted)
    pub max_price_formatted: String,
    /// Lock collateral (wei)
    pub lock_collateral: String,
    /// Lock collateral (formatted)
    pub lock_collateral_formatted: String,
    /// Lock price (wei)
    pub lock_price: Option<String>,
    /// Lock price (formatted)
    pub lock_price_formatted: Option<String>,
    /// Lock price per cycle (wei)
    pub lock_price_per_cycle: Option<String>,
    /// Lock price per cycle (formatted)
    pub lock_price_per_cycle_formatted: Option<String>,
    /// Fixed cost (gas cost) for this request (as string in wei)
    pub fixed_cost: Option<String>,
    /// Fixed cost (formatted for display)
    pub fixed_cost_formatted: Option<String>,
    /// Variable cost per cycle (proving cost) for this request (as string in wei)
    pub variable_cost_per_cycle: Option<String>,
    /// Variable cost per cycle (formatted for display)
    pub variable_cost_per_cycle_formatted: Option<String>,
    /// Base fee per gas at lock block (wei)
    pub lock_base_fee: Option<String>,
    /// Base fee per gas at fulfill block (wei)
    pub fulfill_base_fee: Option<String>,
    /// Ramp up start timestamp
    pub ramp_up_start: i64,
    /// Ramp up start timestamp (ISO 8601)
    pub ramp_up_start_iso: String,
    /// Ramp up period (seconds)
    pub ramp_up_period: i64,
    /// Expires at timestamp
    pub expires_at: i64,
    /// Expires at timestamp (ISO 8601)
    pub expires_at_iso: String,
    /// Lock end timestamp
    pub lock_end: i64,
    /// Lock end timestamp (ISO 8601)
    pub lock_end_iso: String,
    /// Slash recipient address
    pub slash_recipient: Option<String>,
    /// Slash transferred amount
    pub slash_transferred_amount: Option<String>,
    /// Slash transferred amount (formatted)
    pub slash_transferred_amount_formatted: Option<String>,
    /// Slash burned amount
    pub slash_burned_amount: Option<String>,
    /// Slash burned amount (formatted)
    pub slash_burned_amount_formatted: Option<String>,
    /// Program cycles (guest program only)
    pub program_cycles: Option<String>,
    /// Total cycles (program + overhead)
    pub total_cycles: Option<String>,
    /// Effective prove MHz (from requestor perspective: fulfilled_at - created_at)
    pub effective_prove_mhz: Option<f64>,
    /// Prover effective prove MHz (from prover perspective: fulfilled_at - locked_at or lock_end)
    pub prover_effective_prove_mhz: Option<f64>,
    /// Submit transaction hash
    pub submit_tx_hash: Option<String>,
    /// Lock transaction hash
    pub lock_tx_hash: Option<String>,
    /// Fulfill transaction hash
    pub fulfill_tx_hash: Option<String>,
    /// Slash transaction hash
    pub slash_tx_hash: Option<String>,
    /// Image ID
    pub image_id: String,
    /// Image URL
    pub image_url: Option<String>,
    /// Selector
    pub selector: String,
    /// Predicate type
    pub predicate_type: String,
    /// Predicate data (hex)
    pub predicate_data: String,
    /// Input type
    pub input_type: String,
    /// Input data (hex)
    pub input_data: String,
    /// Fulfillment journal (hex)
    pub fulfill_journal: Option<String>,
    /// Fulfillment seal (hex)
    pub fulfill_seal: Option<String>,
}

/// Response containing a list of requests.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestListResponse {
    /// Chain ID
    pub chain_id: u64,
    pub data: Vec<RequestStatusResponse>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}
