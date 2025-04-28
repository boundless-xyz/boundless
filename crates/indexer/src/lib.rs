// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{cmp::min, collections::HashMap, sync::Arc};

use alloy::{
    consensus::Transaction,
    eips::BlockNumberOrTag,
    network::{Ethereum, EthereumWallet, TransactionResponse},
    primitives::{Address, Bytes, B256},
    providers::{
        fillers::{
            BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
            WalletFiller,
        },
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
    transports::{RpcError, TransportErrorKind},
};
use anyhow::{anyhow, Context};
use boundless_market::contracts::{
    boundless_market::{decode_calldata, BoundlessMarketService, MarketError},
    EIP712DomainSaltless, IBoundlessMarket,
};
use db::{AnyDb, DbError, DbObj, TxMetadata};
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

mod db;

pub mod test_utils;

type ProviderWallet = FillProvider<
    JoinFill<
        JoinFill<
            Identity,
            JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
        >,
        WalletFiller<EthereumWallet>,
    >,
    RootProvider,
>;

#[derive(Error, Debug)]
pub enum ServiceError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] DbError),

    #[error("Boundless market error: {0}")]
    BoundlessMarketError(#[from] MarketError),

    #[error("RPC error: {0}")]
    RpcError(#[from] RpcError<TransportErrorKind>),

    #[error("Event query error: {0}")]
    EventQueryError(#[from] alloy::contract::Error),

    #[error("Error: {0}")]
    Error(#[from] anyhow::Error),

    #[error("Maximum retries reached")]
    MaxRetries,

    #[error("Request not expired")]
    RequestNotExpired,
}

#[derive(Clone)]
pub struct IndexerService<P> {
    pub boundless_market: BoundlessMarketService<P>,
    pub db: DbObj,
    pub domain: EIP712DomainSaltless,
    pub config: IndexerServiceConfig,
}

#[derive(Clone)]
pub struct IndexerServiceConfig {
    pub interval: Duration,
    pub retries: u32,
}

impl IndexerService<ProviderWallet> {
    pub async fn new(
        rpc_url: Url,
        private_key: &PrivateKeySigner,
        boundless_market_address: Address,
        db_conn: &str,
        config: IndexerServiceConfig,
    ) -> Result<Self, ServiceError> {
        let caller = private_key.address();
        let wallet = EthereumWallet::from(private_key.clone());

        let provider = ProviderBuilder::new().wallet(wallet.clone()).on_http(rpc_url);

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);

        let db: DbObj = Arc::new(AnyDb::new(db_conn).await?);

        let domain = boundless_market.eip712_domain().await?;

        Ok(Self { boundless_market, db, domain, config })
    }
}

impl<P> IndexerService<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    pub async fn run(self, starting_block: Option<u64>) -> Result<(), ServiceError> {
        let mut interval = tokio::time::interval(self.config.interval);
        let current_block = self.current_block().await?;
        let last_processed_block = self.get_last_processed_block().await?.unwrap_or(current_block);
        let mut from_block = min(starting_block.unwrap_or(last_processed_block), current_block);

        let mut attempt = 0;
        loop {
            interval.tick().await;

            match self.current_block().await {
                Ok(to_block) => {
                    if to_block < from_block {
                        continue;
                    }

                    tracing::info!("Processing blocks from {} to {}", from_block, to_block);

                    match self.process_blocks(from_block, to_block).await {
                        Ok(_) => {
                            attempt = 0;
                            from_block = to_block + 1;
                        }
                        Err(e) => match e {
                            // Irrecoverable errors
                            ServiceError::DatabaseError(_)
                            | ServiceError::MaxRetries
                            | ServiceError::RequestNotExpired
                            | ServiceError::Error(_) => {
                                tracing::error!(
                                    "Failed to process blocks from {} to {}: {:?}",
                                    from_block,
                                    to_block,
                                    e
                                );
                                return Err(e);
                            }
                            // Recoverable errors
                            ServiceError::BoundlessMarketError(_)
                            | ServiceError::EventQueryError(_)
                            | ServiceError::RpcError(_) => {
                                attempt += 1;
                                tracing::warn!(
                                    "Failed to process blocks from {} to {}: {:?}, attempt number {}",
                                    from_block,
                                    to_block,
                                    e,
                                    attempt
                                );
                            }
                        },
                    }
                }
                Err(e) => {
                    attempt += 1;
                    tracing::warn!(
                        "Failed to fetch current block: {:?}, attempt number {}",
                        e,
                        attempt
                    );
                }
            }
            if attempt > self.config.retries {
                tracing::error!("Aborting after {} consecutive attempts", attempt);
                return Err(ServiceError::MaxRetries);
            }
        }
    }

    async fn process_blocks(&self, from: u64, to: u64) -> Result<(), ServiceError> {
        // First check for new request submitted events
        self.process_request_submitted_events(from, to).await?;

        // Then check for new locked in requests
        self.process_locked_events(from, to).await?;

        // Then check for delivered/fulfilled events
        let mut cache = TxCache::new();
        self.process_proof_delivered_events(from, to, &mut cache).await?;
        self.process_fulfilled_events(from, to, &mut cache).await?;
        self.process_callback_failed_events(from, to, &mut cache).await?;

        // Check for new slashed events
        self.process_slashed_events(from, to).await?;

        // Check for new deposit events
        self.process_deposit_events(from, to).await?;
        // Check for new withdrawal events
        self.process_withdrawal_events(from, to).await?;
        // Check for new stake deposit events
        self.process_stake_deposit_events(from, to).await?;
        // Check for new stake withdrawal events
        self.process_stake_withdrawal_events(from, to).await?;

        // Update the last processed block
        self.update_last_processed_block(to).await?;

        Ok(())
    }

    async fn get_last_processed_block(&self) -> Result<Option<u64>, ServiceError> {
        Ok(self.db.get_last_block().await?)
    }

    async fn update_last_processed_block(&self, block_number: u64) -> Result<(), ServiceError> {
        Ok(self.db.set_last_block(block_number).await?)
    }

    async fn process_request_submitted_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .RequestSubmitted_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::debug!(
            "Found {} request submitted events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;
            let tx = self
                .boundless_market
                .instance()
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await?
                .context(anyhow!("Transaction not found for hash: {}", hex::encode(tx_hash)))?;

            tracing::debug!(
                "Processing request submitted event for request: 0x{:x}",
                log.requestId
            );

            let request = IBoundlessMarket::submitRequestCall::abi_decode(tx.input(), true)
                .context(anyhow!(
                    "abi decode failure for request submitted event of tx: {}",
                    hex::encode(tx_hash)
                ))?
                .request;

            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    log.requestId
                ))?;

            let block_number = tx.block_number.context("block number not found")?;
            let block_timestamp = match log_data.block_timestamp {
                Some(ts) => ts,
                None => self.block_timestamp(block_number).await?,
            };
            let metadata = TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);

            self.db.add_proof_request(request_digest, request).await?;
            self.db.add_request_submitted_event(request_digest, log.requestId, &metadata).await?;
        }

        Ok(())
    }

    async fn process_locked_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .RequestLocked_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} locked events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            tracing::debug!("Processing request locked event for request: 0x{:x}", log.requestId);
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;
            let tx = self
                .boundless_market
                .instance()
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await?
                .context(anyhow!("Transaction not found for hash: {}", hex::encode(tx_hash)))?;

            let request = IBoundlessMarket::lockRequestCall::abi_decode(tx.input(), true)
                .context(anyhow!(
                    "abi decode failure for request locked event of tx: {}",
                    hex::encode(tx_hash)
                ))?
                .request;

            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    log.requestId
                ))?;

            let block_number = tx.block_number.context("block number not found")?;
            let block_timestamp = match log_data.block_timestamp {
                Some(ts) => ts,
                None => self.block_timestamp(block_number).await?,
            };
            let metadata = TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);

            self.db.add_proof_request(request_digest, request).await?;
            self.db
                .add_request_locked_event(request_digest, log.requestId, log.prover, &metadata)
                .await?;
        }

        Ok(())
    }

    async fn process_proof_delivered_events(
        &self,
        from_block: u64,
        to_block: u64,
        cache: &mut TxCache,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .ProofDelivered_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} proof delivered events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            tracing::debug!("Processing proof delivered event for request: 0x{:x}", log.requestId);
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;
            // Check if the transaction is already in the cache
            // If it is, use the cached block number and tx_input
            // Otherwise, fetch the transaction from the provider and cache it
            // This is to avoid making multiple calls to the provider for the same transaction
            // as delivery events may be emitted in a batch
            let (metadata, tx_input) = match cache.get(tx_hash) {
                Some((metadata, tx_input)) => (metadata.clone(), tx_input),
                None => {
                    let tx = self
                        .boundless_market
                        .instance()
                        .provider()
                        .get_transaction_by_hash(tx_hash)
                        .await?
                        .context(anyhow!(
                            "Transaction not found for hash: {}",
                            hex::encode(tx_hash)
                        ))?;

                    let block_number = tx.block_number.context("block number not found")?;
                    let block_timestamp = match log_data.block_timestamp {
                        Some(ts) => ts,
                        None => self.block_timestamp(block_number).await?,
                    };
                    let metadata =
                        TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);
                    cache.insert(metadata.clone(), tx.input().clone());
                    (metadata, &tx.input().clone())
                }
            };

            let (fills, assessor_receipt) = decode_calldata(tx_input).context(anyhow!(
                "abi decode failure for proof delivered event of tx: {}",
                hex::encode(tx_hash)
            ))?;

            self.db.add_assessor_receipt(assessor_receipt.clone(), &metadata).await?;
            for fill in fills {
                self.db.add_proof_delivered_event(fill.requestDigest, fill.id, &metadata).await?;
                self.db.add_fulfillment(fill, assessor_receipt.prover, &metadata).await?;
            }
        }

        Ok(())
    }

    async fn process_fulfilled_events(
        &self,
        from_block: u64,
        to_block: u64,
        cache: &mut TxCache,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .RequestFulfilled_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} fulfilled events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            tracing::debug!("Processing fulfilled event for request: 0x{:x}", log.requestId);
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;

            // Check if the transaction is already in the cache
            // If it is, use the cached block number and tx_input
            // Otherwise, fetch the transaction from the provider and cache it
            // This is to avoid making multiple calls to the provider for the same transaction
            // as fulfilled events may be emitted in a batch
            let (metadata, tx_input) = match cache.get(tx_hash) {
                Some((metadata, tx_input)) => (metadata.clone(), tx_input),
                None => {
                    let tx = self
                        .boundless_market
                        .instance()
                        .provider()
                        .get_transaction_by_hash(tx_hash)
                        .await?
                        .context(anyhow!(
                            "Transaction not found for hash: {}",
                            hex::encode(tx_hash)
                        ))?;

                    let block_number = tx.block_number.context("block number not found")?;
                    let block_timestamp = match log_data.block_timestamp {
                        Some(ts) => ts,
                        None => self.block_timestamp(block_number).await?,
                    };
                    let metadata =
                        TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);
                    cache.insert(metadata.clone(), tx.input().clone());
                    (metadata, &tx.input().clone())
                }
            };

            let (fills, _) = decode_calldata(tx_input).context(anyhow!(
                "abi decode failure for fulfilled event of tx: {}",
                hex::encode(tx_hash)
            ))?;
            for fill in fills {
                self.db.add_request_fulfilled_event(fill.requestDigest, fill.id, &metadata).await?;
            }
        }

        Ok(())
    }

    async fn process_slashed_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .ProverSlashed_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} slashed events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            tracing::debug!("Processing slashed event for request: 0x{:x}", log.requestId);
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;
            let tx = self
                .boundless_market
                .instance()
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await?
                .context(anyhow!("Transaction not found for hash: {}", hex::encode(tx_hash)))?;

            let block_number = tx.block_number.context("block number not found")?;
            let block_timestamp = match log_data.block_timestamp {
                Some(ts) => ts,
                None => self.block_timestamp(block_number).await?,
            };
            let metadata = TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);

            self.db
                .add_prover_slashed_event(
                    log.requestId,
                    log.stakeBurned,
                    log.stakeTransferred,
                    log.stakeRecipient,
                    &metadata,
                )
                .await?;
        }

        Ok(())
    }

    async fn process_deposit_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .Deposit_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} deposit events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            tracing::debug!("Processing deposit event for account: 0x{:x}", log.account);
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;
            let tx = self
                .boundless_market
                .instance()
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await?
                .context(anyhow!("Transaction not found for hash: {}", hex::encode(tx_hash)))?;

            let block_number = tx.block_number.context("block number not found")?;
            let block_timestamp = match log_data.block_timestamp {
                Some(ts) => ts,
                None => self.block_timestamp(block_number).await?,
            };
            let metadata = TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);
            self.db.add_deposit_event(log.account, log.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_withdrawal_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .Withdrawal_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} withdrawal events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            tracing::debug!("Processing withdrawal event for account: 0x{:x}", log.account);
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;
            let tx = self
                .boundless_market
                .instance()
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await?
                .context(anyhow!("Transaction not found for hash: {}", hex::encode(tx_hash)))?;

            let block_number = tx.block_number.context("block number not found")?;
            let block_timestamp = match log_data.block_timestamp {
                Some(ts) => ts,
                None => self.block_timestamp(block_number).await?,
            };
            let metadata = TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);
            self.db.add_withdrawal_event(log.account, log.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_stake_deposit_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .StakeDeposit_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} stake deposit events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            tracing::debug!("Processing stake deposit event for account: 0x{:x}", log.account);
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;
            let tx = self
                .boundless_market
                .instance()
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await?
                .context(anyhow!("Transaction not found for hash: {}", hex::encode(tx_hash)))?;

            let block_number = tx.block_number.context("block number not found")?;
            let block_timestamp = match log_data.block_timestamp {
                Some(ts) => ts,
                None => self.block_timestamp(block_number).await?,
            };
            let metadata = TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);
            self.db.add_stake_deposit_event(log.account, log.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_stake_withdrawal_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .StakeWithdrawal_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} stake withdrawal events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            tracing::debug!("Processing stake withdrawal event for account: 0x{:x}", log.account);
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;
            let tx = self
                .boundless_market
                .instance()
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await?
                .context(anyhow!("Transaction not found for hash: {}", hex::encode(tx_hash)))?;

            let block_number = tx.block_number.context("block number not found")?;
            let block_timestamp = match log_data.block_timestamp {
                Some(ts) => ts,
                None => self.block_timestamp(block_number).await?,
            };
            let metadata = TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);
            self.db.add_stake_withdrawal_event(log.account, log.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_callback_failed_events(
        &self,
        from_block: u64,
        to_block: u64,
        cache: &mut TxCache,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .CallbackFailed_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} callback failed events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            tracing::debug!("Processing callback failed event for request: 0x{:x}", log.requestId);
            let tx_hash = log_data.transaction_hash.context("Transaction hash not found")?;
            let (metadata, _tx_input) = match cache.get(tx_hash) {
                Some((metadata, tx_input)) => (metadata.clone(), tx_input),
                None => {
                    let tx = self
                        .boundless_market
                        .instance()
                        .provider()
                        .get_transaction_by_hash(tx_hash)
                        .await?
                        .context(anyhow!(
                            "Transaction not found for hash: {}",
                            hex::encode(tx_hash)
                        ))?;

                    let block_number = tx.block_number.context("block number not found")?;
                    let block_timestamp = match log_data.block_timestamp {
                        Some(ts) => ts,
                        None => self.block_timestamp(block_number).await?,
                    };
                    let metadata =
                        TxMetadata::new(tx_hash, tx.from(), block_number, block_timestamp);
                    cache.insert(metadata.clone(), tx.input().clone());
                    (metadata, &tx.input().clone())
                }
            };

            self.db
                .add_callback_failed_event(
                    log.requestId,
                    log.callback,
                    log.error.to_vec(),
                    &metadata,
                )
                .await?;
        }

        Ok(())
    }

    async fn current_block(&self) -> Result<u64, ServiceError> {
        Ok(self.boundless_market.instance().provider().get_block_number().await?)
    }

    async fn block_timestamp(&self, block_number: u64) -> Result<u64, ServiceError> {
        Ok(self
            .boundless_market
            .instance()
            .provider()
            .get_block_by_number(BlockNumberOrTag::Number(block_number))
            .await?
            .context(anyhow!("Failed to get block by number: {}", block_number))?
            .header
            .timestamp)
    }
}

// Cache for transactions to avoid multiple calls to the provider
struct TxCache {
    // Mapping from transaction hash to (from, block number, block timestamp, tx input)
    cache: HashMap<B256, (TxMetadata, Bytes)>,
}

impl TxCache {
    fn new() -> Self {
        Self { cache: HashMap::new() }
    }

    fn get(&self, tx_hash: B256) -> Option<&(TxMetadata, Bytes)> {
        self.cache.get(&tx_hash)
    }

    fn insert(&mut self, metadata: TxMetadata, tx_input: Bytes) {
        self.cache.insert(metadata.tx_hash, (metadata, tx_input));
    }
}
