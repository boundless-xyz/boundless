// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use std::future::Future;
use std::time::Instant;
use workflow_common::metrics::helpers;

/// Wrapper for database operations that records metrics
pub struct DbMetricsWrapper;

impl DbMetricsWrapper {
    /// Execute a query and record metrics
    pub async fn execute_query<Fut, T, E>(operation_type: &str, query_fn: Fut) -> Result<T, E>
    where
        Fut: Future<Output = Result<T, E>>,
    {
        let start = Instant::now();
        let result = query_fn.await;
        let duration = start.elapsed().as_secs_f64();

        let status = match &result {
            Ok(_) => "success",
            Err(_) => "error",
        };

        helpers::record_db_operation(operation_type, status, duration);
        result
    }

    /// Execute a query that returns a single row
    pub async fn fetch_one<F, T, E>(operation_type: &str, query_fn: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        Self::execute_query(operation_type, query_fn).await
    }

    /// Execute a query that returns multiple rows
    pub async fn fetch_all<F, T, E>(operation_type: &str, query_fn: F) -> Result<Vec<T>, E>
    where
        F: Future<Output = Result<Vec<T>, E>>,
    {
        Self::execute_query(operation_type, query_fn).await
    }

    /// Execute a query that returns an optional row
    pub async fn fetch_optional<F, T, E>(operation_type: &str, query_fn: F) -> Result<Option<T>, E>
    where
        F: Future<Output = Result<Option<T>, E>>,
    {
        Self::execute_query(operation_type, query_fn).await
    }

    /// Execute a query that returns a scalar value
    pub async fn fetch_scalar<F, T, E>(operation_type: &str, query_fn: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        Self::execute_query(operation_type, query_fn).await
    }

    /// Execute a query that performs an update/insert/delete
    pub async fn execute<F, E>(
        operation_type: &str,
        query_fn: F,
    ) -> Result<sqlx::postgres::PgQueryResult, E>
    where
        F: Future<Output = Result<sqlx::postgres::PgQueryResult, E>>,
    {
        Self::execute_query(operation_type, query_fn).await
    }

    /// Execute a transaction
    pub async fn transaction<F, T, E>(operation_type: &str, query_fn: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        Self::execute_query(operation_type, query_fn).await
    }
}

/// Macro to simplify database operation calls with metrics
#[macro_export]
macro_rules! db_operation {
    ($operation_type:expr, $query:expr) => {
        DbMetricsWrapper::execute_query($operation_type, Box::pin($query))
    };
}

/// Macro for fetch_one operations
#[macro_export]
macro_rules! db_fetch_one {
    ($operation_type:expr, $query:expr) => {
        DbMetricsWrapper::fetch_one($operation_type, Box::pin($query))
    };
}

/// Macro for fetch_all operations
#[macro_export]
macro_rules! db_fetch_all {
    ($operation_type:expr, $query:expr) => {
        DbMetricsWrapper::fetch_all($operation_type, Box::pin($query))
    };
}

/// Macro for fetch_optional operations
#[macro_export]
macro_rules! db_fetch_optional {
    ($operation_type:expr, $query:expr) => {
        DbMetricsWrapper::fetch_optional($operation_type, Box::pin($query))
    };
}

/// Macro for execute operations
#[macro_export]
macro_rules! db_execute {
    ($operation_type:expr, $query:expr) => {
        DbMetricsWrapper::execute($operation_type, Box::pin($query))
    };
}
