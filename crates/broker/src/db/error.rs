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

use alloy::primitives::ruint::ParseError as RuintParseErr;
use thiserror::Error;

use crate::errors::{impl_coded_debug, CodedError};

#[derive(Error)]
pub enum DbError {
    #[error("{code} Order key {0} not found in DB", code = self.code())]
    OrderNotFound(String),

    #[error("{code} Batch key {0} not found in DB", code = self.code())]
    BatchNotFound(usize),

    #[error("{code} Batch key {0} has no aggreagtion state", code = self.code())]
    BatchAggregationStateIsNone(usize),

    #[cfg(test)]
    #[error("{code} Batch insert failed {0}", code = self.code())]
    BatchInsertFailure(usize),

    #[error("{code} DB Missing column value: {0}", code = self.code())]
    MissingElm(&'static str),

    #[error("{code} SQL error {0}", code = self.code())]
    SqlErr(sqlx::Error),

    #[error("{code} SQL Pool timed out {0}", code = self.code())]
    SqlPoolTimedOut(sqlx::Error),

    #[error("{code} SQL Database locked {0}", code = self.code())]
    SqlDatabaseLocked(anyhow::Error),

    #[error("{code} SQL Unique violation {0}", code = self.code())]
    SqlUniqueViolation(sqlx::Error),

    #[error("{code} SQL Migration error", code = self.code())]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("{code} JSON serialization error", code = self.code())]
    JsonErr(#[from] serde_json::Error),

    #[error("{code} Invalid order id", code = self.code())]
    InvalidOrderId(#[from] RuintParseErr),

    #[error("{code} Invalid order id: {0} missing field: {1}", code = self.code())]
    InvalidOrder(String, &'static str),

    #[error("{code} Invalid max connection env var value", code = self.code())]
    MaxConnEnvVar(#[from] std::num::ParseIntError),

    #[error("{code} Duplicate order id accepted {0}", code = self.code())]
    DuplicateOrderId(String),
}

impl_coded_debug!(DbError);

impl CodedError for DbError {
    fn code(&self) -> &str {
        match self {
            DbError::SqlDatabaseLocked(_) => "[B-DB-001]",
            DbError::SqlPoolTimedOut(_) => "[B-DB-002]",
            DbError::SqlUniqueViolation(_) => "[B-DB-003]",
            _ => "[B-DB-500]",
        }
    }
}

impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref db_err) = e {
            let msg = db_err.message().to_string();
            if msg.contains("database is locked") {
                return DbError::SqlDatabaseLocked(anyhow::anyhow!(msg));
            }
            if db_err.is_unique_violation() {
                return DbError::SqlUniqueViolation(e);
            }
        }
        match e {
            sqlx::Error::PoolTimedOut => DbError::SqlPoolTimedOut(e),
            _ => DbError::SqlErr(e),
        }
    }
}
