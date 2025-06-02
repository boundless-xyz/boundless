// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{cmp::min, collections::HashMap, sync::Arc};

use alloy::{
    eips::BlockNumberOrTag,
    network::{Ethereum, TransactionResponse},
    primitives::{Address, B256},
    providers::{
        fillers::{ChainIdFiller, FillProvider, JoinFill},
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    rpc::types::Log,
    signers::local::PrivateKeySigner,
    transports::{RpcError, TransportErrorKind},
};
use anyhow::{anyhow, Context};
use boundless_market::contracts::{
    boundless_market::{BoundlessMarketService, MarketError},
    EIP712DomainSaltless,
};
use db::{AnyDb, DbError, DbObj, TxMetadata};
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

mod db;
pub mod test_utils;

const MAX_BATCH_SIZE: u64 = 500;

type ProviderWallet = FillProvider<JoinFill<Identity, ChainIdFiller>, RootProvider>;

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
    // Mapping from transaction hash to TxMetadata
    pub cache: HashMap<B256, TxMetadata>,
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
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_http(rpc_url);
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

        let mut from_block: u64 = self.starting_block(starting_block).await?;
        tracing::info!("Starting indexer at block {}", from_block);

        let mut attempt = 0;
        loop {
            interval.tick().await;

            match self.current_block().await {
                Ok(to_block) => {
                    if to_block < from_block {
                        continue;
                    }

                    // cap to at most 500 blocks per batch
                    let batch_end = min(to_block, from_block.saturating_add(MAX_BATCH_SIZE));

                    tracing::info!("Processing blocks from {} to {}", from_block, batch_end);

                    match self.process_blocks(from_block, batch_end).await {
                        Ok(_) => {
                            attempt = 0;
                            from_block = batch_end + 1;
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
                                    batch_end,
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
                                    batch_end,
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

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;

            tracing::debug!(
                "Processing request submitted event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            let request = event.request.clone();

            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    event.requestId
                ))?;

            self.db.add_proof_request(request_digest, request, &metadata).await?;
            self.db.add_request_submitted_event(request_digest, event.requestId, &metadata).await?;
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
        tracing::debug!(
            "Found {} locked events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing request locked event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );
            let request = event.request.clone();

            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    event.requestId
                ))?;

            // We add the request here also to cover requests that were submitted off-chain,
            // which we currently don't index at submission time.
            let request_exists = self.db.has_proof_request(request_digest).await?;
            if !request_exists {
                tracing::debug!("Detected request locked for unseen request. Likely submitted off-chain: 0x{:x}", event.requestId);
                self.db.add_proof_request(request_digest, request, &metadata).await?;
            }
            self.db
                .add_request_locked_event(request_digest, event.requestId, event.prover, &metadata)
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
        tracing::debug!(
            "Found {} proof delivered events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing proof delivered event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            self.db
                .add_proof_delivered_event(
                    event.fulfillment.requestDigest,
                    event.requestId,
                    &metadata,
                )
                .await?;
            self.db.add_fulfillment(event.fulfillment, event.prover, &metadata).await?;
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
        tracing::debug!(
            "Found {} fulfilled events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing fulfilled event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db
                .add_request_fulfilled_event(
                    event.fulfillment.requestDigest,
                    event.requestId,
                    &metadata,
                )
                .await?;
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
        tracing::debug!(
            "Found {} slashed events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing slashed event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db
                .add_prover_slashed_event(
                    event.requestId,
                    event.stakeBurned,
                    event.stakeTransferred,
                    event.stakeRecipient,
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
        tracing::debug!(
            "Found {} deposit events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing deposit event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_deposit_event(event.account, event.value, &metadata).await?;
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
        tracing::debug!(
            "Found {} withdrawal events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing withdrawal event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_withdrawal_event(event.account, event.value, &metadata).await?;
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
        tracing::debug!(
            "Found {} stake deposit events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing stake deposit event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_stake_deposit_event(event.account, event.value, &metadata).await?;
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
        tracing::debug!(
            "Found {} stake withdrawal events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing stake withdrawal event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_stake_withdrawal_event(event.account, event.value, &metadata).await?;
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
        tracing::debug!(
            "Found {} callback failed events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing callback failed event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            self.db
                .add_callback_failed_event(
                    event.requestId,
                    event.callback,
                    event.error.to_vec(),
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
        let timestamp = self.db.get_block_timestamp(block_number).await?;
        let ts = match timestamp {
            Some(ts) => ts,
            None => {
                tracing::debug!("Block timestamp not found in DB for block {}", block_number);
                let ts = self
                    .boundless_market
                    .instance()
                    .provider()
                    .get_block_by_number(BlockNumberOrTag::Number(block_number))
                    .await?
                    .context(anyhow!("Failed to get block by number: {}", block_number))?
                    .header
                    .timestamp;
                self.db.add_block(block_number, ts).await?;
                ts
            }
        };
        Ok(ts)
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
    }

    // Fetch (and cache) metadata for a tx
    // Check if the transaction is already in the cache
    // If it is, use the cached tx metadata
    // Otherwise, fetch the transaction from the provider and cache it
    // This is to avoid making multiple calls to the provider for the same transaction
    // as delivery events may be emitted in a batch
    async fn fetch_tx_metadata(&mut self, log: Log) -> Result<TxMetadata, ServiceError> {
        let tx_hash = log.transaction_hash.context("Transaction hash not found")?;
        if let Some(meta) = self.cache.get(&tx_hash) {
            return Ok(meta.clone());
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
        self.cache.insert(tx_hash, meta.clone());
        Ok(meta)
    }

    // Return the last processed block from the DB is > 0;
    // otherwise, return the starting_block if set and <= current_block;
    // otherwise, return the current_block.
    async fn starting_block(&self, starting_block: Option<u64>) -> Result<u64, ServiceError> {
        let last_processed = self.get_last_processed_block().await?;
        let current_block = self.current_block().await?;
        Ok(find_starting_block(starting_block, last_processed, current_block))
    }
}

fn find_starting_block(
    starting_block: Option<u64>,
    last_processed: Option<u64>,
    current_block: u64,
) -> u64 {
    if let Some(last) = last_processed.filter(|&b| b > 0) {
        tracing::debug!("Using last processed block {} as starting block", last);
        return last;
    }

    let from = starting_block.unwrap_or(current_block);
    if from > current_block {
        tracing::warn!(
            "Starting block {} is greater than current block {}, defaulting to current block",
            from,
            current_block
        );
        current_block
    } else {
        tracing::debug!("Using {} as starting block", from);
        from
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_find_starting_block() {
        let starting_block = Some(100);
        let last_processed = Some(50);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 50);

        let starting_block = None;
        let last_processed = Some(50);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 50);

        let starting_block = None;
        let last_processed = None;
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 200);

        let starting_block = None;
        let last_processed = Some(0);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 200);

        let starting_block = Some(200);
        let last_processed = None;
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 100);

        let starting_block = Some(200);
        let last_processed = Some(10);
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 10);
    }
}
