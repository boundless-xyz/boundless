// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::sync::Arc;

use alloy::{
    network::{Ethereum, EthereumWallet},
    primitives::{Address, U256},
    providers::{
        fillers::{
            BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
            WalletFiller,
        },
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    signers::local::PrivateKeySigner,
    transports::{http::Http, Transport},
};
use anyhow::Context;
use boundless_market::contracts::boundless_market::{BoundlessMarketService, MarketError};
use db::{DbError, DbObj, SqliteDb};
use futures_util::StreamExt;
use reqwest::Client as HttpClient;
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

mod db;

type ProviderWallet = FillProvider<
    JoinFill<
        JoinFill<
            Identity,
            JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
        >,
        WalletFiller<EthereumWallet>,
    >,
    RootProvider<Http<HttpClient>>,
    Http<HttpClient>,
    Ethereum,
>;

#[derive(Error, Debug)]
pub enum ServiceError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] DbError),

    #[error("Boundless market error: {0}")]
    BoundlessMarketError(#[from] MarketError),

    /// General error.
    #[error("Error: {0}")]
    Error(#[from] anyhow::Error),
}

#[derive(Clone)]
pub struct SlashService<T, P> {
    pub boundless_market: BoundlessMarketService<T, P>,
    pub db: DbObj,
    pub interval: Duration,
}

impl SlashService<Http<HttpClient>, ProviderWallet> {
    pub async fn new(
        rpc_url: Url,
        private_key: &PrivateKeySigner,
        boundless_market_address: Address,
        db_conn: &str,
        interval: Duration,
    ) -> Result<Self, ServiceError> {
        let caller = private_key.address();
        let wallet = EthereumWallet::from(private_key.clone());
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet.clone())
            .on_http(rpc_url);

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);

        let db: DbObj = Arc::new(SqliteDb::new(db_conn).await.unwrap());

        Ok(Self { boundless_market, db, interval })
    }
}
impl<T, P> SlashService<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    pub async fn run(self) -> Result<(), ServiceError> {
        let current_block = self.current_block().await?;
        let last_processed_block = self.get_last_processed_block().await?.unwrap_or(current_block);

        // Catch any missed events on startup
        self.catch_missed_locked_events(last_processed_block, current_block).await?;
        self.catch_missed_fulfilled_events(last_processed_block, current_block).await?;
        self.catch_missed_slashed_events(last_processed_block, current_block).await?;

        // Update last processed block
        self.update_last_processed_block(current_block).await?;

        let self_locked = self.clone();
        let self_fulfilled = self.clone();
        let self_slashed = self.clone();

        // Spawn watching tasks with retry
        let mut locked_handle = tokio::spawn(async move { self_locked.watch_locked().await });
        let mut fulfilled_handle =
            tokio::spawn(async move { self_fulfilled.watch_fulfilled().await });
        let mut slashed_handle = tokio::spawn(async move { self_slashed.watch_slashed().await });

        let mut interval = tokio::time::interval(self.interval);

        loop {
            tokio::select! {
                result = &mut locked_handle => {
                    tracing::error!("Locked watch task terminated unexpectedly: {:?}", result);
                    return Err(ServiceError::Error(anyhow::anyhow!("Locked watch task terminated")));
                },
                result = &mut fulfilled_handle => {
                    tracing::error!("Fulfilled watch task terminated unexpectedly: {:?}", result);
                    return Err(ServiceError::Error(anyhow::anyhow!("Fulfilled watch task terminated")));
                },
                result = &mut slashed_handle => {
                    tracing::error!("Slashed watch task terminated unexpectedly: {:?}", result);
                    return Err(ServiceError::Error(anyhow::anyhow!("Slashed watch task terminated")));
                },
                _ = interval.tick() => {
                    self.process_expired_requests().await?;
                }
            }
        }
    }

    async fn watch_locked(self) -> Result<(), ServiceError> {
        let backoff = Duration::from_secs(5);

        loop {
            match self.listen_to_request_locked().await {
                Ok(()) => {
                    tracing::warn!(
                        "RequestLocked watch task completed unexpectedly, restarting..."
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "RequestLocked watch task failed with error: {}. Restarting after delay...",
                        e
                    );
                }
            }

            tokio::time::sleep(backoff).await;
        }
    }

    async fn watch_fulfilled(self) -> Result<(), ServiceError> {
        let backoff = Duration::from_secs(5);

        loop {
            match self.listen_to_request_fulfilled().await {
                Ok(()) => {
                    tracing::warn!(
                        "RequestFulfilled watch task completed unexpectedly, restarting..."
                    );
                }
                Err(e) => {
                    tracing::error!("RequestFulfilled watch task failed with error: {}. Restarting after delay...", e);
                }
            }

            tokio::time::sleep(backoff).await;
        }
    }

    async fn watch_slashed(self) -> Result<(), ServiceError> {
        let backoff = Duration::from_secs(5);

        loop {
            match self.listen_to_request_slashed().await {
                Ok(()) => {
                    tracing::warn!(
                        "ProverSlashed watch task completed unexpectedly, restarting..."
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "ProverSlashed watch task failed with error: {}. Restarting after delay...",
                        e
                    );
                }
            }

            tokio::time::sleep(backoff).await;
        }
    }

    async fn get_last_processed_block(&self) -> Result<Option<u64>, ServiceError> {
        Ok(self.db.get_last_block().await?)
    }

    async fn update_last_processed_block(&self, block_number: u64) -> Result<(), ServiceError> {
        self.db.set_last_block(block_number).await?;
        Ok(())
    }

    async fn catch_missed_locked_events(
        &self,
        last_processed_block: u64,
        current_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .RequestLocked_filter()
            .from_block(last_processed_block)
            .to_block(current_block);

        // Query the logs for the event
        let logs = event_filter.query().await.context("Failed to query RequestLocked events")?;
        for (log, _) in logs {
            self.add_order(log.requestId, None).await?;
        }

        Ok(())
    }

    async fn catch_missed_slashed_events(
        &self,
        last_processed_block: u64,
        current_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .ProverSlashed_filter()
            .from_block(last_processed_block)
            .to_block(current_block);

        // Query the logs for the event
        let logs = event_filter.query().await.context("Failed to query ProverSlashed events")?;
        for (log, _) in logs {
            self.remove_order(log.requestId, None).await?;
        }

        Ok(())
    }

    async fn catch_missed_fulfilled_events(
        &self,
        last_processed_block: u64,
        current_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .RequestFulfilled_filter()
            .from_block(last_processed_block)
            .to_block(current_block);

        // Query the logs for the event
        let logs = event_filter.query().await.context("Failed to query RequestFulfilled events")?;
        for (log, _) in logs {
            self.remove_order(log.requestId, None).await?;
        }

        Ok(())
    }

    async fn add_order(
        &self,
        request_id: U256,
        block_number: Option<u64>,
    ) -> Result<(), ServiceError> {
        let expiration = self.boundless_market.request_deadline(request_id).await?;

        // Insert request into database
        self.db.add_order(request_id, expiration).await?;

        // Update last processed block
        if let Some(block_number) = block_number {
            self.update_last_processed_block(block_number).await?;
        }

        Ok(())
    }

    async fn remove_order(
        &self,
        request_id: U256,
        block_number: Option<u64>,
    ) -> Result<(), ServiceError> {
        // Remove request from database
        self.db.remove_order(request_id).await?;

        // Update last processed block
        if let Some(block_number) = block_number {
            self.update_last_processed_block(block_number).await?;
        }

        Ok(())
    }

    async fn process_expired_requests(&self) -> Result<(), ServiceError> {
        // Find expired requests
        let current_block = self.current_block().await?;
        let expired = self.db.get_expired_orders(current_block).await?;

        for request in expired {
            if let Err(err) = self.boundless_market.slash(request).await {
                // Log error, potentially retry mechanism
                tracing::error!("Slashing failed for request {}: {}", request, err);
            }
        }

        Ok(())
    }

    async fn current_block(&self) -> Result<u64, ServiceError> {
        let current_block = self
            .boundless_market
            .instance()
            .provider()
            .get_block_number()
            .await
            .context("Failed to get block number")?;
        Ok(current_block)
    }

    async fn listen_to_request_locked(&self) -> Result<(), ServiceError> {
        let event = self
            .boundless_market
            .instance()
            .RequestLocked_filter()
            .watch()
            .await
            .context("Failed to subscribe to RequestLocked event")?;

        tracing::info!("Subscribed to RequestLocked event");

        event
            .into_stream()
            .for_each(|log_res| async {
                match log_res {
                    Ok((event, log)) => {
                        tracing::info!("Detected new request {:x}", event.requestId);
                        if let Err(err) = self.add_order(event.requestId, log.block_number).await {
                            tracing::error!("Failed to add new order into DB: {err:?}");
                        }
                    }
                    Err(err) => {
                        tracing::error!("Failed to fetch event log: {:?}", err);
                    }
                }
            })
            .await;
        Ok(())
    }

    async fn listen_to_request_fulfilled(&self) -> Result<(), ServiceError> {
        let event = self
            .boundless_market
            .instance()
            .RequestFulfilled_filter()
            .watch()
            .await
            .context("Failed to subscribe to RequestFulfilled event")?;

        tracing::info!("Subscribed to RequestFulfilled event");

        event
            .into_stream()
            .for_each(|log_res| async {
                match log_res {
                    Ok((event, log)) => {
                        tracing::info!("Detected fulfillment for request {:x}", event.requestId);
                        if let Err(err) = self.remove_order(event.requestId, log.block_number).await
                        {
                            tracing::error!("Failed to drop order from DB: {err:?}");
                        }
                    }
                    Err(err) => {
                        tracing::error!("Failed to fetch event log: {:?}", err);
                    }
                }
            })
            .await;
        Ok(())
    }

    async fn listen_to_request_slashed(&self) -> Result<(), ServiceError> {
        let event = self
            .boundless_market
            .instance()
            .ProverSlashed_filter()
            .watch()
            .await
            .context("Failed to subscribe to ProverSlashed event")?;

        tracing::info!("Subscribed to ProverSlashed event");

        event
            .into_stream()
            .for_each(|log_res| async {
                match log_res {
                    Ok((event, log)) => {
                        tracing::info!("Detected prover slashed for request {:x}", event.requestId);
                        if let Err(err) = self.remove_order(event.requestId, log.block_number).await
                        {
                            tracing::error!("Failed to drop order from DB: {err:?}");
                        }
                    }
                    Err(err) => {
                        tracing::error!("Failed to fetch event log: {:?}", err);
                    }
                }
            })
            .await;
        Ok(())
    }
}
