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

pub mod market;
pub mod rewards;

use thiserror::Error;

// Re-export common types from market module for backwards compatibility
pub use market::{AnyDb, DbObj, IndexerDb, TxMetadata};

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
}
