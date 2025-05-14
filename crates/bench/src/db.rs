// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::str::FromStr;

use alloy::primitives::Address;
use anyhow::Result;
use sqlx::{
    any::{ AnyConnectOptions, AnyPoolOptions },
    AnyPool, Row,
};

/// The `monitoring` struct provides functionality to monitor and query the indexer database.
pub struct Monitor {
    /// The database connection pool.
    pub db: AnyPool,
}

impl Monitor {
    /// Creates a new instance of the Monitor.
    pub async fn new(conn: &str) -> Result<Self> {
        let opts = AnyConnectOptions::from_str(conn)?;
        let pool = AnyPoolOptions::new().max_connections(5).connect_with(opts).await?;

        let db = pool;
        Ok(Self { db })
    }

    pub async fn fetch_request_digest(&self, request_digest: &str) -> Result<Option<String>> {
        let row =
            sqlx::query("SELECT request_digest FROM proof_requests WHERE request_digest = $1")
                .bind(request_digest)
                .fetch_optional(&self.db)
                .await?;

        if let Some(row) = row {
            let request_digest: String = row.get(0);
            Ok(Some(request_digest))
        } else {
            Ok(None)
        }
    }

    /// Fetches the fulfilled_at timestamp for a given request digest.
    pub async fn fetch_fulfilled_at(&self, request_digest: &str) -> Result<Option<u64>> {
        let row = sqlx::query(
            "SELECT block_timestamp FROM request_fulfilled_events WHERE request_digest = $1",
        )
        .bind(request_digest)
        .fetch_optional(&self.db)
        .await?;

        if let Some(row) = row {
            let fulfilled_at: i64 = row.get(0);
            Ok(Some(fulfilled_at as u64))
        } else {
            Ok(None)
        }
    }

    pub async fn fetch_prover(&self, request_digest: &str) -> Result<Option<Address>> {
        let row = sqlx::query("SELECT prover_address FROM fulfillments WHERE request_digest = $1")
            .bind(request_digest)
            .fetch_optional(&self.db)
            .await?;

        if let Some(row) = row {
            let prover_address: String = row.get(0);
            Ok(Some(Address::from_str(&prover_address)?))
        } else {
            Ok(None)
        }
    }
}
