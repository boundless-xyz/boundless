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

pub mod efficiency;
pub mod events;
pub mod market;
pub mod provers;
pub mod requestors;
pub mod rewards;

use thiserror::Error;

// Re-export common types
pub use efficiency::{
    EfficiencyAggregate, EfficiencyDb, EfficiencyDbImpl, EfficiencyDbObj, EfficiencySummary,
    MarketEfficiencyDaily, MarketEfficiencyHourly, MarketEfficiencyOrder, MoreProfitableSample,
    RequestForEfficiency,
};
pub use events::EventsDb;
pub use market::{
    AllTimeProverSummary, AllTimeRequestorSummary, DailyProverSummary, DailyRequestorSummary,
    DbObj, HourlyProverSummary, HourlyRequestorSummary, IndexerDb, LockPricingData, MarketDb,
    MonthlyProverSummary, MonthlyRequestorSummary, PeriodProverSummary, PeriodRequestorSummary,
    RequestCursor, RequestSortField, RequestStatus, SortDirection, TxMetadata, WeeklyProverSummary,
    WeeklyRequestorSummary,
};
pub use provers::{MarketCollateralStats, ProversDb};
pub use requestors::RequestorDb;

/// Returns a SQL fragment that, when appended to a WHERE clause, filters out
/// duplicate request_id rows keeping only the one with the most advanced status.
/// Returns an empty string when deduplication is disabled.
pub(crate) fn dedup_clause(enabled: bool) -> &'static str {
    if enabled {
        "AND NOT EXISTS (
                       SELECT 1 FROM request_status rs2
                       WHERE rs2.request_id = rs.request_id
                         AND rs2.request_digest != rs.request_digest
                         AND (
                           (CASE rs2.request_status
                                WHEN 'fulfilled' THEN 1 WHEN 'locked' THEN 2
                                WHEN 'submitted' THEN 3 WHEN 'expired' THEN 4 ELSE 5 END
                            <
                            CASE rs.request_status
                                WHEN 'fulfilled' THEN 1 WHEN 'locked' THEN 2
                                WHEN 'submitted' THEN 3 WHEN 'expired' THEN 4 ELSE 5 END)
                           OR (CASE rs2.request_status
                                WHEN 'fulfilled' THEN 1 WHEN 'locked' THEN 2
                                WHEN 'submitted' THEN 3 WHEN 'expired' THEN 4 ELSE 5 END
                               =
                               CASE rs.request_status
                                WHEN 'fulfilled' THEN 1 WHEN 'locked' THEN 2
                                WHEN 'submitted' THEN 3 WHEN 'expired' THEN 4 ELSE 5 END
                               AND (rs2.updated_at > rs.updated_at
                                    OR (rs2.updated_at = rs.updated_at AND rs2.request_digest > rs.request_digest)))
                         )
                   )"
    } else {
        ""
    }
}

#[derive(Error, Debug)]
pub enum DbError {
    #[error("SQL error {0:?}")]
    SqlErr(#[from] sqlx::Error),

    #[error("SQL Migration error {0:?}")]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("Invalid block number: {0}")]
    BadBlockNumb(String),

    #[error("Failed to set last block")]
    SetBlockFail,

    #[error("Invalid transaction: {0}")]
    BadTransaction(String),

    #[error("Error: {0}")]
    Error(#[from] anyhow::Error),
}
