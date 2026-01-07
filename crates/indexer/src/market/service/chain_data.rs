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

use super::{
    IndexerService, TransactionFetchStrategy, BLOCK_QUERY_SLEEP, GET_BLOCK_BY_NUMBER_CHUNK_SIZE,
    GET_BLOCK_RECEIPTS_CHUNK_SIZE, MARKET_EVENT_SIGNATURES,
};
use crate::db::market::{IndexerDb, TxMetadata};
use crate::market::ServiceError;
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::{Address, B256};
use alloy::providers::Provider;
use alloy::rpc::types::{Filter, Log};
use anyhow::{anyhow, Context};
use futures_util::future::try_join_all;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub(super) async fn fetch_logs(&self, from: u64, to: u64) -> Result<Vec<Log>, ServiceError> {
        let start = std::time::Instant::now();

        // Try to get from cache if enabled
        if let Some(cache) = &self.cache_storage {
            match cache.get_logs(self.chain_id, from, to, MARKET_EVENT_SIGNATURES).await {
                Ok(Some(logs)) => {
                    tracing::info!(
                        "Cache hit for logs from block {} to {} [got {} logs in {:?}]",
                        from,
                        to,
                        logs.len(),
                        start.elapsed()
                    );
                    return Ok(logs);
                }
                Ok(None) => {
                    tracing::warn!("Cache miss for logs from block {} to {}", from, to);
                }
                Err(e) => {
                    tracing::warn!(
                        "Cache read error for logs from block {} to {}: {:?}",
                        from,
                        to,
                        e
                    );
                }
            }
        } else {
            tracing::debug!("No cache storage configured. Fetching logs from RPC without caching.");
        }

        let filter = Filter::new()
            .address(*self.boundless_market.instance().address())
            .from_block(from)
            .to_block(to)
            .event_signature(MARKET_EVENT_SIGNATURES.to_vec());

        tracing::debug!("Fetching logs from RPC: block {} to block {}", from, to);

        let logs = self.logs_provider.get_logs(&filter).await?;

        tracing::debug!("Fetched {} total logs from block {} to block {}", logs.len(), from, to);

        // Save to cache if enabled
        if let Some(cache) = &self.cache_storage {
            if let Err(e) =
                cache.put_logs(self.chain_id, from, to, MARKET_EVENT_SIGNATURES, &logs).await
            {
                tracing::warn!("Failed to cache logs for block {} to {}: {}", from, to, e);
            }
        }

        tracing::info!("fetch_logs completed in {:?} [got {} logs]", start.elapsed(), logs.len());
        Ok(logs)
    }

    pub async fn block_timestamp(&self, block_number: u64) -> Result<u64, ServiceError> {
        // Check in-memory cache first (read-only, populated by fetch_tx_metadata)
        if let Some(ts) = self.block_num_to_timestamp.get(&block_number) {
            return Ok(*ts);
        }

        // Check database
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
                self.db.add_blocks(&[(block_number, ts)]).await?;
                ts
            }
        };
        Ok(ts)
    }

    pub(super) async fn get_tx_metadata(&self, log: Log) -> Result<TxMetadata, ServiceError> {
        let tx_hash = log.transaction_hash.context("Transaction hash not found")?;
        let meta = self.tx_hash_to_metadata.get(&tx_hash).cloned().ok_or_else(|| {
            ServiceError::Error(anyhow!("Transaction not found: {}", hex::encode(tx_hash)))
        })?;

        Ok(meta)
    }
    pub(super) async fn fetch_tx_metadata(
        &mut self,
        logs: &[Log],
        from: u64,
        to: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Try to get from cache if enabled
        if let Some(cache) = &self.cache_storage {
            match cache.get_tx_metadata(self.chain_id, from, to, MARKET_EVENT_SIGNATURES).await {
                Ok(Some(metadata)) => {
                    tracing::info!(
                        "Cache hit for tx metadata from block {} to {} [got {} entries in {:?}]",
                        from,
                        to,
                        metadata.len(),
                        start.elapsed()
                    );
                    // Merge cached metadata into our map
                    self.tx_hash_to_metadata.extend(metadata);
                    return Ok(());
                }
                Ok(None) => {
                    tracing::warn!("Cache miss for tx metadata from block {} to {}", from, to);
                }
                Err(e) => {
                    tracing::warn!(
                        "Cache read error for tx metadata from block {} to {}: {}",
                        from,
                        to,
                        e
                    );
                }
            }
        } else {
            tracing::debug!(
                "No cache storage configured. Fetching tx metadata from RPC without caching."
            );
        }

        // Step 0: Collect unique transaction hashes from all logs
        let mut tx_hashes = HashSet::new();
        for log in logs {
            if let Some(tx_hash) = log.transaction_hash {
                tx_hashes.insert(tx_hash);
            }
        }

        if tx_hashes.is_empty() {
            tracing::debug!("No transaction hashes found in logs");
            tracing::info!("fetch_tx_metadata completed in {:?}", start.elapsed());
            return Ok(());
        }

        // Filter out transactions we already have in cache
        let missing_hashes: Vec<B256> = tx_hashes
            .iter()
            .filter(|&&tx_hash| !self.tx_hash_to_metadata.contains_key(&tx_hash))
            .copied()
            .collect();

        if missing_hashes.is_empty() {
            tracing::info!("All {} transaction hashes already in in-memory cache", tx_hashes.len());
            tracing::info!("fetch_tx_metadata completed in {:?}", start.elapsed());
            return Ok(());
        }

        tracing::debug!(
            "Fetching {} transactions in parallel ({} already cached)",
            missing_hashes.len(),
            tx_hashes.len() - missing_hashes.len()
        );

        // Fetch transaction metadata using the configured strategy
        match self.config.tx_fetch_strategy {
            TransactionFetchStrategy::BlockReceipts => {
                self.fetch_tx_metadata_via_block_receipts(logs, &missing_hashes).await?;
            }
            TransactionFetchStrategy::TransactionByHash => {
                self.fetch_tx_metadata_via_tx_by_hash(&missing_hashes).await?;
            }
        }

        tracing::debug!(
            "Successfully fetched {} transactions and {} block timestamps",
            missing_hashes.len(),
            self.block_num_to_timestamp.len()
        );

        // Save to cache if enabled
        if let Some(cache) = &self.cache_storage {
            if let Err(e) = cache
                .put_tx_metadata(
                    self.chain_id,
                    from,
                    to,
                    MARKET_EVENT_SIGNATURES,
                    &self.tx_hash_to_metadata,
                )
                .await
            {
                tracing::warn!("Failed to cache tx metadata for block {} to {}: {}", from, to, e);
            }
        }

        tracing::info!(
            "fetch_tx_metadata completed in {:?} [got {} transactions and {} block timestamps]",
            start.elapsed(),
            missing_hashes.len(),
            self.block_num_to_timestamp.len()
        );
        Ok(())
    }

    // Fetch transaction metadata using eth_getBlockReceipts (optimized, fewer RPC calls)
    pub(super) async fn fetch_tx_metadata_via_block_receipts(
        &mut self,
        logs: &[Log],
        missing_hashes: &[B256],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        // Step 1: Group transaction hashes by block number (from logs)
        let mut block_to_tx_hashes: HashMap<u64, Vec<B256>> = HashMap::new();
        for log in logs {
            if let (Some(tx_hash), Some(block_num)) = (log.transaction_hash, log.block_number) {
                if missing_hashes.contains(&tx_hash) {
                    block_to_tx_hashes.entry(block_num).or_default().push(tx_hash);
                }
            }
        }

        let block_numbers: Vec<u64> = block_to_tx_hashes.keys().copied().collect();

        // Step 2: Fetch block receipts for each block in parallel
        let mut receipt_map: HashMap<B256, (Address, u64, u64)> = HashMap::new();

        for chunk in block_numbers.chunks(GET_BLOCK_RECEIPTS_CHUNK_SIZE) {
            let receipt_futures: Vec<_> = chunk
                .iter()
                .map(|&block_num| {
                    let provider = self.any_network_provider.clone();
                    async move {
                        let receipts = provider
                            .get_block_receipts(BlockId::Number(BlockNumberOrTag::Number(
                                block_num,
                            )))
                            .await?;
                        Ok::<_, ServiceError>((block_num, receipts))
                    }
                })
                .collect();

            let start = std::time::Instant::now();
            let receipt_results = try_join_all(receipt_futures).await?;
            tracing::info!("Got {} receipts in {:?}", receipt_results.len(), start.elapsed());
            for (block_num, receipts_result) in receipt_results {
                if let Some(receipts) = receipts_result {
                    // Get the tx hashes we're looking for in this block
                    if let Some(expected_hashes) = block_to_tx_hashes.get(&block_num) {
                        for receipt in receipts {
                            let tx_hash = receipt.transaction_hash;
                            if expected_hashes.contains(&tx_hash) {
                                let from = receipt.from;
                                let tx_index = receipt.transaction_index.context(anyhow!(
                                    "Transaction index not found for transaction {}",
                                    hex::encode(tx_hash)
                                ))?;
                                let block_number = receipt.block_number.context(anyhow!(
                                    "Block number not found for transaction {}",
                                    hex::encode(tx_hash)
                                ))?;
                                receipt_map.insert(tx_hash, (from, tx_index, block_number));
                            }
                        }
                    }
                }
            }
        }

        // Verify we got all the transactions we expected
        for &tx_hash in missing_hashes.iter() {
            if !receipt_map.contains_key(&tx_hash) {
                return Err(ServiceError::Error(anyhow!(
                    "Transaction {} receipt not found",
                    hex::encode(tx_hash)
                )));
            }
        }

        // Step 3: Build a set of unique block numbers
        let mut block_numbers_set = HashSet::new();
        for log in logs {
            if let Some(bn) = log.block_number {
                if let Some(log_tx_hash) = log.transaction_hash {
                    if missing_hashes.contains(&log_tx_hash) {
                        block_numbers_set.insert(bn);
                    }
                }
            }
        }

        // Sleep in between the above block_receipt calls and the below ones to reduce rate limiting risk.
        tokio::time::sleep(Duration::from_secs(BLOCK_QUERY_SLEEP)).await;

        // Step 4: Fetch block timestamps in parallel and update service map
        if !block_numbers_set.is_empty() {
            let block_numbers_vec: Vec<u64> = block_numbers_set.into_iter().collect();

            // Fetch all blocks from RPC in parallel (chunked)
            for chunk in block_numbers_vec.chunks(GET_BLOCK_BY_NUMBER_CHUNK_SIZE) {
                let block_futures: Vec<_> = chunk
                    .iter()
                    .map(|&block_num| {
                        let provider = self.boundless_market.instance().provider();
                        async move {
                            let block = provider
                                .get_block_by_number(BlockNumberOrTag::Number(block_num))
                                .await?;
                            Ok::<_, ServiceError>((block_num, block))
                        }
                    })
                    .collect();

                let start = std::time::Instant::now();
                let block_results = try_join_all(block_futures).await?;
                tracing::info!("Got {} blocks in {:?}", block_results.len(), start.elapsed());
                for (block_num, block_result) in block_results {
                    if let Some(block) = block_result {
                        let ts = block.header.timestamp;
                        self.block_num_to_timestamp.insert(block_num, ts);
                    }
                }
            }
        }

        // Batch insert all fetched blocks into database
        let blocks_to_insert: Vec<(u64, u64)> = self
            .block_num_to_timestamp
            .iter()
            .map(|(&block_num, &timestamp)| (block_num, timestamp))
            .collect();
        if !blocks_to_insert.is_empty() {
            self.db.add_blocks(&blocks_to_insert).await?;
        }

        // Step 5: Build final map from tx_hash to TxMetadata and update service map
        for &tx_hash in missing_hashes.iter() {
            let (from, tx_index, bn) = receipt_map
                .get(&tx_hash)
                .copied()
                .context(anyhow!("Receipt not found for transaction {}", hex::encode(tx_hash)))?;

            // Get timestamp from service block_timestamps map
            let ts = self
                .block_num_to_timestamp
                .get(&bn)
                .copied()
                .context(anyhow!("Block {} timestamp not found in map", bn))?;

            let meta = TxMetadata::new(tx_hash, from, bn, ts, tx_index);
            self.tx_hash_to_metadata.insert(tx_hash, meta);
        }

        tracing::info!("fetch_tx_metadata_via_block_receipts completed in {:?}", start.elapsed());
        Ok(())
    }

    // Fetch transaction metadata using eth_getTransactionByHash (fallback method)
    pub(super) async fn fetch_tx_metadata_via_tx_by_hash(
        &mut self,
        missing_hashes: &[B256],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        // Step 1: Fetch all transactions in parallel and store in a map
        let mut tx_map: HashMap<B256, _> = HashMap::new();

        for chunk in missing_hashes.chunks(GET_BLOCK_RECEIPTS_CHUNK_SIZE) {
            let futures: Vec<_> = chunk
                .iter()
                .map(|&tx_hash| {
                    let provider = self.boundless_market.instance().provider();
                    async move {
                        let tx = provider.get_transaction_by_hash(tx_hash).await?;
                        Ok::<_, ServiceError>((tx_hash, tx))
                    }
                })
                .collect();

            let start = std::time::Instant::now();
            let results = try_join_all(futures).await?;
            tracing::debug!("Got {} transactions in {:?}", results.len(), start.elapsed());
            for (tx_hash, tx_result) in results {
                match tx_result {
                    Some(tx) => {
                        tx_map.insert(tx_hash, tx);
                    }
                    None => {
                        return Err(ServiceError::Error(anyhow!(
                            "Transaction {} not found",
                            hex::encode(tx_hash)
                        )));
                    }
                }
            }
        }

        // Step 2: Build a set of unique block numbers from all transactions
        let mut block_numbers = HashSet::new();
        for tx in tx_map.values() {
            if let Some(bn) = tx.block_number {
                block_numbers.insert(bn);
            }
        }

        // Sleep in between the above transaction calls and the below block calls to reduce rate limiting risk.
        tokio::time::sleep(Duration::from_secs(BLOCK_QUERY_SLEEP)).await;

        // Step 3: Fetch block timestamps in parallel and update service map
        if !block_numbers.is_empty() {
            let mut block_numbers_vec: Vec<u64> = block_numbers.into_iter().collect();
            let min_block_number = block_numbers_vec.iter().min().unwrap();

            // We add the previous block number to the list of blocks to fetch. This ensures we cache the block info of the previous block
            // to the range we are indexing. This info is used later during indexing when we check for expired requests.
            if *min_block_number > 0 {
                block_numbers_vec.push(*min_block_number - 1);
            }

            // Fetch all blocks from RPC in parallel (chunked)
            for chunk in block_numbers_vec.chunks(GET_BLOCK_BY_NUMBER_CHUNK_SIZE) {
                let block_futures: Vec<_> = chunk
                    .iter()
                    .map(|&block_num| {
                        let provider = self.boundless_market.instance().provider();
                        async move {
                            let block = provider
                                .get_block_by_number(BlockNumberOrTag::Number(block_num))
                                .await?;
                            Ok::<_, ServiceError>((block_num, block))
                        }
                    })
                    .collect();

                let start = std::time::Instant::now();
                let block_results = try_join_all(block_futures).await?;
                tracing::debug!("Got {} blocks in {:?}", block_results.len(), start.elapsed());
                for (block_num, block_result) in block_results {
                    if let Some(block) = block_result {
                        let ts = block.header.timestamp;
                        self.block_num_to_timestamp.insert(block_num, ts);
                    }
                }
            }
        }

        // Batch insert all fetched blocks into database
        let blocks_to_insert: Vec<(u64, u64)> = self
            .block_num_to_timestamp
            .iter()
            .map(|(&block_num, &timestamp)| (block_num, timestamp))
            .collect();
        if !blocks_to_insert.is_empty() {
            self.db.add_blocks(&blocks_to_insert).await?;
        }

        // Step 4: Build final map from tx_hash to TxMetadata and update service map
        for (tx_hash, tx) in tx_map {
            let bn = tx.block_number.context("block number not found")?;
            let tx_index = tx.transaction_index.context(anyhow!(
                "Transaction index not found for transaction {}",
                hex::encode(tx_hash)
            ))?;

            // Get timestamp from service block_timestamps map
            let ts = self
                .block_num_to_timestamp
                .get(&bn)
                .copied()
                .context(anyhow!("Block {} timestamp not found in map", bn))?;

            let from = tx.inner.signer();
            let meta = TxMetadata::new(tx_hash, from, bn, ts, tx_index);
            self.tx_hash_to_metadata.insert(tx_hash, meta);
        }

        tracing::info!("fetch_tx_metadata_via_tx_by_hash completed in {:?}", start.elapsed());
        Ok(())
    }
}
