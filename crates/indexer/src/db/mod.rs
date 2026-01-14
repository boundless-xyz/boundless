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

pub mod events;
pub mod market;
pub mod provers;
pub mod requestors;
pub mod rewards;

use thiserror::Error;

// Re-export common types from market module
pub use events::EventsDb;
pub use market::{
    AllTimeProverSummary, AllTimeRequestorSummary, DailyProverSummary, DailyRequestorSummary,
    DbObj, HourlyProverSummary, HourlyRequestorSummary, IndexerDb, LockPricingData, MarketDb,
    MonthlyProverSummary, MonthlyRequestorSummary, PeriodProverSummary, PeriodRequestorSummary,
    RequestCursor, RequestSortField, RequestStatus, SortDirection, TxMetadata, WeeklyProverSummary,
    WeeklyRequestorSummary,
};
pub use provers::ProversDb;
pub use requestors::RequestorDb;

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
