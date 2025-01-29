// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    providers::Provider,
    primitives::{Address, U256, BlockNumber},
};
use sqlx::{SqlitePool, sqlite::SqliteQueryResult};
use tokio::time::{sleep, Duration};
use thiserror::Error;

#[derive(Error, Debug)]
enum ServiceError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
    
    #[error("Blockchain interaction error: {0}")]
    BlockchainError(String),
}

struct RequestTrackingService {
    market: BoundlessMarketService,
    db_pool: SqlitePool,
}

impl SlashService {
    async fn new(
        rpc_url: &str, 
        private_key: PrivateKeySigner,
        contract_address: Address,
        database_url: &str, 
    ) -> Result<Self, ServiceError> {
        let provider = Provider::new_from_url(rpc_url)?;
        let db_pool = SqlitePool::connect(database_url).await?;
        
        // Create tables for events and last processed block
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS requests (
                id TEXT PRIMARY KEY,
                block_number INTEGER NOT NULL,
                expiration INTEGER NOT NULL,
                processed BOOLEAN DEFAULT 0
            );
            
            CREATE TABLE IF NOT EXISTS service_state (
                key TEXT PRIMARY KEY,
                value INTEGER NOT NULL
            );
            
            -- Ensure we have an initial last processed block
            INSERT OR IGNORE INTO service_state (key, value) 
            VALUES ('last_processed_block', 0)
        "#)
        .execute(&db_pool)
        .await?;

        Ok(Self {
            provider,
            db_pool,
            contract_address,
        })
    }

    async fn get_last_processed_block(&self) -> Result<U256, ServiceError> {
        let last_block = sqlx::query!(
            "SELECT value FROM service_state WHERE key = 'last_processed_block'"
        )
        .fetch_one(&self.db_pool)
        .await?
        .value;

        Ok(U256::from(last_block))
    }

    async fn update_last_processed_block(&self, block_number: U256) -> Result<(), ServiceError> {
        sqlx::query!(
            "UPDATE service_state SET value = ? WHERE key = 'last_processed_block'",
            block_number.to_string()
        )
        .execute(&self.db_pool)
        .await?;

        Ok(())
    }

    async fn catch_missed_events(&self) -> Result<(), ServiceError> {
        let last_processed_block = self.get_last_processed_block().await?;
        let current_block = self.provider.get_block_number().await?;

        // Fetch all events between last processed and current block
        let filter = self.provider
            .get_logs()
            .address(self.contract_address)
            .from_block(last_processed_block)
            .to_block(current_block)
            .event("RequestCreated(address requester, bytes32 requestId, uint256 expiration)");

        for log in filter.stream().await {
            self.process_request_event(log, false).await?;
        }

        // Update last processed block
        self.update_last_processed_block(current_block).await?;

        Ok(())
    }

    async fn process_request_event(
        &self, 
        log: Log, 
        is_real_time: bool
    ) -> Result<(), ServiceError> {
        let request_id = log.data.get_bytes32("requestId");
        let expiration = log.data.get_uint256("expiration");
        let block_number = log.block_number;

        // Insert or update request, marking as unprocessed
        sqlx::query(r#"
            INSERT OR REPLACE INTO requests 
            (id, block_number, expiration, processed) 
            VALUES (?, ?, ?, 0)
        "#)
        .bind(request_id.to_string())
        .bind(block_number)
        .bind(expiration)
        .execute(&self.db_pool)
        .await?;

        // If this is a real-time event, update last processed block
        if is_real_time {
            self.update_last_processed_block(block_number).await?;
        }

        Ok(())
    }

    async fn process_unprocessed_requests(&self) -> Result<(), ServiceError> {
        // Find unprocessed requests
        let unprocessed_requests = sqlx::query!(
            "SELECT id, expiration FROM requests WHERE processed = 0"
        )
        .fetch_all(&self.db_pool)
        .await?;

        for request in unprocessed_requests {
            // Your slashing logic here
            match self.slash_request(&request.id).await {
                Ok(_) => {
                    // Mark as processed
                    sqlx::query("UPDATE requests SET processed = 1 WHERE id = ?")
                        .bind(&request.id)
                        .execute(&self.db_pool)
                        .await?;
                },
                Err(e) => {
                    // Log error, potentially retry mechanism
                    tracing::error!("Processing failed for request {}: {}", request.id, e);
                }
            }
        }

        Ok(())
    }

    async fn run(&self) -> Result<(), ServiceError> {
        // Catch any missed events on startup
        self.catch_missed_events().await?;

        // Continuous event listening and processing
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            tokio::select! {
                // Real-time event listening
                _ = self.listen_to_events() => {},
                
                // Periodically process unprocessed requests
                _ = interval.tick() => {
                    self.process_unprocessed_requests().await?;
                }
            }
        }
    }

    async fn listen_to_events(&self) -> Result<(), ServiceError> {
        let last_processed_block = self.get_last_processed_block().await?;

        let filter = self.provider
            .get_logs()
            .address(self.contract_address)
            .from_block(last_processed_block)
            .event("RequestCreated(address requester, bytes32 requestId, uint256 expiration)");

        for log in filter.stream().await {
            self.process_request_event(log, true).await?;
        }

        Ok(())
    }
}