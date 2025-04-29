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
    rpc::types::Log,
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
    // Mapping from transaction hash to (from, block number, block timestamp, tx input)
    pub cache: HashMap<B256, (TxMetadata, Bytes)>,
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
        let cache = HashMap::new();

        Ok(Self { boundless_market, db, domain, config, cache })
    }
}

impl<P> IndexerService<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    pub async fn run(&mut self, starting_block: Option<u64>) -> Result<(), ServiceError> {
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
                                // exponential backoff with a maximum delay of 120 seconds
                                let delay =
                                    std::time::Duration::from_secs(2u64.pow(attempt - 1).min(120));
                                tracing::warn!(
                                    "Failed to process blocks from {} to {}: {:?}, attempt number {}, retrying in {}s",
                                    from_block,
                                    to_block,
                                    e,
                                    attempt,
                                    delay.as_secs()
                                );
                                tokio::time::sleep(delay).await;
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

    async fn process_blocks(&mut self, from: u64, to: u64) -> Result<(), ServiceError> {
        self.process_request_submitted_events(from, to).await?;
        self.process_locked_events(from, to).await?;
        self.process_proof_delivered_events(from, to).await?;
        self.process_fulfilled_events(from, to).await?;
        self.process_callback_failed_events(from, to).await?;
        self.process_slashed_events(from, to).await?;
        self.process_deposit_events(from, to).await?;
        self.process_withdrawal_events(from, to).await?;
        self.process_stake_deposit_events(from, to).await?;
        self.process_stake_withdrawal_events(from, to).await?;
        self.clear_cache();

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
        &mut self,
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
            let (metadata, input) = self.fetch_tx(log_data).await?;

            tracing::debug!(
                "Processing request submitted event for request: 0x{:x}",
                log.requestId
            );

            let request = IBoundlessMarket::submitRequestCall::abi_decode(&input, true)
                .context(anyhow!(
                    "abi decode failure for request submitted event of tx: {}",
                    hex::encode(metadata.tx_hash)
                ))?
                .request;

            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    log.requestId
                ))?;

            self.db.add_proof_request(request_digest, request).await?;
            self.db.add_request_submitted_event(request_digest, log.requestId, &metadata).await?;
        }

        Ok(())
    }

    async fn process_locked_events(
        &mut self,
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
            let (metadata, input) = self.fetch_tx(log_data).await?;

            let request = IBoundlessMarket::lockRequestCall::abi_decode(&input, true)
                .context(anyhow!(
                    "abi decode failure for request locked event of tx: {}",
                    hex::encode(metadata.tx_hash)
                ))?
                .request;

            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    log.requestId
                ))?;

            self.db.add_proof_request(request_digest, request).await?;
            self.db
                .add_request_locked_event(request_digest, log.requestId, log.prover, &metadata)
                .await?;
        }

        Ok(())
    }

    async fn process_proof_delivered_events(
        &mut self,
        from_block: u64,
        to_block: u64,
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
            let (metadata, input) = self.fetch_tx(log_data).await?;
            let (fills, assessor_receipt) = decode_calldata(&input).context(anyhow!(
                "abi decode failure for proof delivered event of tx: {}",
                hex::encode(metadata.tx_hash)
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
        &mut self,
        from_block: u64,
        to_block: u64,
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
            let (metadata, input) = self.fetch_tx(log_data).await?;

            let (fills, _) = decode_calldata(&input).context(anyhow!(
                "abi decode failure for fulfilled event of tx: {}",
                hex::encode(metadata.tx_hash)
            ))?;
            for fill in fills {
                self.db.add_request_fulfilled_event(fill.requestDigest, fill.id, &metadata).await?;
            }
        }

        Ok(())
    }

    async fn process_slashed_events(
        &mut self,
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
            let (metadata, _) = self.fetch_tx(log_data).await?;
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
        &mut self,
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
            let (metadata, _) = self.fetch_tx(log_data).await?;
            self.db.add_deposit_event(log.account, log.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_withdrawal_events(
        &mut self,
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
            let (metadata, _) = self.fetch_tx(log_data).await?;
            self.db.add_withdrawal_event(log.account, log.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_stake_deposit_events(
        &mut self,
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
            let (metadata, _) = self.fetch_tx(log_data).await?;
            self.db.add_stake_deposit_event(log.account, log.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_stake_withdrawal_events(
        &mut self,
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
            let (metadata, _) = self.fetch_tx(log_data).await?;
            self.db.add_stake_withdrawal_event(log.account, log.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_callback_failed_events(
        &mut self,
        from_block: u64,
        to_block: u64,
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
            let (metadata, _tx_input) = self.fetch_tx(log_data).await?;

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

    fn clear_cache(&mut self) {
        self.cache.clear();
    }

    // Fetch (and cache) metadata and input for a tx
    // Check if the transaction is already in the cache
    // If it is, use the cached tx metadata and tx_input
    // Otherwise, fetch the transaction from the provider and cache it
    // This is to avoid making multiple calls to the provider for the same transaction
    // as delivery events may be emitted in a batch
    async fn fetch_tx(&mut self, log: Log) -> Result<(TxMetadata, Bytes), ServiceError> {
        let tx_hash = log.transaction_hash.context("Transaction hash not found")?;
        if let Some((meta, input)) = self.cache.get(&tx_hash) {
            return Ok((meta.clone(), input.clone()));
        }
        let tx = self
            .boundless_market
            .instance()
            .provider()
            .get_transaction_by_hash(tx_hash)
            .await?
            .context(anyhow!("Transaction not found: {}", hex::encode(tx_hash)))?;
        let bn = tx.block_number.context("block number not found")?;
        let ts =
            if let Some(ts) = log.block_timestamp { ts } else { self.block_timestamp(bn).await? };
        let meta = TxMetadata::new(tx_hash, tx.from(), bn, ts);
        let input = tx.input().clone();
        self.cache.insert(tx_hash, (meta.clone(), input.clone()));
        Ok((meta, input))
    }
}
