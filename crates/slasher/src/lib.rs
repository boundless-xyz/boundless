// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::sync::Arc;

use alloy::{
    network::{Ethereum, EthereumWallet},
    primitives::{Address, BlockNumber, U256},
    providers::{
        fillers::{
            BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
            WalletFiller,
        },
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    rpc::types::{BlockNumberOrTag, Filter, Log},
    signers::local::{LocalSigner, PrivateKeySigner},
    sol_types::SolEvent,
    transports::{http::Http, Transport},
};
use anyhow::Context;
use boundless_market::contracts::{
    boundless_market::{BoundlessMarketService, MarketError},
    IBoundlessMarket::RequestLocked,
};
use db::{DbError, DbObj, SqliteDb};
use futures_util::StreamExt;
use reqwest::Client as HttpClient;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
    Row,
};
use thiserror::Error;
use tokio::time::{sleep, Duration};
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
enum ServiceError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] DbError),

    #[error("Boundless market error: {0}")]
    BoundlessMarketError(#[from] MarketError),

    /// General error.
    #[error("Error: {0}")]
    Error(#[from] anyhow::Error),
}

struct SlashService<T, P> {
    boundless_market: BoundlessMarketService<T, P>,
    contract_address: Address,
    db: DbObj,
}

impl SlashService<Http<HttpClient>, ProviderWallet> {
    async fn new(
        rpc_url: Url,
        private_key: PrivateKeySigner,
        boundless_market_address: Address,
    ) -> Result<Self, ServiceError> {
        let caller = private_key.address();
        let wallet = EthereumWallet::from(private_key.clone());
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet.clone())
            .on_http(rpc_url);

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());

        Ok(Self { boundless_market, contract_address: boundless_market_address, db })
    }
}
impl<T, P> SlashService<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    async fn get_last_processed_block(&self) -> Result<Option<u64>, ServiceError> {
        Ok(self.db.get_last_block().await?)
    }

    async fn update_last_processed_block(&self, block_number: u64) -> Result<(), ServiceError> {
        self.db.set_last_block(block_number).await?;
        Ok(())
    }

    async fn catch_missed_events(&self) -> Result<(), ServiceError> {
        let last_processed_block =
            self.get_last_processed_block().await?.context("No last processed block")?;
        let current_block = self
            .boundless_market
            .instance()
            .provider()
            .get_block_number()
            .await
            .context("Failed to get block number")?;

        let event = self
            .boundless_market
            .instance()
            .RequestLocked_filter()
            .from_block(last_processed_block)
            .to_block(current_block)
            .watch()
            .await
            .context("Failed to subscribe to RequestLocked event")?;
        tracing::info!("Subscribed to RequestSubmitted event");
        event
            .into_stream()
            .for_each(|log_res| async {
                match log_res {
                    Ok((event, log)) => {
                        tracing::info!("Detected new request {:x}", event.requestId);
                        if let Err(err) = self.process_request_event(log, false).await {
                            tracing::error!("Failed to add new order into DB: {err:?}");
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Failed to fetch event log: {:?}", err);
                    }
                }
            })
            .await;

        // Update last processed block
        self.update_last_processed_block(current_block).await?;

        Ok(())
    }

    async fn process_request_event(
        &self,
        log: Log,
        is_real_time: bool,
    ) -> Result<(), ServiceError> {
        let decoded_log = log.log_decode::<RequestLocked>().context("Failed to decode log")?;
        let block_number = log.block_number.context("Failed to get block number")?;
        let request_id = decoded_log.inner.data.requestId;
        let expiration = self.boundless_market.request_deadline(request_id).await?;

        // Insert request into database
        self.db.add_order(request_id, expiration).await?;

        // If this is a real-time event, update last processed block
        if is_real_time {
            self.update_last_processed_block(block_number).await?;
        }

        Ok(())
    }

    // async fn process_unprocessed_requests(&self) -> Result<(), ServiceError> {
    //     // Find unprocessed requests
    //     let unprocessed_requests =
    //         sqlx::query!("SELECT id, expiration FROM requests WHERE processed = 0")
    //             .fetch_all(&self.db_pool)
    //             .await?;

    //     for request in unprocessed_requests {
    //         // Your slashing logic here
    //         match self.boundless_market.slash(&request.id).await {
    //             Ok(_) => {
    //                 // Mark as processed
    //                 sqlx::query("UPDATE requests SET processed = 1 WHERE id = ?")
    //                     .bind(&request.id)
    //                     .execute(&self.db_pool)
    //                     .await?;
    //             }
    //             Err(e) => {
    //                 // Log error, potentially retry mechanism
    //                 tracing::error!("Processing failed for request {}: {}", request.id, e);
    //             }
    //         }
    //     }

    //     Ok(())
    // }

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
                    // self.process_unprocessed_requests().await?;
                }
            }
        }
    }

    async fn listen_to_events(&self) -> Result<(), ServiceError> {
        let last_processed_block =
            self.get_last_processed_block().await?.context("No last processed block")?;

        let event = self
            .boundless_market
            .instance()
            .RequestLocked_filter()
            .from_block(last_processed_block)
            .watch()
            .await
            .context("Failed to subscribe to RequestLocked event")?;
        tracing::info!("Subscribed to RequestSubmitted event");
        event
            .into_stream()
            .for_each(|log_res| async {
                match log_res {
                    Ok((event, log)) => {
                        tracing::info!("Detected new request {:x}", event.requestId);
                        if let Err(err) = self.process_request_event(log, false).await {
                            tracing::error!("Failed to add new order into DB: {err:?}");
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Failed to fetch event log: {:?}", err);
                    }
                }
            })
            .await;
        Ok(())
    }
}
