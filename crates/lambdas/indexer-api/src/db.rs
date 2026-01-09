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

use anyhow::Result;
use boundless_indexer::db::{
    market::{DbObj as MarketDbObj, MarketDb},
    rewards::{RewardsDb, RewardsDbObj},
};
use sqlx::postgres::PgPoolOptions;
use std::{sync::Arc, time::Duration};

/// Application state containing database connections
pub struct AppState {
    pub rewards_db: RewardsDbObj,
    pub market_db: MarketDbObj,
    pub chain_id: u64,
}

impl AppState {
    /// Create new application state with database connection
    /// Uses read-only constructors since Lambda API connects to reader endpoint
    pub async fn new(database_url: &str, chain_id: u64) -> Result<Self> {
        tracing::info!("Connecting to database...");

        // Create rewards database connection (Lambda-optimized: 3 connections, short timeouts)
        // Skip migrations since we're connecting to a reader endpoint
        let rewards_db = RewardsDb::new(database_url, None, true).await?;
        let rewards_db: RewardsDbObj = Arc::new(rewards_db);

        // Create market database connection (Lambda-optimized: 3 connections, short timeouts)
        // Skip migrations since we're connecting to a reader endpoint
        let lambda_pool_options = PgPoolOptions::new()
            .max_connections(3) // Lambda: 25 lambdas × 3 = 75 max connections
            .acquire_timeout(Duration::from_secs(5)) // Lambda: fail fast for users
            .idle_timeout(Some(Duration::from_secs(300))) // Lambda: match container warm time
            .max_lifetime(Some(Duration::from_secs(300))); // Lambda: 5 min max
        let market_db = MarketDb::new(database_url, Some(lambda_pool_options), true).await?;
        let market_db: MarketDbObj = Arc::new(market_db);

        tracing::info!("Database connection established");

        Ok(Self { rewards_db, market_db, chain_id })
    }
}
